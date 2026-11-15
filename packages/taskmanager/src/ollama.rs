//! The AI models Ollama has loaded, from its local API (GET /api/ps on the loopback address only). Asked only while a
//! graphics card's page is open; when Ollama does not answer, nothing is shown.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

/// Ollama's own default address; never anything but this machine.
const ADDR: ([u8; 4], u16) = ([127, 0, 0, 1], 11434);

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Model {
    pub name: String,
    /// Bytes in all, and in the graphics card's memory.
    pub size: u64,
    pub size_vram: u64,
    pub params: String,
    pub quant: String,
    pub context: Option<u64>,
    /// When Ollama unloads it if nothing asks (as Ollama writes it, RFC 3339).
    pub expires: String,
}

impl Model {
    /// The share of the model in the graphics card's memory, 0-100.
    pub fn on_gpu(&self) -> f32 {
        if self.size == 0 {
            0.0
        } else {
            (self.size_vram as f64 / self.size as f64 * 100.0).min(100.0) as f32
        }
    }
}

pub fn parse_ps(json: &str) -> Option<Vec<Model>> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let arr = v.get("models")?.as_array()?;
    Some(
        arr.iter()
            .map(|m| {
                let s = |k: &str| m.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
                let d = |k: &str| m.get("details").and_then(|x| x.get(k)).and_then(|x| x.as_str()).unwrap_or("").to_string();
                Model {
                    name: if s("name").is_empty() { s("model") } else { s("name") },
                    size: m.get("size").and_then(|x| x.as_u64()).unwrap_or(0),
                    size_vram: m.get("size_vram").and_then(|x| x.as_u64()).unwrap_or(0),
                    params: d("parameter_size"),
                    quant: d("quantization_level"),
                    context: m.get("context_length").and_then(|x| x.as_u64()).filter(|&n| n > 0),
                    expires: s("expires_at"),
                }
            })
            .collect(),
    )
}

/// The body of an HTTP/1.0 answer, when the status is 200.
pub fn http_body(resp: &[u8]) -> Option<&str> {
    let text = std::str::from_utf8(resp).ok()?;
    let (head, body) = text.split_once("\r\n\r\n")?;
    let status = head.lines().next()?.split_whitespace().nth(1)?;
    (status == "200").then_some(body)
}

/// The loaded models; None when Ollama is not running (or answers something else).
pub fn loaded() -> Option<Vec<Model>> {
    let addr = SocketAddr::from(ADDR);
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_millis(150)).ok()?;
    s.set_read_timeout(Some(Duration::from_millis(500))).ok()?;
    s.set_write_timeout(Some(Duration::from_millis(500))).ok()?;
    // HTTP/1.0: the answer comes whole (no chunks) and the connection closes after it
    s.write_all(b"GET /api/ps HTTP/1.0\r\nHost: 127.0.0.1\r\nAccept: application/json\r\n\r\n").ok()?;
    let mut buf = Vec::with_capacity(4096);
    s.take(1 << 20).read_to_end(&mut buf).ok()?;
    parse_ps(http_body(&buf)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ps_answer() {
        let body = r#"{"models":[{"name":"qwen3:8b","model":"qwen3:8b","size":6848000000,"digest":"x","details":{"format":"gguf","family":"qwen3","parameter_size":"8.2B","quantization_level":"Q4_K_M"},"expires_at":"2026-11-15T15:04:05.1+01:00","size_vram":6848000000,"context_length":16384},{"model":"tiny","size":1000,"size_vram":250}]}"#;
        let v = parse_ps(body).unwrap();
        assert_eq!(v.len(), 2);
        assert_eq!(
            (v[0].name.as_str(), v[0].params.as_str(), v[0].quant.as_str(), v[0].context),
            ("qwen3:8b", "8.2B", "Q4_K_M", Some(16384))
        );
        assert_eq!(v[0].on_gpu(), 100.0);
        assert_eq!((v[1].name.as_str(), v[1].on_gpu(), v[1].context), ("tiny", 25.0, None));
        assert_eq!(parse_ps(r#"{"models":[]}"#), Some(vec![]));
        assert_eq!(parse_ps("not json"), None);
    }

    #[test]
    fn http_answers() {
        let ok = b"HTTP/1.0 200 OK\r\nContent-Type: application/json\r\n\r\n{\"models\":[]}";
        assert_eq!(http_body(ok), Some("{\"models\":[]}"));
        assert_eq!(http_body(b"HTTP/1.0 404 Not Found\r\n\r\nnope"), None);
        assert_eq!(http_body(b"garbage"), None);
    }
}
