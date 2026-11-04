//! OpenAI 兼容接口的 embedding 客户端。
//!
//! `POST {endpoint}/embeddings`，密钥从 `api_key_env` 指定的环境变量读取，只放在
//! `Authorization` 头中，不写入日志或错误信息。HTTP 状态码由本模块自行映射为
//! [`EmbedError`]，因此关闭了 ureq 默认的状态码错误。代理设置被显式关闭，避免
//! 环境变量中的代理改变请求去向。

use std::time::Duration;

use serde::Deserialize;
use tracing::debug;
use ureq::Agent;

use super::{EmbedError, Embedder};
use crate::config::EmbeddingConfig;

/// OpenAI 兼容接口客户端。
///
/// `Debug` 实现不输出密钥。
pub struct OpenAiEmbedder {
    endpoint: String,
    model: String,
    dimensions: usize,
    timeout: Duration,
    api_key: Option<String>,
    api_key_env: String,
    agent: Agent,
}

impl std::fmt::Debug for OpenAiEmbedder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiEmbedder")
            .field("endpoint", &self.endpoint)
            .field("model", &self.model)
            .field("dimensions", &self.dimensions)
            .field("timeout", &self.timeout)
            .field("api_key", &self.api_key.as_ref().map(|_| "<set>"))
            .finish()
    }
}

impl OpenAiEmbedder {
    /// 从配置构造。环境变量未设置或为空时返回 [`EmbedError::Auth`]，错误信息只包含变量名。
    pub fn from_config(cfg: &EmbeddingConfig) -> Result<Self, EmbedError> {
        let api_key = crate::config::embedding_api_key(cfg)
            .ok_or_else(|| EmbedError::Auth(cfg.api_key_env.clone()))?;
        Ok(Self::new(
            &cfg.endpoint,
            &cfg.model,
            cfg.dimensions,
            Duration::from_millis(cfg.timeout_ms),
            Some(api_key),
            &cfg.api_key_env,
        ))
    }

    /// 直接构造，供测试使用。`api_key` 为 `None` 时请求不带鉴权头。
    pub(crate) fn new(
        endpoint: &str,
        model: &str,
        dimensions: usize,
        timeout: Duration,
        api_key: Option<String>,
        api_key_env: &str,
    ) -> Self {
        let config = Agent::config_builder()
            .timeout_global(Some(timeout))
            .http_status_as_error(false)
            .proxy(None)
            .build();
        Self {
            endpoint: endpoint.trim_end_matches('/').to_owned(),
            model: model.to_owned(),
            dimensions,
            timeout,
            api_key,
            api_key_env: api_key_env.to_owned(),
            agent: Agent::new_with_config(config),
        }
    }

    /// 超时的毫秒数，用于错误信息。
    fn timeout_ms(&self) -> u64 {
        u64::try_from(self.timeout.as_millis()).unwrap_or(u64::MAX)
    }

    /// 把 ureq 的传输错误映射为 [`EmbedError`]。
    fn map_transport(&self, err: ureq::Error) -> EmbedError {
        match err {
            ureq::Error::Timeout(_) => EmbedError::Timeout(self.timeout_ms()),
            ureq::Error::Protocol(e) => EmbedError::BadResponse(e.to_string()),
            ureq::Error::Json(e) => EmbedError::BadResponse(e.to_string()),
            other => EmbedError::Unreachable(other.to_string()),
        }
    }
}

impl Embedder for OpenAiEmbedder {
    fn model(&self) -> &str {
        &self.model
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }

    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let url = format!("{}/embeddings", self.endpoint);
        let body = serde_json::json!({
            "model": self.model,
            "input": texts,
            "encoding_format": "float",
        });
        debug!(
            model = %self.model,
            count = texts.len(),
            endpoint = %self.endpoint,
            "embedding request"
        );

        let mut request = self
            .agent
            .post(&url)
            .header("Content-Type", "application/json");
        if let Some(key) = &self.api_key {
            request = request.header("Authorization", format!("Bearer {key}"));
        }
        let mut response = request
            .send_json(&body)
            .map_err(|e| self.map_transport(e))?;

        let status = response.status().as_u16();
        let bytes = response
            .body_mut()
            .read_to_vec()
            .map_err(|e| self.map_transport(e))?;
        debug!(status, bytes = bytes.len(), "embedding response");

        match status {
            200..=299 => {}
            401 | 403 => return Err(EmbedError::Auth(self.api_key_env.clone())),
            _ => {
                return Err(EmbedError::Unreachable(format!("http status {status}")));
            }
        }

        let vectors = parse_embeddings_response(&bytes, texts.len())?;
        for (i, v) in vectors.iter().enumerate() {
            if v.len() != self.dimensions {
                return Err(EmbedError::BadResponse(format!(
                    "item {i} has {} dimensions, expected {}",
                    v.len(),
                    self.dimensions
                )));
            }
        }
        Ok(vectors)
    }
}

/// OpenAI 响应体的结构。只取需要的字段。
#[derive(Deserialize)]
struct EmbeddingsResponse {
    data: Vec<EmbeddingItem>,
}

/// 单条向量，`index` 对应输入中的位置。
#[derive(Deserialize)]
struct EmbeddingItem {
    index: usize,
    embedding: Vec<f32>,
}

/// 纯函数：解析 `/embeddings` 响应体。
///
/// 返回的向量按输入顺序排列。条数不等于 `expected`、索引缺失或重复、
/// JSON 不合法时返回 [`EmbedError::BadResponse`]。维度检查由调用方完成。
pub fn parse_embeddings_response(
    body: &[u8],
    expected: usize,
) -> Result<Vec<Vec<f32>>, EmbedError> {
    let parsed: EmbeddingsResponse =
        serde_json::from_slice(body).map_err(|e| EmbedError::BadResponse(format!("json: {e}")))?;
    if parsed.data.len() != expected {
        return Err(EmbedError::BadResponse(format!(
            "got {} embeddings, expected {expected}",
            parsed.data.len()
        )));
    }
    let mut slots: Vec<Option<Vec<f32>>> = vec![None; expected];
    for item in parsed.data {
        if item.index >= expected {
            return Err(EmbedError::BadResponse(format!(
                "index {} out of range",
                item.index
            )));
        }
        if slots[item.index].is_some() {
            return Err(EmbedError::BadResponse(format!(
                "duplicate index {}",
                item.index
            )));
        }
        slots[item.index] = Some(item.embedding);
    }
    slots
        .into_iter()
        .enumerate()
        .map(|(i, slot)| slot.ok_or_else(|| EmbedError::BadResponse(format!("missing index {i}"))))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;
    use std::time::Duration;

    use super::{EmbedError, Embedder, OpenAiEmbedder, parse_embeddings_response};

    /// 起一个只接受一次连接的本地服务，返回固定状态与响应体。
    /// 线程返回收到的原始请求文本，便于检查鉴权头与请求体。
    fn serve_once(status: u16, body: &'static str) -> (String, thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let request = read_request(&mut stream);
            let response = format!(
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).unwrap();
            request
        });
        (format!("http://{addr}/v1"), handle)
    }

    /// 读取一个带 Content-Length 的 HTTP 请求。
    fn read_request(stream: &mut std::net::TcpStream) -> String {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 1024];
        loop {
            let n = stream.read(&mut chunk).unwrap();
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
            if let Some(pos) = find(&buf, b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&buf[..pos]).to_ascii_lowercase();
                let len = head
                    .lines()
                    .find_map(|l| l.strip_prefix("content-length:"))
                    .and_then(|v| v.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                if buf.len() >= pos + 4 + len {
                    break;
                }
            }
        }
        String::from_utf8_lossy(&buf).into_owned()
    }

    fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
        hay.windows(needle.len()).position(|w| w == needle)
    }

    fn client(endpoint: &str, key: Option<&str>) -> OpenAiEmbedder {
        OpenAiEmbedder::new(
            endpoint,
            "BAAI/bge-m3",
            2,
            Duration::from_secs(5),
            key.map(str::to_owned),
            "TEST_KEY_ENV",
        )
    }

    fn texts(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn parses_out_of_order_items_in_input_order() {
        let body =
            br#"{"data":[{"index":1,"embedding":[0.0,1.0]},{"index":0,"embedding":[1.0,0.0]}]}"#;
        let vectors = parse_embeddings_response(body, 2).unwrap();
        assert_eq!(vectors, vec![vec![1.0, 0.0], vec![0.0, 1.0]]);
    }

    #[test]
    fn count_mismatch_is_bad_response() {
        let body = br#"{"data":[{"index":0,"embedding":[1.0]}]}"#;
        assert!(matches!(
            parse_embeddings_response(body, 2),
            Err(EmbedError::BadResponse(_))
        ));
    }

    #[test]
    fn duplicate_or_out_of_range_index_is_bad_response() {
        let dup = br#"{"data":[{"index":0,"embedding":[1.0]},{"index":0,"embedding":[1.0]}]}"#;
        assert!(matches!(
            parse_embeddings_response(dup, 2),
            Err(EmbedError::BadResponse(_))
        ));
        let far = br#"{"data":[{"index":5,"embedding":[1.0]}]}"#;
        assert!(matches!(
            parse_embeddings_response(far, 1),
            Err(EmbedError::BadResponse(_))
        ));
    }

    #[test]
    fn malformed_json_is_bad_response() {
        assert!(matches!(
            parse_embeddings_response(b"not json", 1),
            Err(EmbedError::BadResponse(_))
        ));
    }

    #[test]
    fn success_sends_bearer_key_and_model_and_returns_vectors() {
        let (endpoint, server) = serve_once(
            200,
            r#"{"data":[{"index":0,"embedding":[1.0,0.0]},{"index":1,"embedding":[0.0,1.0]}]}"#,
        );
        let vectors = client(&endpoint, Some("sk-test-123"))
            .embed(&texts(&["git pull", "git clone"]))
            .unwrap();
        assert_eq!(vectors.len(), 2);
        let request = server.join().unwrap();
        assert!(request.starts_with("POST /v1/embeddings "));
        assert!(request.contains("authorization: Bearer sk-test-123"));
        // 请求体可能带空白，解析后比较字段，不依赖格式。
        let body = request
            .split("\r\n\r\n")
            .nth(1)
            .unwrap_or("")
            .trim_start_matches(|c| c != '{');
        let json: serde_json::Value = serde_json::from_str(body)
            .unwrap_or_else(|e| panic!("body is not json ({e}): {request}"));
        assert_eq!(json["model"], "BAAI/bge-m3");
        assert_eq!(json["input"], serde_json::json!(["git pull", "git clone"]));
    }

    #[test]
    fn no_key_sends_no_authorization_header() {
        let (endpoint, server) = serve_once(200, r#"{"data":[{"index":0,"embedding":[1.0,0.0]}]}"#);
        client(&endpoint, None).embed(&texts(&["x"])).unwrap();
        let request = server.join().unwrap();
        assert!(!request.to_ascii_lowercase().contains("authorization:"));
    }

    #[test]
    fn unauthorized_is_auth_error_naming_the_env_var_only() {
        let (endpoint, server) = serve_once(401, r#"{"error":"bad key"}"#);
        let err = client(&endpoint, Some("sk-secret-value"))
            .embed(&texts(&["x"]))
            .unwrap_err();
        assert_eq!(err, EmbedError::Auth("TEST_KEY_ENV".to_owned()));
        assert!(!format!("{err}").contains("sk-secret-value"));
        server.join().unwrap();
    }

    #[test]
    fn server_error_is_unreachable() {
        let (endpoint, server) = serve_once(500, r#"{"error":"boom"}"#);
        let err = client(&endpoint, None).embed(&texts(&["x"])).unwrap_err();
        assert!(matches!(err, EmbedError::Unreachable(_)));
        server.join().unwrap();
    }

    #[test]
    fn dimension_mismatch_is_bad_response() {
        // 客户端期望 2 维，服务返回 3 维。
        let (endpoint, server) =
            serve_once(200, r#"{"data":[{"index":0,"embedding":[1.0,0.0,0.0]}]}"#);
        let err = client(&endpoint, None).embed(&texts(&["x"])).unwrap_err();
        assert!(matches!(err, EmbedError::BadResponse(_)));
        server.join().unwrap();
    }

    #[test]
    fn malformed_body_over_http_is_bad_response() {
        let (endpoint, server) = serve_once(200, "<html>oops</html>");
        let err = client(&endpoint, None).embed(&texts(&["x"])).unwrap_err();
        assert!(matches!(err, EmbedError::BadResponse(_)));
        server.join().unwrap();
    }

    #[test]
    fn refused_connection_is_unreachable() {
        // 绑定后立即释放端口，连接会被拒绝。
        let port = {
            let l = TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let err = client(&format!("http://127.0.0.1:{port}/v1"), None)
            .embed(&texts(&["x"]))
            .unwrap_err();
        assert!(matches!(err, EmbedError::Unreachable(_)), "got {err:?}");
    }

    #[test]
    fn slow_server_times_out() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        // 接受连接后不回应，保持到客户端超时。
        let server = thread::spawn(move || {
            let (_stream, _) = listener.accept().unwrap();
            thread::sleep(Duration::from_secs(2));
        });
        let embedder = OpenAiEmbedder::new(
            &format!("http://{addr}/v1"),
            "m",
            2,
            Duration::from_millis(200),
            None,
            "TEST_KEY_ENV",
        );
        let err = embedder.embed(&texts(&["x"])).unwrap_err();
        assert_eq!(err, EmbedError::Timeout(200));
        server.join().unwrap();
    }

    #[test]
    fn debug_output_never_shows_the_key() {
        let embedder = client("http://127.0.0.1:9/v1", Some("sk-very-secret"));
        let shown = format!("{embedder:?}");
        assert!(!shown.contains("sk-very-secret"));
        assert!(shown.contains("<set>"));
    }

    #[test]
    fn empty_input_makes_no_request() {
        // 端口 9 没有服务，若发出请求会返回 Unreachable。
        let vectors = client("http://127.0.0.1:9/v1", None).embed(&[]).unwrap();
        assert!(vectors.is_empty());
    }
}
