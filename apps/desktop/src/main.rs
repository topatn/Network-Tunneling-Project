//! Standalone native Windows desktop application executable for Game Tunnel Tooling (GTT)
//! (`T-080`, `T-081`, `T-100`, `ADR-005`, `DESIGN.md`).
//!
//! Embeds all frontend assets (HTML, tokens.css, app.css, app.js) into the binary,
//! hosts an in-memory loopback HTTP server on `127.0.0.1`, and launches the native
//! standalone application window in WebView2/Edge app mode.

use std::env;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::Command;
use std::thread;
use std::time::Duration;

const INDEX_HTML: &str = include_str!("../index.html");
const TOKENS_CSS: &str = include_str!("tokens.css");
const APP_CSS: &str = include_str!("app.css");
const APP_JS: &str = include_str!("app.js");

fn main() {
    let args: Vec<String> = env::args().collect();
    let no_open = args.iter().any(|a| a == "--no-open");
    let target_port: Option<u16> = args
        .windows(2)
        .find(|w| w.first().map(|s| s.as_str()) == Some("--port"))
        .and_then(|w| w.get(1))
        .and_then(|p| p.parse().ok());

    // Bind loopback listener (try target port, then 5173..=5180, then auto-assigned 0)
    let listener = match target_port {
        Some(p) => match TcpListener::bind(("127.0.0.1", p)) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("[ERROR] Could not bind to requested port {p}: {e}");
                return;
            }
        },
        None => {
            let mut bound = None;
            for p in 5173..=5180 {
                if let Ok(l) = TcpListener::bind(("127.0.0.1", p)) {
                    bound = Some(l);
                    break;
                }
            }
            match bound {
                Some(l) => l,
                None => match TcpListener::bind(("127.0.0.1", 0)) {
                    Ok(l) => l,
                    Err(e) => {
                        eprintln!("[ERROR] Could not bind loopback listener: {e}");
                        return;
                    }
                },
            }
        }
    };

    let local_addr = match listener.local_addr() {
        Ok(addr) => addr,
        Err(e) => {
            eprintln!("[ERROR] Could not get local address: {e}");
            return;
        }
    };

    let port = local_addr.port();
    let url = format!("http://127.0.0.1:{port}");

    println!("\n======================================================");
    println!(" Game Tunnel Tooling (GTT) Desktop Shell Running!     ");
    println!(" Web UI URL: {url}                                    ");
    println!(" Keep this console window open while using GTT.       ");
    println!(" Press Ctrl+C to stop.                                ");
    println!("======================================================\n");

    // Launch standalone window in Edge app mode if requested
    if !no_open {
        launch_window(&url);
    } else {
        println!("[INFO] Running in headless mode (--no-open). Access via {url}");
    }

    // Serve incoming connections on listener
    for stream_res in listener.incoming() {
        match stream_res {
            Ok(stream) => {
                thread::spawn(|| {
                    handle_connection(stream);
                });
            }
            Err(e) => {
                eprintln!("[WARN] Failed to accept stream: {e}");
            }
        }
    }
}

fn handle_connection(mut stream: TcpStream) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));

    let mut buffer = [0u8; 4096];
    let bytes_read = match stream.read(&mut buffer) {
        Ok(n) => n,
        Err(_) => return,
    };

    let req_str = match std::str::from_utf8(buffer.get(..bytes_read).unwrap_or(&[])) {
        Ok(s) => s,
        Err(_) => return,
    };

    let first_line = req_str.lines().next().unwrap_or("");
    let mut parts = first_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let full_path = parts.next().unwrap_or("");
    let clean_path = full_path.split('?').next().unwrap_or(full_path);

    if method != "GET" && method != "HEAD" {
        let response = "HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 0\r\n\r\n";
        let _ = stream.write_all(response.as_bytes());
        return;
    }

    let (content_type, body) = match clean_path {
        "/" | "/index.html" => ("text/html; charset=utf-8", INDEX_HTML),
        "/src/tokens.css" => ("text/css; charset=utf-8", TOKENS_CSS),
        "/src/app.css" => ("text/css; charset=utf-8", APP_CSS),
        "/src/app.js" => ("application/javascript; charset=utf-8", APP_JS),
        _ => {
            let response = "HTTP/1.1 404 Not Found\r\nContent-Length: 13\r\n\r\n404 Not Found";
            let _ = stream.write_all(response.as_bytes());
            return;
        }
    };

    let content_len = body.len();
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {content_len}\r\nConnection: close\r\n\r\n"
    );

    let _ = stream.write_all(header.as_bytes());
    if method == "GET" {
        let _ = stream.write_all(body.as_bytes());
    }
}

fn launch_window(url: &str) {
    let edge_paths = [
        "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe",
        "C:\\Program Files\\Microsoft\\Edge\\Application\\msedge.exe",
    ];

    let user_data_dir = env::var("LOCALAPPDATA")
        .map(|p| Path::new(&p).join("GTT").join("webview-profile"))
        .unwrap_or_else(|_| env::temp_dir().join("gtt-webview-profile"));

    let _ = std::fs::create_dir_all(&user_data_dir);
    let user_data_arg = format!("--user-data-dir={}", user_data_dir.display());

    for path in edge_paths {
        if Path::new(path).exists() {
            println!("[INFO] Opening GTT standalone window via WebView2/Edge...");
            let app_arg = format!("--app={url}");
            if Command::new(path)
                .args([
                    &app_arg,
                    &user_data_arg,
                    "--window-size=980,720",
                    "--no-first-run",
                    "--no-default-browser-check",
                ])
                .spawn()
                .is_ok()
            {
                return;
            }
        }
    }

    // Fallback: system browser
    println!("[INFO] Opening in default web browser: {url}");
    let _ = Command::new("cmd").args(["/c", "start", url]).spawn();
}
