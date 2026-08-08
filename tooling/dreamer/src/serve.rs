//! Minimal static file server for `dreamer run --target web`.

use anyhow::{bail, Context, Result};
use std::fs;
use std::io::Cursor;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::Duration;
use tiny_http::{Header, Method, Request, Response, Server, StatusCode};
use yansi::Paint;

/// Default loopback port for `dreamer run --target web` (stable across restarts).
pub const DEFAULT_WEB_PORT: u16 = 8787;

const PID_FILE_NAME: &str = "dream-web-server.pid";

/// Serve `root` on `127.0.0.1:{port}`. Replaces any previous dreamer web server for this project
/// (PID lock under `target/`) so Run-again reuses the same URL. Blocks until Ctrl-C / server error.
pub fn serve_project(root: &Path, port: u16) -> Result<()> {
    let pid_path = pid_file_path(root);
    stop_previous_server(&pid_path)?;

    let addr = format!("127.0.0.1:{port}");
    let server = bind_with_retry(&addr)?;
    write_pid_file(&pid_path, port)?;

    let url = format!("http://127.0.0.1:{port}/index.html");
    let root_disp = root.display().to_string();
    println!(
        "{} {} {} {}",
        Paint::green("Serving").bold(),
        Paint::cyan(&root_disp),
        Paint::green("at"),
        Paint::cyan(&url).bold().underline()
    );
    println!("{}", Paint::dim("Press Ctrl-C to stop."));

    let _pid_guard = PidFileGuard { path: pid_path };

    for request in server.incoming_requests() {
        if let Err(e) = handle_request(root, request) {
            eprintln!("{} {:#}", Paint::yellow("static server error:"), e);
        }
    }
    Ok(())
}

struct PidFileGuard {
    path: PathBuf,
}

impl Drop for PidFileGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn pid_file_path(root: &Path) -> PathBuf {
    root.join("target").join(PID_FILE_NAME)
}

fn write_pid_file(path: &Path, port: u16) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let body = format!("{}\n{}\n", std::process::id(), port);
    fs::write(path, body).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

fn read_pid_file(path: &Path) -> Option<(u32, u16)> {
    let text = fs::read_to_string(path).ok()?;
    let mut lines = text.lines();
    let pid: u32 = lines.next()?.trim().parse().ok()?;
    let port: u16 = lines
        .next()
        .and_then(|l| l.trim().parse().ok())
        .unwrap_or(DEFAULT_WEB_PORT);
    Some((pid, port))
}

fn stop_previous_server(pid_path: &Path) -> Result<()> {
    let Some((pid, _port)) = read_pid_file(pid_path) else {
        let _ = fs::remove_file(pid_path);
        return Ok(());
    };
    if pid == std::process::id() {
        return Ok(());
    }
    if process_alive(pid) {
        println!(
            "{} previous web server (pid {})",
            Paint::yellow("Restarting:"),
            pid
        );
        kill_process(pid)?;
        for _ in 0..50 {
            if !process_alive(pid) {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    let _ = fs::remove_file(pid_path);
    Ok(())
}

fn bind_with_retry(addr: &str) -> Result<Server> {
    let mut last_err = None;
    for attempt in 0..25 {
        match Server::http(addr) {
            Ok(server) => return Ok(server),
            Err(e) => {
                last_err = Some(e.to_string());
                if attempt + 1 < 25 {
                    std::thread::sleep(Duration::from_millis(40));
                }
            }
        }
    }
    bail!(
        "could not bind {}: {} (is another process using this port?)",
        addr,
        last_err.unwrap_or_else(|| "unknown error".into())
    )
}

#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(windows)]
fn process_alive(pid: u32) -> bool {
    let output = Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH"])
        .output();
    match output {
        Ok(out) => String::from_utf8_lossy(&out.stdout).contains(&pid.to_string()),
        Err(_) => true,
    }
}

#[cfg(not(any(unix, windows)))]
fn process_alive(_pid: u32) -> bool {
    false
}

#[cfg(unix)]
fn kill_process(pid: u32) -> Result<()> {
    let status = Command::new("kill")
        .arg(pid.to_string())
        .status()
        .with_context(|| format!("kill pid {pid}"))?;
    // Non-zero is fine if the process already exited.
    let _ = status;
    Ok(())
}

#[cfg(windows)]
fn kill_process(pid: u32) -> Result<()> {
    let status = Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/F"])
        .status()
        .with_context(|| format!("taskkill pid {pid}"))?;
    let _ = status;
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn kill_process(_pid: u32) -> Result<()> {
    Ok(())
}

fn handle_request(root: &Path, request: Request) -> Result<()> {
    if request.method() != &Method::Get && request.method() != &Method::Head {
        let response =
            Response::from_string("Method Not Allowed").with_status_code(StatusCode(405));
        request.respond(response)?;
        return Ok(());
    }

    let url_path = request.url().split('?').next().unwrap_or("/");
    let rel = if url_path == "/" || url_path.is_empty() {
        PathBuf::from("index.html")
    } else {
        PathBuf::from(url_path.trim_start_matches('/'))
    };

    let Some(file_path) = safe_join(root, &rel) else {
        let response = Response::from_string("Forbidden").with_status_code(StatusCode(403));
        request.respond(response)?;
        return Ok(());
    };

    if !file_path.is_file() {
        let response = Response::from_string("Not Found").with_status_code(StatusCode(404));
        request.respond(response)?;
        return Ok(());
    }

    let bytes =
        fs::read(&file_path).with_context(|| format!("reading {}", file_path.display()))?;
    let is_head = request.method() == &Method::Head;
    let mime = content_type(&file_path);
    let header = Header::from_bytes(&b"Content-Type"[..], mime.as_bytes())
        .map_err(|_| anyhow::anyhow!("invalid Content-Type header"))?;
    let response = if is_head {
        Response::empty(StatusCode(200))
            .with_header(header)
            .with_data(Cursor::new(Vec::new()), Some(bytes.len()))
    } else {
        Response::from_data(bytes).with_header(header)
    };
    request.respond(response)?;
    Ok(())
}

fn safe_join(root: &Path, rel: &Path) -> Option<PathBuf> {
    if rel.is_absolute() {
        return None;
    }
    let mut clean = PathBuf::new();
    for c in rel.components() {
        match c {
            Component::Normal(s) => clean.push(s),
            Component::CurDir => {}
            _ => return None,
        }
    }
    Some(root.join(clean))
}

fn content_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "html" | "htm" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "wasm" => "application/wasm",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "ico" => "image/x-icon",
        "map" => "application/json",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pid_file_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(PID_FILE_NAME);
        write_pid_file(&path, 8787).unwrap();
        let (pid, port) = read_pid_file(&path).unwrap();
        assert_eq!(pid, std::process::id());
        assert_eq!(port, 8787);
    }
}
