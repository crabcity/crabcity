use anyhow::{Context, Result};
use axum::{
    Json, Router,
    extract::{State, ws::WebSocketUpgrade},
    http::StatusCode,
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use clap::Parser;
use maud::{DOCTYPE, PreEscaped, html};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::Arc;
use tower_http::cors::CorsLayer;
use tracing::info;

use tty_wrapper::PtyManager;
use tty_wrapper::websocket::handle_websocket;

// RAII guard to restore terminal settings on drop
#[cfg(unix)]
struct TerminalGuard {
    original: Option<nix::sys::termios::Termios>,
}

#[cfg(unix)]
impl TerminalGuard {
    fn new() -> Self {
        use nix::sys::termios;
        let stdin = std::io::stdin();
        let original = termios::tcgetattr(&stdin).ok();
        Self { original }
    }

    fn make_raw(&self) {
        if let Some(ref termios) = self.original {
            use nix::sys::termios;
            let stdin = std::io::stdin();
            let mut raw = termios.clone();
            termios::cfmakeraw(&mut raw);
            let _ = termios::tcsetattr(&stdin, termios::SetArg::TCSANOW, &raw);
        }
    }
}

#[cfg(unix)]
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if let Some(ref termios) = self.original {
            use nix::sys::termios;
            let stdin = std::io::stdin();
            let _ = termios::tcsetattr(&stdin, termios::SetArg::TCSANOW, termios);
        }
    }
}

#[derive(Parser)]
#[command(name = "wrapper")]
#[command(about = "HTTP-controlled TTY wrapper for interactive programs")]
struct Args {
    /// Command to run
    command: String,

    /// Arguments for the command
    args: Vec<String>,

    /// Port for the HTTP server (0 for automatic)
    #[arg(short, long, default_value = "0")]
    port: u16,

    /// Host to bind to
    #[arg(short = 'b', long, default_value = "127.0.0.1")]
    host: String,

    /// Enable debug logging
    #[arg(short, long)]
    debug: bool,

    /// Run in headless mode (no terminal output)
    #[arg(long)]
    headless: bool,

    /// Keep server running after process exits
    #[arg(long)]
    persist: bool,
}

#[derive(Clone)]
struct AppState {
    pty: Arc<PtyManager>,
    _session_id: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Setup logging
    let filter = if args.debug {
        "tty_wrapper=debug,tower_http=debug"
    } else {
        "tty_wrapper=info"
    };
    tracing_subscriber::fmt().with_env_filter(filter).init();

    info!("Starting TTY wrapper for: {} {:?}", args.command, args.args);

    // Create PTY manager and spawn the process
    info!("Spawning PTY process...");
    let pty = PtyManager::spawn(&args.command, &args.args, None, !args.headless)
        .context("Failed to spawn process")?;
    info!("PTY process spawned successfully");

    let session_id = uuid::Uuid::new_v4().to_string();
    let state = AppState {
        pty: Arc::new(pty),
        _session_id: session_id.clone(),
    };

    // Create terminal guard that will restore settings on drop
    #[cfg(unix)]
    let _terminal_guard = if !args.headless {
        let guard = TerminalGuard::new();
        guard.make_raw();
        Some(guard)
    } else {
        None
    };

    // Set up signal handling to ignore SIGINT in the wrapper
    // The PTY will handle forwarding Ctrl+C to the child process
    #[cfg(unix)]
    {
        tokio::spawn(async move {
            let mut stream =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
                    .expect("Failed to create SIGINT handler");

            while stream.recv().await.is_some() {
                // Ignore SIGINT - let the PTY handle it
                info!("SIGINT received, ignoring (PTY will handle it)");
            }
        });
    }

    // Forward stdin to PTY when not in headless mode
    if !args.headless {
        let pty_clone = state.pty.clone();
        std::thread::spawn(move || {
            use std::io::{self, Read};

            let runtime = tokio::runtime::Runtime::new().unwrap();
            let mut stdin = io::stdin();
            let mut buffer = [0u8; 1]; // Read one byte at a time in raw mode

            loop {
                match stdin.read(&mut buffer) {
                    Ok(0) => {
                        // EOF reached
                        break;
                    }
                    Ok(n) => {
                        // Forward the actual bytes received to PTY
                        let data = buffer[..n].to_vec();
                        let pty = pty_clone.clone();
                        runtime.block_on(async move {
                            let input = String::from_utf8_lossy(&data).to_string();
                            let _ = pty.write_input(&input).await;
                        });
                    }
                    Err(e) => {
                        // Log error but keep trying
                        eprintln!("Error reading stdin: {}", e);
                        std::thread::sleep(std::time::Duration::from_millis(100));
                    }
                }
            }
        });
    }

    // Set up shutdown channel for graceful exit
    let shutdown_rx = if !args.persist {
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let pty_monitor = state.pty.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                match pty_monitor.get_state().await {
                    Ok(state) => {
                        if !state.running {
                            info!("Process exited, shutting down wrapper");
                            let _ = shutdown_tx.send(());
                            break;
                        }
                    }
                    Err(_) => {
                        // Actor is gone, process must have exited
                        info!("PTY actor stopped, shutting down wrapper");
                        let _ = shutdown_tx.send(());
                        break;
                    }
                }
            }
        });
        shutdown_rx
    } else {
        // Create a channel that will never send (for persist mode)
        let (_tx, rx) = tokio::sync::oneshot::channel::<()>();
        rx
    };

    // Build the HTTP API
    let api_routes = Router::new()
        .route("/state", get(get_state))
        .route("/input", post(send_input))
        .route("/output", get(get_output))
        .route("/history", get(get_history))
        .route("/resize", post(resize_terminal))
        .route("/kill", post(kill_process))
        .route("/ws", get(websocket_handler))
        .route("/docs", get(api_docs))
        .with_state(state.clone());

    let app = Router::new()
        .route("/", get(web_ui))
        .route("/health", get(health))
        .nest("/api", api_routes)
        .layer(CorsLayer::permissive())
        .with_state(state);

    let addr = format!("{}:{}", args.host, args.port).parse::<SocketAddr>()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;

    // Get the actual bound port (important when port was 0)
    let actual_addr = listener.local_addr()?;
    let actual_port = actual_addr.port();

    // Output port information in a machine-readable format first
    println!("WRAPPER_PORT={}", actual_port);
    println!("WRAPPER_ADDR={}", actual_addr);

    info!("HTTP server listening on http://{}", actual_addr);
    info!("Session ID: {}", session_id);
    info!("");
    info!("Endpoints:");
    info!("  GET  /           - Web UI");
    info!("  GET  /api/state  - Get current PTY state");
    info!("  POST /api/input  - Send input to the process");
    info!("  GET  /api/output - Get recent output");
    info!("  GET  /api/history- Get full output history");
    info!("  POST /api/resize - Resize terminal");
    info!("  POST /api/kill   - Kill the process");
    info!("  GET  /api/ws     - WebSocket for real-time I/O");

    // Run the server with graceful shutdown
    let server = axum::serve(listener, app);

    tokio::select! {
        result = server => {
            result?;
        }
        _ = shutdown_rx => {
            info!("Received shutdown signal, exiting gracefully");
        }
    }

    Ok(())
}

async fn web_ui(State(state): State<AppState>) -> impl IntoResponse {
    let pty_state = state.pty.get_state().await.ok();

    let status_class = if pty_state.as_ref().map_or(false, |s| s.running) {
        "running"
    } else {
        "stopped"
    };

    let command_info = pty_state.as_ref().map_or("Unknown".to_string(), |s| {
        if s.args.is_empty() {
            s.command.clone()
        } else {
            format!("{} {}", s.command, s.args.join(" "))
        }
    });

    let pid_info = pty_state
        .as_ref()
        .and_then(|s| s.pid)
        .map_or("N/A".to_string(), |pid| pid.to_string());

    let markup = html! {
        (DOCTYPE)
        html {
            head {
                title { "TTY Wrapper - " (command_info) }
                link rel="stylesheet" href="https://cdn.jsdelivr.net/npm/xterm@5.3.0/css/xterm.css";
                style { (PreEscaped(CSS)) }
            }
            body {
                div class="container" {
                    h1 { "TTY Wrapper" }

                    div class="status-card" {
                        h2 { "Process Status" }
                        div class="info-grid" {
                            div class="info-item" {
                                span class="label" { "Status:" }
                                span class=(format!("value status-{}", status_class)) {
                                    @if pty_state.as_ref().map_or(false, |s| s.running) {
                                        "\u{25cf} Running"
                                    } @else {
                                        "\u{25cb} Stopped"
                                    }
                                }
                            }
                            div class="info-item" {
                                span class="label" { "Command:" }
                                span class="value command" { (command_info) }
                            }
                            div class="info-item" {
                                span class="label" { "PID:" }
                                span class="value" { (pid_info) }
                            }
                            div class="info-item" {
                                span class="label" { "Terminal Size:" }
                                span class="value" {
                                    @if let Some(s) = &pty_state {
                                        (format!("{}\u{00d7}{}", s.cols, s.rows))
                                    } @else {
                                        "N/A"
                                    }
                                }
                            }
                        }
                    }

                    div class="terminal-card" {
                        h2 { "Terminal" }
                        div class="terminal-container" {
                            div id="terminal" {}
                        }
                    }

                    div class="controls-card" {
                        h2 { "Controls" }
                        div class="button-group" {
                            button id="refresh" class="btn btn-primary" { "Refresh Output" }
                            button id="clear" class="btn btn-secondary" { "Clear Display" }
                            @if pty_state.as_ref().map_or(false, |s| s.running) {
                                button id="kill" class="btn btn-danger" { "Kill Process" }
                            }
                        }
                    }

                    div class="api-card" {
                        h2 { "API Documentation" }
                        div class="api-section" {
                            h3 { "REST Endpoints" }
                            ul class="api-list" {
                                li { code { "GET /api/state" } " - Get current process state" }
                                li { code { "POST /api/input" } " - Send input text (JSON: {\"text\": \"...\"})" }
                                li { code { "GET /api/output" } " - Get recent output (last 100 lines)" }
                                li { code { "GET /api/history" } " - Get full output history" }
                                li { code { "POST /api/resize" } " - Resize terminal (JSON: {\"rows\": N, \"cols\": N})" }
                                li { code { "POST /api/kill" } " - Kill process (JSON: {\"signal\": \"SIGTERM\"})" }
                            }
                        }
                        div class="api-section" {
                            h3 { "WebSocket" }
                            p {
                                "Connect to " code { "/api/ws" } " for real-time bidirectional I/O. "
                                "Send/receive JSON messages with type 'Input' or 'Output'."
                            }
                        }
                    }
                }

                script src="https://cdn.jsdelivr.net/npm/xterm@5.3.0/lib/xterm.js" {}
                script src="https://cdn.jsdelivr.net/npm/xterm-addon-fit@0.8.0/lib/xterm-addon-fit.js" {}
                script src="https://cdn.jsdelivr.net/npm/xterm-addon-web-links@0.9.0/lib/xterm-addon-web-links.js" {}
                script { (PreEscaped(JAVASCRIPT)) }
            }
        }
    };

    Html(markup.into_string())
}

async fn api_docs() -> impl IntoResponse {
    Json(ApiDocs {
        name: "TTY Wrapper API".to_string(),
        version: "1.0.0".to_string(),
        endpoints: vec![
            EndpointDoc {
                method: "GET",
                path: "/api/state",
                description: "Get current PTY state including process info and terminal size",
            },
            EndpointDoc {
                method: "POST",
                path: "/api/input",
                description: "Send text input to the wrapped process",
            },
            EndpointDoc {
                method: "GET",
                path: "/api/output",
                description: "Get recent output (last 100 lines)",
            },
            EndpointDoc {
                method: "GET",
                path: "/api/history",
                description: "Get full output history",
            },
            EndpointDoc {
                method: "POST",
                path: "/api/resize",
                description: "Resize the terminal (rows, cols)",
            },
            EndpointDoc {
                method: "POST",
                path: "/api/kill",
                description: "Send signal to the process",
            },
            EndpointDoc {
                method: "GET",
                path: "/api/ws",
                description: "WebSocket for real-time bidirectional I/O",
            },
        ],
    })
}

async fn health() -> impl IntoResponse {
    StatusCode::OK
}

async fn get_state(State(state): State<AppState>) -> Response {
    match state.pty.get_state().await {
        Ok(pty_state) => Json(pty_state).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn send_input(State(state): State<AppState>, Json(payload): Json<InputPayload>) -> Response {
    match state.pty.write_input(&payload.text).await {
        Ok(bytes_written) => Json(InputResponse {
            success: true,
            bytes_written,
            error: None,
        })
        .into_response(),
        Err(e) => Json(InputResponse {
            success: false,
            bytes_written: 0,
            error: Some(e.to_string()),
        })
        .into_response(),
    }
}

async fn get_output(State(state): State<AppState>) -> Response {
    let lines = state.pty.get_recent_output(100).await;
    let total_lines = state.pty.get_output_line_count().await;
    Json(OutputResponse { lines, total_lines }).into_response()
}

async fn get_history(State(state): State<AppState>) -> Response {
    let full_output = state.pty.get_full_output().await;
    let line_count = state.pty.get_output_line_count().await;
    Json(HistoryResponse {
        full_output,
        line_count,
    })
    .into_response()
}

async fn resize_terminal(
    State(state): State<AppState>,
    Json(payload): Json<ResizePayload>,
) -> Response {
    match state.pty.resize(payload.rows, payload.cols).await {
        Ok(_) => StatusCode::OK.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn kill_process(State(state): State<AppState>, Json(payload): Json<KillPayload>) -> Response {
    match state.pty.kill(payload.signal.as_deref()).await {
        Ok(_) => StatusCode::OK.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn websocket_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_websocket(socket, state.pty))
}

// API Types
#[derive(Serialize)]
struct ApiDocs {
    name: String,
    version: String,
    endpoints: Vec<EndpointDoc>,
}

#[derive(Serialize)]
struct EndpointDoc {
    method: &'static str,
    path: &'static str,
    description: &'static str,
}

#[derive(Deserialize)]
struct InputPayload {
    text: String,
}

#[derive(Serialize)]
struct InputResponse {
    success: bool,
    bytes_written: usize,
    error: Option<String>,
}

#[derive(Serialize)]
struct OutputResponse {
    lines: Vec<String>,
    total_lines: usize,
}

#[derive(Serialize)]
struct HistoryResponse {
    full_output: String,
    line_count: usize,
}

#[derive(Deserialize)]
struct ResizePayload {
    rows: u16,
    cols: u16,
}

#[derive(Deserialize)]
struct KillPayload {
    signal: Option<String>,
}

const CSS: &str = r#"
    * {
        margin: 0;
        padding: 0;
        box-sizing: border-box;
    }

    body {
        font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, Oxygen, Ubuntu, sans-serif;
        background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
        min-height: 100vh;
        padding: 20px;
    }

    .container {
        max-width: 1200px;
        margin: 0 auto;
    }

    h1 {
        color: white;
        font-size: 2.5rem;
        margin-bottom: 30px;
        text-shadow: 2px 2px 4px rgba(0,0,0,0.2);
    }

    .status-card, .terminal-card, .controls-card, .api-card {
        background: white;
        border-radius: 12px;
        padding: 20px;
        margin-bottom: 20px;
        box-shadow: 0 10px 30px rgba(0,0,0,0.1);
    }

    h2 {
        color: #333;
        margin-bottom: 15px;
        font-size: 1.5rem;
    }

    h3 {
        color: #555;
        margin-bottom: 10px;
        font-size: 1.2rem;
    }

    .info-grid {
        display: grid;
        grid-template-columns: repeat(auto-fit, minmax(250px, 1fr));
        gap: 15px;
    }

    .info-item {
        display: flex;
        align-items: center;
        gap: 10px;
    }

    .label {
        font-weight: 600;
        color: #666;
    }

    .value {
        color: #333;
        font-family: 'SF Mono', Monaco, 'Cascadia Code', monospace;
    }

    .command {
        background: #f0f0f0;
        padding: 2px 8px;
        border-radius: 4px;
    }

    .status-running {
        color: #10b981;
        font-weight: 600;
    }

    .status-stopped {
        color: #ef4444;
        font-weight: 600;
    }

    .terminal-container {
        background: #1e1e1e;
        border-radius: 8px;
        padding: 15px;
    }

    #terminal {
        height: 500px;
    }

    .button-group {
        display: flex;
        gap: 10px;
        flex-wrap: wrap;
    }

    .btn {
        padding: 10px 20px;
        border: none;
        border-radius: 6px;
        font-weight: 600;
        cursor: pointer;
        transition: all 0.2s;
    }

    .btn-primary {
        background: #667eea;
        color: white;
    }

    .btn-primary:hover {
        background: #5a67d8;
    }

    .btn-secondary {
        background: #6b7280;
        color: white;
    }

    .btn-secondary:hover {
        background: #4b5563;
    }

    .btn-danger {
        background: #ef4444;
        color: white;
    }

    .btn-danger:hover {
        background: #dc2626;
    }

    .api-section {
        margin-bottom: 20px;
    }

    .api-list {
        list-style: none;
        padding-left: 0;
    }

    .api-list li {
        padding: 8px 0;
        border-bottom: 1px solid #f0f0f0;
    }

    .api-list li:last-child {
        border-bottom: none;
    }

    code {
        background: #f4f4f4;
        padding: 2px 6px;
        border-radius: 3px;
        font-family: 'SF Mono', Monaco, 'Cascadia Code', monospace;
        color: #e11d48;
    }
"#;

const JAVASCRIPT: &str = r#"
    let ws = null;
    let term = null;
    let fitAddon = null;

    function initTerminal() {
        term = new Terminal({
            cursorBlink: true,
            fontSize: 14,
            fontFamily: "'SF Mono', Monaco, 'Cascadia Code', monospace",
            theme: {
                background: '#000000',
                foreground: '#00ff00',
                cursor: '#00ff00',
                cursorAccent: '#000000',
                selection: '#00ff0044',
            }
        });

        fitAddon = new FitAddon.FitAddon();
        term.loadAddon(fitAddon);

        const webLinksAddon = new WebLinksAddon.WebLinksAddon();
        term.loadAddon(webLinksAddon);

        term.open(document.getElementById('terminal'));
        fitAddon.fit();

        // Handle terminal input
        term.onData(data => {
            if (ws && ws.readyState === WebSocket.OPEN) {
                ws.send(JSON.stringify({ type: 'Input', data: data }));
            }
        });

        // Handle resize
        window.addEventListener('resize', () => {
            if (fitAddon) {
                fitAddon.fit();
                // Send new size to backend
                if (ws && ws.readyState === WebSocket.OPEN && term) {
                    const cols = term.cols;
                    const rows = term.rows;
                    fetch('/api/resize', {
                        method: 'POST',
                        headers: { 'Content-Type': 'application/json' },
                        body: JSON.stringify({ rows, cols })
                    }).catch(err => console.error('Failed to resize PTY:', err));
                }
            }
        });

        // Also resize on terminal open
        term.onResize(({ cols, rows }) => {
            if (ws && ws.readyState === WebSocket.OPEN) {
                fetch('/api/resize', {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({ rows, cols })
                }).catch(err => console.error('Failed to resize PTY:', err));
            }
        });
    }

    function connectWebSocket() {
        const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
        const wsUrl = `${protocol}//${window.location.host}/api/ws`;

        ws = new WebSocket(wsUrl);

        ws.onopen = () => {
            console.log('WebSocket connected');
            if (term) {
                term.write('\r\n\x1b[32mConnected to process\x1b[0m\r\n');
            }
        };

        ws.onmessage = (event) => {
            try {
                const msg = JSON.parse(event.data);
                if (msg.type === 'Output' && term) {
                    term.write(msg.data);
                }
            } catch (e) {
                console.error('Failed to parse message:', e);
            }
        };

        ws.onerror = (error) => {
            console.error('WebSocket error:', error);
            if (term) {
                term.write('\r\n\x1b[31m[Connection error]\x1b[0m\r\n');
            }
        };

        ws.onclose = () => {
            console.log('WebSocket disconnected');
            if (term) {
                term.write('\r\n\x1b[31m[Disconnected - reconnecting...]\x1b[0m\r\n');
            }
            // Try to reconnect after 2 seconds
            setTimeout(connectWebSocket, 2000);
        };
    }

    // Event listeners for controls
    document.addEventListener('DOMContentLoaded', () => {
        initTerminal();
        connectWebSocket();

        document.getElementById('refresh').addEventListener('click', async () => {
            try {
                const response = await fetch('/api/output');
                const data = await response.json();
                if (term) {
                    term.clear();
                    term.write(data.lines.join('\r\n'));
                }
            } catch (error) {
                console.error('Failed to refresh output:', error);
            }
        });

        document.getElementById('clear').addEventListener('click', () => {
            if (term) {
                term.clear();
            }
        });

        const killBtn = document.getElementById('kill');
        if (killBtn) {
            killBtn.addEventListener('click', async () => {
                if (!confirm('Are you sure you want to kill the process?')) return;

                try {
                    await fetch('/api/kill', {
                        method: 'POST',
                        headers: { 'Content-Type': 'application/json' },
                        body: JSON.stringify({ signal: 'SIGTERM' })
                    });
                    location.reload();
                } catch (error) {
                    console.error('Failed to kill process:', error);
                }
            });
        }
    });
"#;
