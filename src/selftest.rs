//! 自检模式：不启动网页服务，直接以 TS3 客户端身份连上去、进频道、发测试音、收语音帧。
//!
//! 用途：部署时先确认 TS3 这一侧是通的（协议兼容、频道能进、talk power 够、
//! 能收到别人的语音），再去调浏览器那一侧。
//!
//! 用法：TS3WEB_SELFTEST=15 docker compose run --rm ts3web
//!       （数值 = 发测试音多少秒）

use std::rc::Rc;
use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use audiopus::coder::Encoder;
use futures_util::StreamExt;
use slog::{info, o, Logger};
use tokio::time::timeout;
use tsclientlib::{Connection, StreamItem};
use tsproto_packets::packets::{AudioData, CodecType, OutAudio};

use crate::Config;

const SAMPLE_RATE: u32 = 48000;
const FRAME_SAMPLES: usize = 960; // 20ms

pub async fn run(cfg: Rc<Config>, logger: Logger, seconds: u64) -> Result<()> {
    let logger = logger.new(o!("selftest" => true));
    let nick = format!("{}自检", cfg.nick_prefix);
    info!(logger, "自检开始"; "nick" => %nick, "seconds" => seconds);

    let mut builder = Connection::build(cfg.ts3_addr.clone())
        .name(nick.clone())
        .logger(logger.clone());
    if let Some(ch) = &cfg.ts3_channel {
        builder = builder.channel(ch.clone());
    }
    if let Some(p) = &cfg.ts3_channel_password {
        builder = builder.channel_password(p.clone());
    }
    if let Some(p) = &cfg.ts3_server_password {
        builder = builder.password(p.clone());
    }
    let mut con = builder.connect()?;

    // 等连接完成
    {
        let mut events = con.events();
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut connected = false;
        while Instant::now() < deadline {
            match timeout(Duration::from_secs(3), events.next()).await {
                Ok(Some(Ok(StreamItem::BookEvents(evs)))) => {
                    info!(logger, "已连接并同步到服务器"; "events" => evs.len());
                    connected = true;
                    break;
                }
                Ok(Some(Ok(StreamItem::IdentityLevelIncreasing(level)))) => {
                    info!(logger, "正在提升身份等级（服务器要求）"; "level" => level);
                }
                Ok(Some(Ok(_))) => continue,
                Ok(Some(Err(e))) => bail!("连接失败: {e}"),
                Ok(None) => bail!("连接被关闭"),
                Err(_) => continue,
            }
        }
        if !connected {
            bail!("15 秒内没能连上 TS3");
        }
    }

    // 让事件流多跑 3 秒，等 book（客户端/频道列表）填充完整再诊断
    {
        let mut events = con.events();
        let settle = Instant::now() + Duration::from_secs(3);
        while Instant::now() < settle {
            match timeout(Duration::from_millis(300), events.next()).await {
                Ok(Some(Ok(_))) => {}
                Ok(Some(Err(e))) => bail!("同步期间连接出错: {e}"),
                Ok(None) => break,
                Err(_) => {} // 这次没有事件，继续等
            }
        }
    }

    // 诊断：报告我们实际落在哪个频道、频道里都有谁
    match con.get_state() {
        Ok(book) => {
            let own = book.own_client;
            let all_names: Vec<String> = book.channels.values().map(|c| c.name.clone()).collect();
            info!(logger, "诊断：服务器频道一览";
                "count" => all_names.len(),
                "names" => all_names.join(" | "));

            match book.clients.get(&own) {
                Some(me) => {
                    let ch_name = book
                        .channels
                        .get(&me.channel)
                        .map(|c| c.name.clone())
                        .unwrap_or_else(|| format!("(未知频道 {:?})", me.channel));
                    let members: Vec<String> = book
                        .clients
                        .values()
                        .filter(|c| c.channel == me.channel)
                        .map(|c| c.name.clone())
                        .collect();
                    info!(logger, "诊断：我的位置";
                        "nick" => %me.name,
                        "channel" => %ch_name,
                        "same_channel_members" => members.len(),
                        "who" => members.join(" | "));
                }
                None => info!(logger, "诊断：book 里还没有我自己（事件尚未同步完）"),
            }
        }
        Err(e) => info!(logger, "诊断：读取服务器状态失败"; "error" => %e),
    }

    // 准备 Opus 编码器（TS3 用 OpusVoice = 48kHz 单声道 20ms）
    let encoder = Encoder::new(
        audiopus::SampleRate::Hz48000,
        audiopus::Channels::Mono,
        audiopus::Application::Voip,
    )
    .map_err(|e| anyhow::anyhow!("创建 Opus 编码器失败: {e}"))?;

    let mut pcm = vec![0f32; FRAME_SAMPLES];
    let mut opus_out = vec![0u8; 4000];
    let mut phase = 0f64;
    let step = 2.0 * std::f64::consts::PI * 440.0 / SAMPLE_RATE as f64;

    let mut ticker = tokio::time::interval(Duration::from_millis(20));
    let stop_at = Instant::now() + Duration::from_secs(seconds);

    let mut sent: u64 = 0;
    let mut recv_frames: u64 = 0;
    let mut speakers: std::collections::HashSet<u16> = std::collections::HashSet::new();
    let mut send_errors: u64 = 0;
    let mut non_opus: u64 = 0;

    info!(logger, "开始发送 440Hz 测试音，同时统计收到的语音帧…");

    loop {
        let step_res = {
            let mut events = con.events();
            let ev_fut = events.next();
            tokio::pin!(ev_fut);
            tokio::select! {
                item = &mut ev_fut => Ok(item),
                _ = ticker.tick() => Err(()),
            }
        };

        match step_res {
            Ok(item) => match item {
                Some(Ok(StreamItem::Audio(pkt))) => {
                    // pkt.data() 是 &InAudio，再 .data() 一次才是 &AudioData
                    let d = pkt.data().data();
                    let (from, codec) = match d {
                        AudioData::S2C { from, codec, .. } => (*from, *codec),
                        AudioData::S2CWhisper { from, codec, .. } => (*from, *codec),
                        _ => continue,
                    };
                    if matches!(codec, CodecType::OpusVoice | CodecType::OpusMusic) {
                        recv_frames += 1;
                        speakers.insert(from);
                    } else {
                        non_opus += 1;
                    }
                }
                Some(Ok(StreamItem::DisconnectedTemporarily(r))) => {
                    info!(logger, "临时断开，等待重连"; "reason" => ?r);
                }
                // TS3 会用文本消息告知"发言权限不足 / 被服务器拒绝"之类，
                // 这是我们判断访客能不能说话最直接的证据
                Some(Ok(StreamItem::MessageEvent(msg))) => {
                    info!(logger, "服务器消息"; "msg" => ?msg);
                }
                Some(Ok(_)) => {}
                Some(Err(e)) => bail!("TS3 连接错误: {e}"),
                None => bail!("事件流结束"),
            },
            Err(()) => {
                // 该发一帧测试音了
                if Instant::now() >= stop_at {
                    break;
                }
                for s in pcm.iter_mut() {
                    *s = (phase.sin() * 0.25) as f32;
                    phase += step;
                    if phase > 2.0 * std::f64::consts::PI {
                        phase -= 2.0 * std::f64::consts::PI;
                    }
                }
                match encoder.encode_float(&pcm, &mut opus_out) {
                    Ok(len) => {
                        let packet = OutAudio::new(&AudioData::C2S {
                            id: 0,
                            codec: CodecType::OpusVoice,
                            data: &opus_out[..len],
                        });
                        if let Err(e) = con.send_audio(packet) {
                            send_errors += 1;
                            if send_errors == 1 {
                                info!(logger, "发送语音失败"; "error" => %e);
                            }
                        } else {
                            sent += 1;
                        }
                    }
                    Err(e) => info!(logger, "Opus 编码失败"; "error" => %e),
                }
            }
        }
    }

    info!(logger, "自检结束";
        "sent_frames" => sent,
        "send_errors" => send_errors,
        "recv_frames" => recv_frames,
        "speakers" => speakers.len(),
        "non_opus_frames" => non_opus,
    );

    if send_errors > 0 {
        info!(logger, "提示：发送失败通常是 talk power 不足（访客身份被限制发言）");
    }
    if recv_frames == 0 {
        info!(logger, "提示：没收到任何语音帧。可能频道里没人说话，或频道 subscribe power 高于访客权限");
    }

    let _ = con.disconnect(tsclientlib::DisconnectOptions::new());
    Ok(())
}
