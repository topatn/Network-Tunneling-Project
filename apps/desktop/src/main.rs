//! Standalone native Windows desktop application executable for Game Tunnel Tooling (GTT)
//! (`T-080`, `T-081`, `T-100`, `T-101`, `T-102`, `ADR-005`, `DESIGN.md`).
//!
//! Embeds all frontend assets (HTML, tokens.css, app.css, app.js) and the Windows `asInvoker`
//! application manifest into the binary, provides CLI entrypoints for per-user installation,
//! WebView2 runtime detection, startup self-checks, and clean uninstallation, and hosts an
//! in-memory loopback HTTP server on `127.0.0.1` for the standalone WebView2/Edge window.

use std::env;
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Command, ExitCode};
use std::thread;
use std::time::Duration;

use gtt_desktop::packaging::{
    verify_manifest_as_invoker, PerUserInstaller, PerUserPaths, UninstallOptions, Uninstaller,
    WebView2Probe, APPLICATION_MANIFEST_XML,
};

const INDEX_HTML: &str = include_str!("../index.html");
const TOKENS_CSS: &str = include_str!("tokens.css");
const APP_CSS: &str = include_str!("app.css");
const APP_JS: &str = include_str!("app.js");

fn flag_value<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.windows(2)
        .find(|w| w.first().map(String::as_str) == Some(flag))
        .and_then(|w| w.get(1))
        .map(String::as_str)
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();

    // 1. `--self-check`: verify embedded assets + `asInvoker` manifest policy (`T-100`, `T-101`).
    if args.iter().any(|a| a == "--self-check") {
        if let Err(e) = verify_manifest_as_invoker(APPLICATION_MANIFEST_XML) {
            eprintln!("[ERROR] Self-check failed: {e}");
            return ExitCode::FAILURE;
        }
        if INDEX_HTML.is_empty() || TOKENS_CSS.is_empty() || APP_CSS.is_empty() || APP_JS.is_empty()
        {
            eprintln!("[ERROR] Self-check failed: embedded UI assets are empty");
            return ExitCode::FAILURE;
        }
        println!(
            "{{\"status\":\"ok\",\"version\":{:?},\"execution_level\":\"asInvoker\"}}",
            env!("CARGO_PKG_VERSION")
        );
        return ExitCode::SUCCESS;
    }

    // 2. `--verify-webview2`: probe Evergreen WebView2 runtime status (`T-100`, `INST-005`).
    if args.iter().any(|a| a == "--verify-webview2") {
        let probe = WebView2Probe::from_host_system();
        let status = probe.evaluate();
        match serde_json::to_string_pretty(&status) {
            Ok(json) => {
                println!("{json}");
                return ExitCode::SUCCESS;
            }
            Err(e) => {
                eprintln!("[ERROR] Failed to serialize WebView2 status: {e}");
                return ExitCode::FAILURE;
            }
        }
    }

    // 3. `--install-per-user <localappdata_dir> [--version <ver>]`: install per-user (`T-100`, `INST-001`, `SEC-012`).
    if let Some(base_dir) = flag_value(&args, "--install-per-user") {
        let version = flag_value(&args, "--version").unwrap_or("0.1.0");
        let paths = match PerUserPaths::from_local_app_data(Path::new(base_dir)) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("[ERROR] Invalid per-user install path: {e}");
                return ExitCode::FAILURE;
            }
        };
        let current_exe = match env::current_exe() {
            Ok(p) => p,
            Err(e) => {
                eprintln!("[ERROR] Could not resolve current executable path: {e}");
                return ExitCode::FAILURE;
            }
        };
        let binary_bytes = match fs::read(&current_exe) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("[ERROR] Could not read current executable bytes: {e}");
                return ExitCode::FAILURE;
            }
        };
        let probe = WebView2Probe::from_host_system();
        match PerUserInstaller::install(&paths, version, &binary_bytes, &probe) {
            Ok(receipt) => {
                if let Ok(json) = serde_json::to_string_pretty(&receipt) {
                    println!("{json}");
                }
                return ExitCode::SUCCESS;
            }
            Err(e) => {
                eprintln!("[ERROR] Per-user installation failed: {e}");
                return ExitCode::FAILURE;
            }
        }
    }

    // 4. `--uninstall-per-user <localappdata_dir> [--remove-user-data]` (`T-102`, `INST-004`, `SRV-010`).
    if let Some(base_dir) = flag_value(&args, "--uninstall-per-user") {
        let remove_user_data = args.iter().any(|a| a == "--remove-user-data");
        let paths = match PerUserPaths::from_local_app_data(Path::new(base_dir)) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("[ERROR] Invalid per-user uninstall path: {e}");
                return ExitCode::FAILURE;
            }
        };
        let options = UninstallOptions { remove_user_data };
        match Uninstaller::uninstall(&paths, options, |_| {}) {
            Ok(report) => {
                if let Ok(json) = serde_json::to_string_pretty(&report) {
                    println!("{json}");
                }
                return ExitCode::SUCCESS;
            }
            Err(e) => {
                eprintln!("[ERROR] Per-user uninstallation failed: {e}");
                return ExitCode::FAILURE;
            }
        }
    }

    let no_open = args.iter().any(|a| a == "--no-open");
    let target_port: Option<u16> = flag_value(&args, "--port").and_then(|p| p.parse().ok());

    // Bind loopback listener (try target port, then 5173..=5180, then auto-assigned 0)
    let listener = match target_port {
        Some(p) => match TcpListener::bind(("127.0.0.1", p)) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("[ERROR] Could not bind to requested port {p}: {e}");
                return ExitCode::FAILURE;
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
                        return ExitCode::FAILURE;
                    }
                },
            }
        }
    };

    let local_addr = match listener.local_addr() {
        Ok(addr) => addr,
        Err(e) => {
            eprintln!("[ERROR] Could not get local address: {e}");
            return ExitCode::FAILURE;
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

    ExitCode::SUCCESS
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

    let _ = fs::create_dir_all(&user_data_dir);
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
