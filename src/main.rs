//! ts3web —— 让临时队友用浏览器加入 TS3 语音频道的网关。
//!
//! 架构：
//!   浏览器 (getUserMedia + WebCodecs/AudioEncoder 产出 20ms Opus 帧)
//!        │  wss://<域名>/ws   （由 1Panel 的 OpenResty 反代到本进程）
//!        ▼
//!   ts3web（本程序，单任务 per 会话）
//!        │  tsclientlib 实现的 TS3 客户端协议（UDP 9987）
//!        ▼
//!   TeamSpeak 3 服务器（TS3 服务端自己负责混音，本程序不做混音）
//!
//! 每个网页连接 = 一个独立的 TS3 身份，因此 N 个网页用户就是 N 个轻量客户端。

mod http;
mod opus;
mod selftest;
mod session;

use std::cell::Cell;
use std::env;
use std::net::SocketAddr;
use std::rc::Rc;

use anyhow::{bail, Context, Result};
use slog::{o, Drain, Logger};
use tokio::net::TcpListener;
use tokio::task::LocalSet;

/// 运行期配置，全部来自环境变量（见 docker-compose.yml）。
#[derive(Debug, Clone)]
pub struct Config {
    /// 监听地址。默认只监听容器内的 18080，由 OpenResty 反代过来。
    pub bind: SocketAddr,
    /// TS3 服务器地址，形如 1Panel-teamspeak-nn9k:9987 或 10.0.0.123:9987。
    pub ts3_addr: String,
    /// 进入哪个频道（按频道路径/名称）。留空则进默认频道。
    pub ts3_channel: Option<String>,
    pub ts3_channel_password: Option<String>,
    pub ts3_server_password: Option<String>,
    /// 网页访问口令：URL 里带 ?t=<token> 才能进，防止链接被到处乱传。
    pub token: String,
    /// 给网页用户在 TS3 里的昵称加前缀，方便一眼分辨。
    pub nick_prefix: String,
    /// 最多同时几个网页用户。
    pub max_sessions: usize,
    /// 等客户端发 hello 的超时（秒）。
    pub hello_timeout_secs: u64,
    pub log_level: String,
    /// 设了就用自检模式跑（值 = 发测试音的秒数），不启动网页服务。
    pub selftest_seconds: Option<u64>,
    /// 允许通过网页发任意 TS3 命令（排查用，默认关闭）
    pub allow_raw: bool,
    /// 把浏览器上行的前几帧解码出来算音量，用于排查"听不见"（默认关闭）
    pub debug_audio: bool,
}

fn env_opt(key: &str) -> Option<String> {
    match env::var(key) {
        Ok(v) if !v.trim().is_empty() => Some(v),
        _ => None,
    }
}

fn env_or(key: &str, default: &str) -> String {
    env_opt(key).unwrap_or_else(|| default.to_string())
}

impl Config {
    fn from_env() -> Result<Self> {
        let bind: SocketAddr = env_or("TS3WEB_BIND", "0.0.0.0:18080")
            .parse()
            .context("TS3WEB_BIND 不是合法地址")?;

        let ts3_addr = match env_opt("TS3WEB_ADDR") {
            Some(v) => v,
            None => bail!("必须设置 TS3WEB_ADDR，例如 TS3WEB_ADDR=1Panel-teamspeak-nn9k:9987"),
        };

        let token = env_opt("TS3WEB_TOKEN").unwrap_or_default();

        let max_sessions = env_or("TS3WEB_MAX_SESSIONS", "10")
            .parse()
            .context("TS3WEB_MAX_SESSIONS 不是数字")?;

        let hello_timeout_secs = env_or("TS3WEB_HELLO_TIMEOUT", "10")
            .parse()
            .context("TS3WEB_HELLO_TIMEOUT 不是数字")?;

        Ok(Self {
            bind,
            ts3_addr,
            ts3_channel: env_opt("TS3WEB_CHANNEL"),
            ts3_channel_password: env_opt("TS3WEB_CHANNEL_PASSWORD"),
            ts3_server_password: env_opt("TS3WEB_SERVER_PASSWORD"),
            token,
            nick_prefix: env_or("TS3WEB_NICK_PREFIX", "网页-"),
            max_sessions,
            hello_timeout_secs,
            log_level: env_or("TS3WEB_LOG", "info"),
            selftest_seconds: env_opt("TS3WEB_SELFTEST").and_then(|v| v.parse().ok()),
            allow_raw: env_opt("TS3WEB_ALLOW_RAW").map(|v| v == "1" || v == "true").unwrap_or(false),
            debug_audio: env_opt("TS3WEB_DEBUG_AUDIO")
                .map(|v| v == "1" || v == "true")
                .unwrap_or(false),
        })
    }

    /// 口令校验：未配置口令时放行（仅建议内网/测试用）。
    pub fn token_ok(&self, given: Option<&str>) -> bool {
        if self.token.is_empty() {
            return true;
        }
        given.map(|g| g == self.token).unwrap_or(false)
    }

    /// 组装 TS3 里显示的昵称。
    pub fn display_nick(&self, raw: &str) -> String {
        let cleaned: String = raw
            .chars()
            .filter(|c| !c.is_control())
            .take(24)
            .collect::<String>()
            .trim()
            .to_string();
        if cleaned.is_empty() {
            format!("{}访客", self.nick_prefix)
        } else {
            format!("{}{}", self.nick_prefix, cleaned)
        }
    }
}

fn parse_level(s: &str) -> slog::Level {
    match s.to_ascii_lowercase().as_str() {
        "trace" => slog::Level::Trace,
        "debug" => slog::Level::Debug,
        "warn" | "warning" => slog::Level::Warning,
        "error" => slog::Level::Error,
        "critical" => slog::Level::Critical,
        _ => slog::Level::Info,
    }
}

fn build_logger(level: &str) -> Logger {
    let decorator = slog_term::TermDecorator::new().build();
    let drain = slog_term::CompactFormat::new(decorator).build().fuse();
    let drain = slog_async::Async::new(drain).build().fuse();
    let drain = slog::LevelFilter::new(drain, parse_level(level)).fuse();
    Logger::root(drain, o!())
}

fn main() -> Result<()> {
    let cfg = Rc::new(Config::from_env()?);
    let logger = build_logger(&cfg.log_level);

    slog::info!(logger, "ts3web 启动";
        "bind" => %cfg.bind,
        "ts3" => %cfg.ts3_addr,
        "channel" => cfg.ts3_channel.clone().unwrap_or_else(|| "(默认)".into()),
        "token" => if cfg.token.is_empty() { "未设置（任何人可连）" } else { "已设置" },
    );

    // tsclientlib 的 Connection 不保证 Send，而且全部逻辑都在单个任务里轮询，
    // 所以用 current_thread 运行时 + LocalSet，会话用 spawn_local 起。
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("创建 tokio 运行时失败")?;
    let local = LocalSet::new();

    rt.block_on(local.run_until(async move {
        match cfg.selftest_seconds {
            Some(secs) => selftest::run(cfg, logger, secs).await,
            None => serve(cfg, logger).await,
        }
    }))
}

async fn serve(cfg: Rc<Config>, logger: Logger) -> Result<()> {
    let listener = TcpListener::bind(cfg.bind)
        .await
        .with_context(|| format!("监听 {} 失败", cfg.bind))?;
    slog::info!(logger, "开始接受连接"; "addr" => %cfg.bind);

    let active = Rc::new(Cell::new(0usize));

    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(v) => v,
            Err(e) => {
                slog::warn!(logger, "accept 失败"; "error" => %e);
                continue;
            }
        };

        if active.get() >= cfg.max_sessions {
            slog::warn!(logger, "超过并发上限，拒绝连接"; "peer" => %peer, "max" => cfg.max_sessions);
            // 直接丢弃：让浏览器侧看到连接被关闭
            drop(stream);
            continue;
        }

        let cfg_task = cfg.clone();
        let logger_task = logger.clone();
        let active_task = active.clone();
        active.set(active.get() + 1);

        tokio::task::spawn_local(async move {
            let peer_str = peer.to_string();
            if let Err(e) = http::handle_stream(stream, cfg_task, logger_task.clone()).await {
                slog::info!(logger_task, "会话结束"; "peer" => %peer_str, "reason" => %e);
            } else {
                slog::info!(logger_task, "会话正常结束"; "peer" => %peer_str);
            }
            active_task.set(active_task.get().saturating_sub(1));
        });
    }
}
