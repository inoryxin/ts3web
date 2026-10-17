//! 单个网页会话：WebSocket <-> TS3 音频双向直通。
//!
//! 线协议（尽量简单，零拷贝思路）：
//!   浏览器 -> 服务端：二进制帧 = 裸 Opus 包（浏览器侧固定 20ms 一帧）
//!   服务端 -> 浏览器：二进制帧 = [u16 speaker_id (LE)][u8 duration_ms][Opus 包]
//!                     带上 speaker_id 是因为 Opus 解码器是有状态的：
//!                     不同说话人必须用各自的解码器，否则会有杂音。
//!   双向文本帧 = JSON 控制消息（hello / ready / error / bye / mute / ping）

use std::rc::Rc;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use futures_util::{SinkExt, StreamExt};
use slog::{debug, info, o, Logger};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use tokio_tungstenite::{accept_hdr_async, WebSocketStream};
use tsclientlib::{Connection, MessageHandle, OutCommandExt, StreamItem};
use tsproto_packets::packets::{AudioData, CodecType, Direction, Flags, OutAudio, OutCommand, PacketType};

use crate::opus;
use crate::Config;

/// 给浏览器下发的频道/成员名单（每次 book 事件后节流发送）
///
/// 顺序说明：TS3 的 `channel_order` 不是可排序的序号，而是**链表指针**——
/// 它的值表示"排在我前面的那个频道 id"，0 表示"我是这一层的第一个"。
/// 所以必须顺着链表走，得到的顺序才和 TS3 客户端里看到的一致。
fn state_json(con: &Connection) -> Option<serde_json::Value> {
    use std::collections::HashMap;

    let book = con.get_state().ok()?;
    let me = book.clients.get(&book.own_client)?;

    // 按父子关系分组，并把每一组按链表顺序排好
    let mut by_parent: HashMap<u64, Vec<&tsclientlib::data::Channel>> = HashMap::new();
    for c in book.channels.values() {
        by_parent.entry(c.parent.0).or_default().push(c);
    }
    for group in by_parent.values_mut() {
        let mut ordered: Vec<&tsclientlib::data::Channel> = Vec::with_capacity(group.len());
        // 这一层的第一个：order == 0
        let mut cur = group.iter().find(|c| c.order.0 == 0).map(|c| c.id.0);
        let mut guard = 0;
        while let Some(id) = cur {
            if guard > group.len() + 2 {
                break; // 防御：数据异常时不要死循环
            }
            guard += 1;
            match group.iter().find(|c| c.id.0 == id) {
                Some(c) => {
                    ordered.push(*c);
                    cur = group
                        .iter()
                        .find(|n| n.order.0 == id && n.id.0 != id)
                        .map(|n| n.id.0);
                }
                None => break,
            }
        }
        // 兜底：万一有频道没被串进链表，也别丢
        for c in group.iter() {
            if !ordered.iter().any(|o| o.id == c.id) {
                ordered.push(c);
            }
        }
        *group = ordered;
    }

    fn walk(
        parent: u64, depth: usize,
        by_parent: &std::collections::HashMap<u64, Vec<&tsclientlib::data::Channel>>,
        book: &tsclientlib::data::Connection, out: &mut Vec<serde_json::Value>,
    ) {
        if let Some(group) = by_parent.get(&parent) {
            for c in group {
                let mut members: Vec<serde_json::Value> = book
                    .clients
                    .values()
                    .filter(|cl| cl.channel == c.id)
                    .map(|cl| {
                        serde_json::json!({
                            "id": cl.id.0,
                            "name": cl.name,
                            "you": cl.id == book.own_client,
                        })
                    })
                    .collect();
                members.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
                out.push(serde_json::json!({
                    "id": c.id.0,
                    "name": c.name,
                    "parent": c.parent.0,
                    "depth": depth,
                    "members": members,
                }));
                walk(c.id.0, depth + 1, by_parent, book, out);
            }
        }
    }

    let mut chans: Vec<serde_json::Value> = Vec::with_capacity(book.channels.len());
    walk(0, 0, &by_parent, book, &mut chans);

    Some(serde_json::json!({
        "type": "state",
        "you": { "id": me.id.0, "name": me.name, "channel": me.channel.0 },
        "channels": chans,
    }))
}

/// 切频道：给 TS3 发 clientmove 命令，并要一个应答（这样能知道是不是被权限拒绝）
///
/// 注意：TS3 要求同时给出 `clid`（移动哪个客户端）和 `cid`（目标频道）。
/// 只给 cid 会返回 ParameterNotFound —— 这是实测出来的（见 deploy/raw-probe.sh）。
fn move_to_channel(
    con: &mut Connection, own_id: u16, channel_id: u64, logger: &slog::Logger,
) -> anyhow::Result<MessageHandle> {
    let cmd_text = format!("clientmove clid={own_id} cid={channel_id}");
    debug!(logger, "发送 TS3 命令"; "content" => %cmd_text);
    let cmd = OutCommand::new(Direction::C2S, Flags::empty(), PacketType::Command, &cmd_text);
    Ok(cmd.send_with_result(con)?)
}


/// hello 消息（浏览器发的第一条 JSON）
#[derive(Debug, serde::Deserialize)]
struct Hello {
    #[serde(default)]
    nick: Option<String>,
    #[serde(default)]
    token: Option<String>,
}

/// 一轮循环里发生的事。因为 `con.events()` 会可变借用连接，
/// 所以必须先把"该做什么"取出来，等借用结束再动连接。
///
/// 注意：这里的错误类型必须写全，不能用 anyhow::Result（两者不是同一类型）。
enum Step {
    Ts3(Option<std::result::Result<StreamItem, tsclientlib::Error>>),
    Ws(Option<std::result::Result<Message, WsError>>),
    /// 定时器到点：兜底重发频道/成员名单
    Tick,
}

pub async fn handle(stream: TcpStream, cfg: Rc<Config>, logger: Logger) -> Result<()> {
    // ---- 1. 握手，顺便把 URL query 里的 t / n 参数抓出来 ----
    let mut query = String::new();
    let ws: WebSocketStream<TcpStream> = {
        let callback = |req: &Request, resp: Response| -> std::result::Result<Response, ErrorResponse> {
            query = req.uri().query().unwrap_or_default().to_string();
            Ok(resp)
        };
        accept_hdr_async(stream, callback)
            .await
            .context("WebSocket 握手失败")?
    };
    let params = parse_query(&query);
    let logger = logger.new(o!("session" => short_id()));

    let (mut ws_tx, mut ws_rx) = ws.split();

    // ---- 2. 等 hello（拿到昵称 + 口令） ----
    let hello_timeout = Duration::from_secs(cfg.hello_timeout_secs);
    let mut hello = Hello { nick: None, token: None };
    let deadline = tokio::time::Instant::now() + hello_timeout;

    while hello.nick.is_none() && hello.token.is_none() {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            break;
        }
        match timeout(left, ws_rx.next()).await {
            Ok(Some(Ok(Message::Text(t)))) => {
                hello = serde_json::from_str(t.as_str()).context("hello 不是合法 JSON")?;
                break;
            }
            Ok(Some(Ok(Message::Ping(p)))) => {
                let _ = ws_tx.send(Message::Pong(p)).await;
            }
            Ok(Some(Ok(_))) => continue,
            Ok(Some(Err(e))) => bail!("读 hello 失败: {e}"),
            Ok(None) => bail!("客户端在 hello 之前断开"),
            Err(_) => break,
        }
    }

    // URL 参数优先，其次 hello 里的字段
    let token = params.get("t").cloned().or(hello.token);
    let nick_raw = params
        .get("n")
        .cloned()
        .or(hello.nick)
        .unwrap_or_else(|| "访客".to_string());

    if !cfg.token_ok(token.as_deref()) {
        info!(logger, "口令错误，拒绝连接");
        let _ = ws_tx
            .send(Message::text(
                serde_json::json!({"type":"error","message":"访问口令不正确"}).to_string(),
            ))
            .await;
        let _ = ws_tx.close().await;
        return Ok(());
    }

    let nick = cfg.display_nick(&nick_raw);
    info!(logger, "网页用户接入"; "nick" => %nick);
    let _ = ws_tx
        .send(Message::text(
            serde_json::json!({"type":"status","message":"正在连接 TS3…"}).to_string(),
        ))
        .await;

    // ---- 3. 连接 TS3（一个网页用户 = 一个独立 TS3 身份） ----
    let mut builder = Connection::build(cfg.ts3_addr.clone())
        .name(nick.clone())
        .logger(logger.clone())
        // 把收发到的命令都记进日志（只在 debug 级别输出），排查 clientmove 这类命令时很有用
        .log_commands(true);
    if let Some(ch) = &cfg.ts3_channel {
        builder = builder.channel(ch.clone());
    }
    if let Some(p) = &cfg.ts3_channel_password {
        builder = builder.channel_password(p.clone());
    }
    if let Some(p) = &cfg.ts3_server_password {
        builder = builder.password(p.clone());
    }
    let mut con = builder.connect().context("创建 TS3 连接失败")?;

    // 等 TS3 完成初始同步（拿到第一批 book 事件即认为已连上并进入频道）
    wait_connected(&mut con, &logger).await?;
    info!(logger, "TS3 已连接"; "nick" => %nick);

    let _ = ws_tx
        .send(Message::text(
            serde_json::json!({
                "type": "ready",
                "nick": nick,
                "channel": cfg.ts3_channel.clone().unwrap_or_default(),
                // 服务器是否需要口令（不需要的话，网页就不显示口令输入框）
                "needToken": !cfg.token.is_empty(),
            })
            .to_string(),
        ))
        .await;

    // 订阅所有频道：TS3 只把"已订阅频道"的成员信息发给客户端，
    // 不订阅的话，我们的名单里只能看到自己所在频道的人（其它频道显示 0 人）。
    // 真正的 TS3 客户端连上后也会做这一步。
    let mut pending_subscribe: Option<MessageHandle> = None;
    {
        let cmd = OutCommand::new(
            Direction::C2S,
            Flags::empty(),
            PacketType::Command,
            "channelsubscribeall",
        );
        match cmd.send_with_result(&mut con) {
            Ok(h) => {
                pending_subscribe = Some(h);
                debug!(logger, "已请求订阅全部频道");
            }
            Err(e) => info!(logger, "订阅全部频道失败（名单可能不全）"; "error" => %e),
        }
    }

    // 等 book（客户端/频道列表）填充完整。刚连上时它还是空的，
    // 直接构造名单会失败，而安静的服务器之后可能很久没有事件可触发。
    {
        let deadline = tokio::time::Instant::now() + Duration::from_millis(2500);
        let mut ok = false;
        while tokio::time::Instant::now() < deadline {
            {
                let mut events = con.events();
                let _ = timeout(Duration::from_millis(150), events.next()).await;
            }
            if state_json(&con).is_some() {
                ok = true;
                break;
            }
        }
        debug!(logger, "名单数据就绪"; "ok" => ok);
    }

    // 立刻把频道/成员名单发一份过去
    if let Some(v) = state_json(&con) {
        let _ = ws_tx.send(Message::text(v.to_string())).await;
    }

    // 记住自己的客户端 id：如果 TS3 万一把我们自己的声音回传，就不要转给浏览器
    // （否则网页端会听到自己说话）
    let own_id: Option<u16> = con.get_state().ok().map(|b| b.own_client.0);

    // ---- 4. 主循环：双向转发 ----
    let mut opus_in_frames: u64 = 0;
    let mut opus_out_frames: u64 = 0;
    let mut skipped_codec: Option<CodecType> = None;
    let mut last_state = tokio::time::Instant::now() - Duration::from_secs(5);
    let mut pending_move: Option<(MessageHandle, u64)> = None;
    let mut pending_raw: Option<(MessageHandle, String)> = None;
    // 每 3 秒兜底重发一次名单（同时充当 WebSocket 保活）
    let mut state_tick = tokio::time::interval(Duration::from_secs(3));
    state_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        let step = {
            let mut events = con.events();
            let ev_fut = events.next();
            let ws_fut = ws_rx.next();
            let st_fut = state_tick.tick();
            tokio::pin!(ev_fut);
            tokio::pin!(ws_fut);
            tokio::pin!(st_fut);
            tokio::select! {
                item = &mut ev_fut => Step::Ts3(item),
                msg = &mut ws_fut => Step::Ws(msg),
                _ = &mut st_fut => Step::Tick,
            }
        };

        match step {
            // ---------------- TS3 -> 浏览器 ----------------
            Step::Ts3(item) => match item {
                Some(Ok(StreamItem::Audio(pkt))) => {
                    // pkt.data() 是 &InAudio，再 .data() 一次才是 &AudioData
                    let data = pkt.data().data();
                    let (from, codec, payload) = match data {
                        AudioData::S2C { from, codec, data, .. }
                        | AudioData::S2CWhisper { from, codec, data, .. } => (*from, *codec, *data),
                        _ => continue, // C2S 不该出现在这里
                    };

                    // 自己说的话不要回放给自己
                    if Some(from) == own_id {
                        continue;
                    }

                    // 只有 Opus 能原样直通给浏览器；老客户端可能用 CELT/Speex，跳过
                    if !matches!(codec, CodecType::OpusVoice | CodecType::OpusMusic) {
                        if skipped_codec != Some(codec) {
                            skipped_codec = Some(codec);
                            info!(logger, "收到非 Opus 语音，已跳过"; "codec" => ?codec);
                        }
                        continue;
                    }

                    let dur = opus::packet_duration_ms(payload).unwrap_or(20).min(255) as u8;
                    let mut frame = Vec::with_capacity(payload.len() + 3);
                    frame.extend_from_slice(&from.to_le_bytes());
                    frame.push(dur);
                    frame.extend_from_slice(payload);

                    opus_in_frames += 1;
                    if opus_in_frames % 500 == 1 {
                        debug!(logger, "TS3->网页 语音帧"; "from" => from, "bytes" => payload.len());
                    }
                    if ws_tx.send(Message::binary(frame)).await.is_err() {
                        break;
                    }
                }
                Some(Ok(StreamItem::BookEvents(_))) => {
                    // 频道/成员有变化：节流地把最新名单推给浏览器
                    if last_state.elapsed() > Duration::from_millis(600) {
                        last_state = tokio::time::Instant::now();
                        if let Some(v) = state_json(&con) {
                            if ws_tx.send(Message::text(v.to_string())).await.is_err() {
                                break;
                            }
                        }
                    }
                }
                Some(Ok(StreamItem::MessageResult(handle, result))) => {
                    // 订阅全部频道的应答
                    if pending_subscribe == Some(handle) {
                        pending_subscribe = None;
                        match &result {
                            Ok(()) => {
                                info!(logger, "已订阅全部频道（名单会变完整）");
                                if let Some(v) = state_json(&con) {
                                    let _ = ws_tx.send(Message::text(v.to_string())).await;
                                }
                            }
                            Err(e) => info!(logger, "订阅全部频道被拒绝（只能看到部分频道的人）"; "error" => %e),
                        }
                    }
                    // 调试命令的应答
                    if let Some((h, cmd_text)) = pending_raw.clone() {
                        if h == handle {
                            pending_raw = None;
                            let (ok, detail) = match &result {
                                Ok(()) => (true, String::new()),
                                Err(e) => (false, format!("{e}")),
                            };
                            info!(logger, "调试命令应答"; "cmd" => %cmd_text, "ok" => ok, "detail" => %detail);
                            let _ = ws_tx
                                .send(Message::text(
                                    serde_json::json!({
                                        "type":"raw_result","cmd": cmd_text,
                                        "ok": ok, "error": detail
                                    })
                                    .to_string(),
                                ))
                                .await;
                        }
                    }
                    // 我们请求过应答的命令（目前只有 clientmove）
                    if let Some((h, cid)) = pending_move {
                        if h == handle {
                            pending_move = None;
                            match result {
                                Ok(()) => {
                                    info!(logger, "切换频道成功"; "channel_id" => cid);
                                    if let Some(v) = state_json(&con) {
                                        let _ = ws_tx.send(Message::text(v.to_string())).await;
                                    }
                                }
                                Err(e) => {
                                    info!(logger, "切换频道被拒绝"; "channel_id" => cid, "error" => %e);
                                    let _ = ws_tx
                                        .send(Message::text(
                                            serde_json::json!({
                                                "type":"error",
                                                "message": format!(
                                                    "TS3 拒绝了切换频道：{e}。通常是访客权限不够——\
                                                     让管理员在 TS3 里给 Guest 组或你的身份加上进入该频道的权限。"
                                                )
                                            })
                                            .to_string(),
                                        ))
                                        .await;
                                }
                            }
                        }
                    }
                }
                Some(Ok(StreamItem::DisconnectedTemporarily(reason))) => {
                    info!(logger, "与 TS3 临时断开，等待自动重连"; "reason" => ?reason);
                    let _ = ws_tx
                        .send(Message::text(
                            serde_json::json!({"type":"status","message":"与 TS3 断开，正在重连…"})
                                .to_string(),
                        ))
                        .await;
                }
                Some(Ok(_)) => {}
                Some(Err(e)) => {
                    let _ = ws_tx
                        .send(Message::text(
                            serde_json::json!({"type":"error","message":format!("TS3 连接错误: {e}")})
                                .to_string(),
                        ))
                        .await;
                    bail!("TS3 连接错误: {e}");
                }
                None => bail!("TS3 事件流结束"),
            },

            // ---------------- 定时兜底：重发名单 ----------------
            Step::Tick => {
                if let Some(v) = state_json(&con) {
                    if ws_tx.send(Message::text(v.to_string())).await.is_err() {
                        break;
                    }
                }
            }

            // ---------------- 浏览器 -> TS3 ----------------
            Step::Ws(msg) => match msg {
                Some(Ok(Message::Binary(bytes))) => {
                    if bytes.len() < 2 {
                        continue;
                    }

                    // 排查"网页说话别人听不见"：把前几帧解码出来看看有没有真实声音
                    if cfg.debug_audio && opus_out_frames < 8 {
                        let dur = opus::packet_duration_ms(&bytes).unwrap_or(0);
                        let head: String = bytes
                            .iter()
                            .take(10)
                            .map(|b| format!("{b:02x}"))
                            .collect::<Vec<_>>()
                            .join(" ");
                        match audiopus::coder::Decoder::new(
                            audiopus::SampleRate::Hz48000,
                            audiopus::Channels::Mono,
                        ) {
                            Ok(mut dec) => {
                                let mut pcm = vec![0f32; 5760];
                                match dec.decode_float(Some(&bytes[..]), &mut pcm, false) {
                                    Ok(n) if n > 0 => {
                                        let rms = (pcm[..n].iter().map(|s| s * s).sum::<f32>()
                                            / n as f32)
                                            .sqrt();
                                        info!(logger, "上行帧诊断";
                                            "len" => bytes.len(), "dur_ms" => dur,
                                            "decoded_samples" => n, "rms" => format!("{rms:.5}"),
                                            "head" => %head);
                                    }
                                    Ok(_) => info!(logger, "上行帧诊断：解出 0 个样本"; "len" => bytes.len()),
                                    Err(e) => info!(logger, "上行帧诊断：解码失败（可能不是合法 Opus）";
                                        "len" => bytes.len(), "dur_ms" => dur, "head" => %head, "error" => %e),
                                }
                            }
                            Err(e) => debug!(logger, "创建解码器失败"; "error" => %e),
                        }
                    }

                    let packet = OutAudio::new(&AudioData::C2S {
                        id: 0,
                        codec: CodecType::OpusVoice,
                        data: &bytes[..],
                    });
                    if let Err(e) = con.send_audio(packet) {
                        debug!(logger, "发送语音到 TS3 失败"; "error" => %e);
                        continue;
                    }
                    opus_out_frames += 1;
                    if opus_out_frames % 500 == 1 {
                        debug!(logger, "网页->TS3 语音帧"; "bytes" => bytes.len());
                    }
                }
                Some(Ok(Message::Text(t))) => {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(t.as_str()) {
                        match v.get("type").and_then(|x| x.as_str()) {
                            Some("ping") => {
                                let _ = ws_tx
                                    .send(Message::text(
                                        serde_json::json!({"type":"pong"}).to_string(),
                                    ))
                                    .await;
                            }
                            Some("move") => {
                                if let Some(cid) = v.get("channelId").and_then(|x| x.as_u64()) {
                                    // clientmove 必须带 clid（自己的客户端 id）
                                    let own_id = con.get_state().ok().map(|b| b.own_client.0);
                                    match own_id {
                                        Some(own_id) => match move_to_channel(&mut con, own_id, cid, &logger) {
                                            Ok(handle) => {
                                                pending_move = Some((handle, cid));
                                                info!(logger, "已请求切换频道"; "clid" => own_id, "channel_id" => cid);
                                            }
                                            Err(e) => {
                                                let _ = ws_tx
                                                    .send(Message::text(
                                                        serde_json::json!({
                                                            "type":"error",
                                                            "message": format!("切换频道失败: {e}")
                                                        })
                                                        .to_string(),
                                                    ))
                                                    .await;
                                            }
                                        },
                                        None => {
                                            let _ = ws_tx
                                                .send(Message::text(
                                                    serde_json::json!({
                                                        "type":"error",
                                                        "message":"还没拿到自己的客户端信息，稍等一下再试"
                                                    })
                                                    .to_string(),
                                                ))
                                                .await;
                                        }
                                    }
                                }
                            }
                            Some("raw") if cfg.allow_raw => {
                                // 排查用：直接发一条 TS3 命令并把应答原样回给调用方
                                if let Some(text) = v.get("cmd").and_then(|x| x.as_str()) {
                                    let cmd = OutCommand::new(
                                        Direction::C2S,
                                        Flags::empty(),
                                        PacketType::Command,
                                        text,
                                    );
                                    match cmd.send_with_result(&mut con) {
                                        Ok(handle) => {
                                            pending_raw = Some((handle, text.to_string()));
                                            info!(logger, "已发送调试命令"; "cmd" => text);
                                        }
                                        Err(e) => {
                                            let _ = ws_tx
                                                .send(Message::text(
                                                    serde_json::json!({
                                                        "type":"raw_result","cmd": text,
                                                        "ok": false, "error": format!("{e}")
                                                    })
                                                    .to_string(),
                                                ))
                                                .await;
                                        }
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
                Some(Ok(Message::Ping(p))) => {
                    let _ = ws_tx.send(Message::Pong(p)).await;
                }
                Some(Ok(Message::Close(frame))) => {
                    let (code, reason) = match &frame {
                        Some(f) => (u16::from(f.code), f.reason.to_string()),
                        None => (0u16, String::new()),
                    };
                    info!(logger, "浏览器关闭了连接";
                        "code" => code, "reason" => reason,
                        "ts3_to_web" => opus_in_frames, "web_to_ts3" => opus_out_frames);
                    break;
                }
                None => {
                    info!(logger, "浏览器断开（没有关闭帧，通常是页面被关掉或网络中断）";
                        "ts3_to_web" => opus_in_frames, "web_to_ts3" => opus_out_frames);
                    break;
                }
                Some(Ok(_)) => {}
                Some(Err(e)) => {
                    debug!(logger, "WebSocket 错误"; "error" => %e);
                    break;
                }
            },
        }
    }

    // ---- 5. 收尾 ----
    let _ = ws_tx.close().await;
    let _ = con.disconnect(tsclientlib::DisconnectOptions::new());
    // 继续轮询一小会儿，让 clientdisconnect 命令真正发出去。
    // 否则连接直接被丢掉，TS3 要等到 ping 超时才清理，日志里会留下「xxx 超时了」。
    {
        let mut events = con.events();
        let deadline = tokio::time::Instant::now() + Duration::from_millis(1500);
        while tokio::time::Instant::now() < deadline {
            match timeout(Duration::from_millis(200), events.next()).await {
                Ok(Some(Ok(_))) | Err(_) => {}
                Ok(Some(Err(_))) | Ok(None) => break,
            }
        }
    }
    info!(logger, "会话清理完成";
        "ts3_to_web" => opus_in_frames, "web_to_ts3" => opus_out_frames);
    Ok(())
}

/// 等 TS3 连接建立：收到第一批 book 事件即认为已连上并进入频道。
async fn wait_connected(con: &mut Connection, logger: &Logger) -> Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let mut events = con.events();

    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            bail!("连接 TS3 超时（15 秒）");
        }
        match timeout(left, events.next()).await {
            Ok(Some(Ok(StreamItem::BookEvents(_)))) => return Ok(()),
            Ok(Some(Ok(StreamItem::IdentityLevelIncreasing(level)))) => {
                info!(logger, "正在提升身份等级"; "level" => level);
            }
            Ok(Some(Ok(StreamItem::IdentityLevelIncreased))) => {
                debug!(logger, "身份等级已提升");
            }
            Ok(Some(Ok(_))) => continue,
            Ok(Some(Err(e))) => return Err(anyhow!("连接 TS3 失败: {e}")),
            Ok(None) => bail!("TS3 连接被关闭"),
            Err(_) => continue,
        }
    }
}

/// 极简 query string 解析（含 %XX 百分号解码，# 之后不处理）。
fn parse_query(q: &str) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    for pair in q.split('&') {
        if pair.is_empty() {
            continue;
        }
        let mut it = pair.splitn(2, '=');
        let k = it.next().unwrap_or("");
        let v = it.next().unwrap_or("");
        if k.is_empty() {
            continue;
        }
        out.insert(k.to_string(), percent_decode(v));
    }
    out
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(b) => {
                        out.push(b);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

/// 给日志用的短随机 id（避免引入 uuid 依赖）。
fn short_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!("{:04x}", (n ^ (std::process::id())) & 0xffff)
}
