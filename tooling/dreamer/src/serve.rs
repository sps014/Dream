//! Minimal static file server for `dreamer run --target web`.

use anyhow::{Context, Result};
use std::fs;
use std::io::Cursor;
use std::net::SocketAddr;
use std::path::{Component, Path, PathBuf};
use tiny_http::{Header, Method, Request, Response, Server, StatusCode};

/// Serve `root` on `127.0.0.1` (ephemeral port). Prints the index URL and blocks until Ctrl-C /
/// server error.
pub fn serve_project(root: &Path) -> Result<()> {
    let server = Server::http("127.0.0.1:0").map_err(|e| anyhow::anyhow!("bind failed: {}", e))?;
    let addr = server.server_addr().to_ip().unwrap_or_else(|| SocketAddr::from(([127, 0, 0, 1], 0)));
    let url = format!("http://127.0.0.1:{}/index.html", addr.port());
    println!("Serving {} at {}", root.display(), url);
    println!("Press Ctrl-C to stop.");

    for request in server.incoming_requests() {
        if let Err(e) = handle_request(root, request) {
            eprintln!("static server error: {:#}", e);
        }
    }
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
