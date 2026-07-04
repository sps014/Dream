//! Debug Adapter Protocol wire format: `Content-Length`-framed JSON over a byte stream, plus a
//! small helper for constructing DAP response/event envelopes. Deliberately minimal — just what the
//! Dream debug adapter needs to talk to a client such as the VS Code extension.

use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::sync::atomic::{AtomicI64, Ordering};

/// Monotonic sequence number shared by every message the adapter emits (responses + events).
static SEQ: AtomicI64 = AtomicI64::new(1);

fn next_seq() -> i64 {
    SEQ.fetch_add(1, Ordering::SeqCst)
}

/// Reads one `Content-Length`-framed JSON message from `reader`. Returns `Ok(None)` at end of input.
pub fn read_message<R: BufRead>(reader: &mut R) -> std::io::Result<Option<Value>> {
    let mut content_length: Option<usize> = None;
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            return Ok(None); // EOF
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break; // end of headers
        }
        if let Some(rest) = trimmed.to_ascii_lowercase().strip_prefix("content-length:") {
            content_length = rest.trim().parse::<usize>().ok();
        }
    }
    let len = match content_length {
        Some(l) => l,
        None => return Ok(None),
    };
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf)?;
    let value = serde_json::from_slice(&buf).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("bad DAP JSON: {}", e),
        )
    })?;
    Ok(Some(value))
}

/// Writes framed DAP messages to an underlying stream (stdout in practice). Shared across threads
/// behind a mutex so responses (main thread) and events (wasm thread) never interleave.
pub struct DapWriter<W: Write> {
    out: W,
}

impl<W: Write> DapWriter<W> {
    pub fn new(out: W) -> Self {
        DapWriter { out }
    }

    /// Frames and writes a single message, flushing so the client sees it immediately.
    pub fn send(&mut self, msg: &Value) -> std::io::Result<()> {
        let body = serde_json::to_string(msg)?;
        write!(self.out, "Content-Length: {}\r\n\r\n{}", body.len(), body)?;
        self.out.flush()
    }

    /// Sends a successful response to `request`, carrying optional `body`.
    pub fn respond(&mut self, request: &Value, body: Value) -> std::io::Result<()> {
        self.send(&response_envelope(request, true, None, body))
    }

    /// Sends a failure response to `request` with a human-readable `message`.
    #[allow(dead_code)]
    pub fn respond_error(&mut self, request: &Value, message: &str) -> std::io::Result<()> {
        self.send(&response_envelope(
            request,
            false,
            Some(message.to_string()),
            Value::Null,
        ))
    }

    /// Sends an event with the given name and body.
    pub fn event(&mut self, event: &str, body: Value) -> std::io::Result<()> {
        self.send(&json!({
            "seq": next_seq(),
            "type": "event",
            "event": event,
            "body": body,
        }))
    }
}

fn response_envelope(
    request: &Value,
    success: bool,
    message: Option<String>,
    body: Value,
) -> Value {
    let request_seq = request.get("seq").and_then(|s| s.as_i64()).unwrap_or(0);
    let command = request
        .get("command")
        .and_then(|c| c.as_str())
        .unwrap_or("");
    let mut env = json!({
        "seq": next_seq(),
        "type": "response",
        "request_seq": request_seq,
        "success": success,
        "command": command,
    });
    if let Some(msg) = message {
        env["message"] = json!(msg);
    }
    if !body.is_null() {
        env["body"] = body;
    }
    env
}
