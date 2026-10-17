//! 极简 HTTP 层：因为脚本①(add-https-service.sh)会把整个端口反代到一个后端，
//! 所以网页也得由本服务自己发出去（页面直接编译进二进制，不用额外挂文件）。
//!
//! 只处理三件事：
//!   GET /  /index.html  -> 网页
//!   GET /healthz        -> ok（给监控/自测用）
//!   带 Upgrade: websocket 的请求 -> 交给 WebSocket 会话处理
//!
//! 不引入 axum/hyper 之类的依赖，是因为这台 NAS 只有 1 个核，
//! 编译依赖越少，改一次代码等的就越短。

use std::rc::Rc;

use anyhow::{bail, Result};
use slog::{debug, Logger};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::{Duration, Instant};

use crate::{session, Config};

/// 网页内容直接编译进二进制
const INDEX_HTML: &str = include_str!("../web/index.html");

/// 导航页图片资源在容器里的挂载点（docker-compose 里从 NAS 只读挂进来）。
/// 网页用相对路径 assets/... 引用，于是内网/外网访问都能出图，不依赖 home.inory.xin。
const ASSET_DIR: &str = "/assets";

/// 只放行图片类资源，并挡掉路径穿越
fn safe_asset_path(name: &str) -> Option<std::path::PathBuf> {
    if name.is_empty() || name.len() > 160 || name.contains("..") || name.contains('\\') {
        return None;
    }
    let ok = name.split('/').all(|seg| {
        !seg.is_empty()
            && !seg.starts_with('.')
            && seg
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
    });
    if !ok {
        return None;
    }
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    if !matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "webp" | "gif" | "svg" | "ico") {
        return None;
    }
    Some(std::path::Path::new(ASSET_DIR).join(name))
}

fn content_type_for(name: &str) -> &'static str {
    match name.rsplit('.').next().unwrap_or("").to_ascii_lowercase().as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        _ => "application/octet-stream",
    }
}

pub async fn handle_stream(mut stream: TcpStream, cfg: Rc<Config>, logger: Logger) -> Result<()> {
    // 先偷看请求头，判断这是 WebSocket 升级还是普通 HTTP 请求。
    // peek 不会消耗数据，所以后面 WebSocket 握手还能正常读到完整请求。
    let mut buf = vec![0u8; 4096];
    let start = Instant::now();
    let n = loop {
        let k = stream.peek(&mut buf).await?;
        if k == 0 {
            bail!("连接在发送请求前就关闭了");
        }
        let head_done = buf[..k].windows(4).any(|w| w == b"\r\n\r\n".as_slice());
        if head_done || k >= buf.len() || start.elapsed() > Duration::from_secs(5) {
            break k;
        }
        // 头部还没收全，等一会儿再看
        tokio::time::sleep(Duration::from_millis(20)).await;
    };

    let head = String::from_utf8_lossy(&buf[..n]).to_string();
    if head.to_ascii_lowercase().contains("upgrade: websocket") {
        // WebSocket 路径：不能把请求读掉，交给握手去消费
        return session::handle(stream, cfg, logger).await;
    }

    // 普通 HTTP：把刚才 peek 到的请求字节真正读掉。
    // 这一点很关键 —— 如果关连接时接收队列里还有未读数据，Linux 会发 RST 而不是 FIN，
    // 客户端就会丢掉还没收完的响应体（表现为页面被截断）。
    let mut discard = vec![0u8; n];
    let _ = stream.read_exact(&mut discard).await;

    serve_http(stream, &head, &logger).await
}

async fn serve_http(mut stream: TcpStream, head: &str, logger: &Logger) -> Result<()> {
    let first_line = head.lines().next().unwrap_or("");
    let mut parts = first_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("/");
    let path = target.split('?').next().unwrap_or("/");

    debug!(logger, "HTTP 请求"; "method" => method, "path" => path);

    let (status, ctype, body): (&str, &str, &[u8]) = match (method, path) {
        ("GET", "/") | ("GET", "/index.html") | ("GET", "/voice") | ("GET", "/voice/") => {
            ("200 OK", "text/html; charset=utf-8", INDEX_HTML.as_bytes())
        }
        ("GET", "/healthz") => ("200 OK", "text/plain; charset=utf-8", b"ok" as &[u8]),
        _ => ("404 Not Found", "text/plain; charset=utf-8", b"not found" as &[u8]),
    };

    // 图片资源（logo / 立绘 / Q 版看板娘）：从挂载进来的只读目录直接读，
    // 相对路径引用，所以内网外网都能显示
    if method == "GET" {
        if let Some(name) = path.strip_prefix("/assets/") {
            return match safe_asset_path(name) {
                Some(full) => match tokio::fs::read(&full).await {
                    Ok(bytes) => {
                        debug!(logger, "提供静态资源"; "name" => name, "bytes" => bytes.len());
                        send_bytes(
                            &mut stream,
                            "200 OK",
                            content_type_for(name),
                            &bytes,
                            "public, max-age=86400",
                        )
                        .await
                    }
                    Err(e) => {
                        debug!(logger, "资源读取失败"; "name" => name, "error" => %e);
                        send_bytes(&mut stream, "404 Not Found", "text/plain; charset=utf-8", b"not found", "no-store").await
                    }
                },
                None => {
                    send_bytes(&mut stream, "404 Not Found", "text/plain; charset=utf-8", b"not found", "no-store").await
                }
            };
        }
    }

    send_bytes(&mut stream, status, ctype, body, "no-store").await
}

/// 把一段字节按 HTTP/1.1 发出去（写完关连接）
async fn send_bytes(
    stream: &mut TcpStream, status: &str, ctype: &str, body: &[u8], cache: &str,
) -> Result<()> {
    let header = format!(
        "HTTP/1.1 {status}\r\n\
         Content-Type: {ctype}\r\n\
         Content-Length: {}\r\n\
         Cache-Control: {cache}\r\n\
         Connection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes()).await?;
    stream.write_all(body).await?;
    stream.flush().await?;
    let _ = stream.shutdown().await;
    Ok(())
}
