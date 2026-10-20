//! 网关 ↔ agent 的**本体链路**：回环 TCP 或本地管道（Windows 命名管道 / Unix domain socket）。
//!
//! 三种形态跑的是**同一套** WS 帧与协议：`client_async` 只要求一个
//! `AsyncRead + AsyncWrite`，换的只是"字节从哪来"。
//!
//! 端点字符串是**唯一**表达链路形态的地方（注册表里存的也是它）：
//!
//! | 形态 | 端点 | 谁产生 |
//! |---|---|---|
//! | 回环 TCP | `ws://127.0.0.1:<port>/rpc?token=…` | 网关 spawn（默认）、agent `--gateway` 自注册 |
//! | Windows 命名管道 | `pipe://\\.\pipe\a-da-agent-<id>?token=…` | 网关 spawn（`A_DA_GATEWAY_AGENT_LINK=pipe`） |
//! | Unix domain socket | `unix:///tmp/a-da-agent-<id>.sock?token=…` | 同上（Unix） |
//!
//! 为什么还要带 token：管道已经是本机可达，鉴权其实可以省；但**两种形态共用一条代码路径**
//! 比"管道特判免鉴权"更不容易在将来出洞——agent 侧的握手回调因此一字不改。

use tokio::io::{AsyncRead, AsyncWrite};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::Request;
use tokio_tungstenite::WebSocketStream;

/// 把 TCP / 命名管道 / Unix socket 统一成一个字节流类型。
pub trait AsyncReadWrite: AsyncRead + AsyncWrite {}
impl<T: AsyncRead + AsyncWrite> AsyncReadWrite for T {}

/// 装箱后的双向字节流（WS 客户端只需要它）。
pub type BoxedIo = Box<dyn AsyncReadWrite + Unpin + Send>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentEndpoint {
    /// 回环 TCP。
    Tcp {
        host: String,
        port: u16,
        /// 含前导 `?`；空串表示没有 query。
        query: String,
    },
    /// 本地管道：Windows 命名管道名或 Unix socket 路径。
    Pipe { name: String, query: String },
}

impl AgentEndpoint {
    /// 解析注册表里的端点字符串。
    pub fn parse(endpoint: &str) -> Result<Self, String> {
        if let Some(rest) = endpoint.strip_prefix("pipe://") {
            let (name, query) = split_query(rest);
            return Ok(Self::Pipe {
                name: name.to_string(),
                query,
            });
        }
        if let Some(rest) = endpoint.strip_prefix("unix://") {
            let (path, query) = split_query(rest);
            return Ok(Self::Pipe {
                name: path.to_string(),
                query,
            });
        }

        let rest = endpoint
            .strip_prefix("ws://")
            .ok_or_else(|| format!("不认识的 agent 端点：{endpoint}"))?;
        let (authority, query) = split_query(rest);
        // `host:port/rpc` 与 `host:port` 都认（路径对握手无意义）
        let authority = authority.split('/').next().unwrap_or(authority);
        let (host, port) = authority
            .rsplit_once(':')
            .ok_or_else(|| format!("agent 端点缺少端口：{endpoint}"))?;
        let port: u16 = port
            .parse()
            .map_err(|_| format!("agent 端点端口非法：{endpoint}"))?;
        Ok(Self::Tcp {
            host: host.to_string(),
            port,
            query,
        })
    }

    /// 建立 WS 连接（握手 + 帧，与直接 `connect_async` 等价）。
    pub async fn connect(&self) -> Result<WebSocketStream<BoxedIo>, anyhow::Error> {
        let (query, io): (String, BoxedIo) = match self {
            Self::Tcp { host, port, query } => {
                let stream = tokio::net::TcpStream::connect(format!("{host}:{port}")).await?;
                (query.clone(), Box::new(stream))
            }
            Self::Pipe { name, query } => (query.clone(), open_pipe(name).await?),
        };

        // 握手用的 URI 只承载 query（token / api=1），host 段随意——
        // agent 的回调只读 `uri.query()`，不校验 path 与 Host。
        let req: Request<()> = format!("ws://a-da-agent.local/rpc{query}").into_client_request()?;
        let (ws, _resp) = tokio_tungstenite::client_async(req, io).await?;
        Ok(ws)
    }
}

fn split_query(s: &str) -> (&str, String) {
    match s.split_once('?') {
        Some((head, tail)) => (head, format!("?{tail}")),
        None => (s, String::new()),
    }
}

#[cfg(windows)]
async fn open_pipe(name: &str) -> Result<BoxedIo, anyhow::Error> {
    use tokio::net::windows::named_pipe::ClientOptions;
    // `open` 是同步的 CreateFile；管道连接本身不阻塞等待（服务端已先建好实例）。
    let client = ClientOptions::new().open(name)?;
    Ok(Box::new(client))
}

#[cfg(unix)]
async fn open_pipe(path: &str) -> Result<BoxedIo, anyhow::Error> {
    let stream = tokio::net::UnixStream::connect(path).await?;
    Ok(Box::new(stream))
}

#[cfg(not(any(unix, windows)))]
async fn open_pipe(_path: &str) -> Result<BoxedIo, anyhow::Error> {
    anyhow::bail!("当前平台不支持本地管道链路")
}

/// 把端点字符串当"是否管道"来判（管理面展示与诊断用）。
pub fn is_pipe_endpoint(endpoint: &str) -> bool {
    endpoint.starts_with("pipe://") || endpoint.starts_with("unix://")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_loopback_tcp_endpoint() {
        let e = AgentEndpoint::parse("ws://127.0.0.1:52353/rpc?token=abc").unwrap();
        assert_eq!(
            e,
            AgentEndpoint::Tcp {
                host: "127.0.0.1".into(),
                port: 52353,
                query: "?token=abc".into()
            }
        );
    }

    #[test]
    fn parses_windows_pipe_endpoint_with_backslashes() {
        let e = AgentEndpoint::parse(r"pipe://\\.\pipe\a-da-agent-1?token=abc").unwrap();
        assert_eq!(e, AgentEndpoint::Pipe { name: r"\\.\pipe\a-da-agent-1".into(), query: "?token=abc".into() });
    }

    #[test]
    fn parses_unix_socket_endpoint() {
        let e = AgentEndpoint::parse("unix:///tmp/a-da-agent-1.sock?token=abc").unwrap();
        assert_eq!(e, AgentEndpoint::Pipe { name: "/tmp/a-da-agent-1.sock".into(), query: "?token=abc".into() });
    }

    #[test]
    fn rejects_endpoint_without_scheme() {
        assert!(AgentEndpoint::parse("127.0.0.1:1/rpc").is_err());
        assert!(AgentEndpoint::parse("ws://127.0.0.1/rpc").is_err());
    }
}
