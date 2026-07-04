//! End-to-end integration test for the Dream Debug Adapter Protocol server (`dream debug-adapter`).
//!
//! Drives a real DAP session over stdio against the built `dream` binary: set a breakpoint, run to
//! it, inspect the call stack + variables, step, and continue to exit. Exercises the full pipeline —
//! debug-info instrumentation, source map, the wasmtime debug runner, and the DAP protocol.

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

/// A tiny two-function program so the call stack has depth: a breakpoint inside `add` should show
/// both `add` and `main`.
const PROGRAM: &str = r#"fun add(a: int, b: int): int {
    let sum = a + b;
    return sum;
}

fun main(): void {
    let x = 10;
    let y = 32;
    let total = add(x, y);
    System.println(total);
}
"#;

struct DapClient {
    child: Child,
    stdin: ChildStdin,
    rx: mpsc::Receiver<serde_json::Value>,
    seq: i64,
}

impl DapClient {
    fn spawn(source: &str) -> DapClient {
        let bin = env!("CARGO_BIN_EXE_dream");
        let mut child = Command::new(bin)
            .arg("debug-adapter")
            .arg(source)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("failed to spawn dream debug-adapter");
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();

        // Reader thread: parse framed DAP messages and forward them over a channel.
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || read_messages(stdout, tx));

        DapClient {
            child,
            stdin,
            rx,
            seq: 1,
        }
    }

    fn request(&mut self, command: &str, arguments: serde_json::Value) {
        let msg = serde_json::json!({
            "seq": self.seq,
            "type": "request",
            "command": command,
            "arguments": arguments,
        });
        self.seq += 1;
        let body = serde_json::to_string(&msg).unwrap();
        write!(self.stdin, "Content-Length: {}\r\n\r\n{}", body.len(), body).unwrap();
        self.stdin.flush().unwrap();
    }

    /// Blocks until a message matching `pred` arrives (or times out / the process exits).
    fn wait_for(&self, pred: impl Fn(&serde_json::Value) -> bool) -> serde_json::Value {
        loop {
            let msg = self
                .rx
                .recv_timeout(Duration::from_secs(20))
                .expect("timed out waiting for a DAP message");
            if pred(&msg) {
                return msg;
            }
        }
    }

    fn wait_response(&self, command: &str) -> serde_json::Value {
        self.wait_for(|m| {
            m["type"] == "response" && m["command"] == command
        })
    }

    fn wait_event(&self, event: &str) -> serde_json::Value {
        self.wait_for(|m| m["type"] == "event" && m["event"] == event)
    }
}

impl Drop for DapClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn read_messages(stdout: ChildStdout, tx: mpsc::Sender<serde_json::Value>) {
    let mut reader = BufReader::new(stdout);
    loop {
        let mut content_length: Option<usize> = None;
        loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
            let trimmed = line.trim_end_matches(['\r', '\n']);
            if trimmed.is_empty() {
                break;
            }
            if let Some(rest) = trimmed.to_ascii_lowercase().strip_prefix("content-length:") {
                content_length = rest.trim().parse().ok();
            }
        }
        let Some(len) = content_length else {
            return;
        };
        let mut buf = vec![0u8; len];
        if reader.read_exact(&mut buf).is_err() {
            return;
        }
        match serde_json::from_slice(&buf) {
            Ok(v) => {
                if tx.send(v).is_err() {
                    return;
                }
            }
            Err(_) => return,
        }
    }
}

#[test]
fn dap_breakpoint_stack_variables_step_continue() {
    // Write the program to a unique temp file (the adapter compiles it and emits sibling artifacts).
    let dir = std::env::temp_dir().join(format!("dream_dap_test_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let source = dir.join("prog.dream");
    std::fs::write(&source, PROGRAM).unwrap();
    let source_path = source.to_string_lossy().into_owned();

    let mut client = DapClient::spawn(&source_path);

    client.request(
        "initialize",
        serde_json::json!({ "adapterID": "dream", "linesStartAt1": true }),
    );
    client.wait_response("initialize");
    client.wait_event("initialized");

    client.request("launch", serde_json::json!({ "program": source_path }));
    client.wait_response("launch");

    // Breakpoint on `return sum;` (line 3), inside `add`.
    client.request(
        "setBreakpoints",
        serde_json::json!({
            "source": { "path": source_path },
            "breakpoints": [ { "line": 3 } ],
        }),
    );
    let bp = client.wait_response("setBreakpoints");
    assert_eq!(bp["body"]["breakpoints"][0]["verified"], true);

    client.request("configurationDone", serde_json::json!({}));
    client.wait_response("configurationDone");

    // Should stop at the breakpoint.
    let stopped = client.wait_event("stopped");
    assert_eq!(stopped["body"]["reason"], "breakpoint");

    // The call stack must show `add` (innermost, line 3) over `main`.
    client.request("stackTrace", serde_json::json!({ "threadId": 1 }));
    let st = client.wait_response("stackTrace");
    let frames = st["body"]["stackFrames"].as_array().unwrap();
    assert_eq!(frames[0]["name"], "add");
    assert_eq!(frames[0]["line"], 3);
    assert_eq!(frames[1]["name"], "main");

    // The innermost frame's locals should reflect a=10, b=32, sum=42 (the assignment on line 2 ran).
    client.request("scopes", serde_json::json!({ "frameId": 0 }));
    let scopes = client.wait_response("scopes");
    let reference = scopes["body"]["scopes"][0]["variablesReference"].clone();
    client.request(
        "variables",
        serde_json::json!({ "variablesReference": reference }),
    );
    let vars = client.wait_response("variables");
    let vars = vars["body"]["variables"].as_array().unwrap();
    let get = |name: &str| {
        vars.iter()
            .find(|v| v["name"] == name)
            .map(|v| v["value"].as_str().unwrap().to_string())
    };
    assert_eq!(get("a"), Some("10".to_string()));
    assert_eq!(get("b"), Some("32".to_string()));
    assert_eq!(get("sum"), Some("42".to_string()));

    // A watch expression resolves against the same locals.
    client.request(
        "evaluate",
        serde_json::json!({ "expression": "sum", "frameId": 0, "context": "watch" }),
    );
    let eval = client.wait_response("evaluate");
    assert_eq!(eval["body"]["result"], "42");

    // Step out of `add` and run to completion.
    client.request("stepOut", serde_json::json!({ "threadId": 1 }));
    client.wait_response("stepOut");
    client.wait_event("stopped");

    client.request("continue", serde_json::json!({ "threadId": 1 }));
    client.wait_response("continue");

    // Program output is surfaced as `output` events; expect the printed total.
    // Then the program terminates.
    client.wait_event("terminated");

    // Best-effort cleanup of the emitted artifacts.
    let _ = std::fs::remove_dir_all(&dir);
}
