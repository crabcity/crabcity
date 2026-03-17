use anyhow::{Context, Result};

use crate::config::CrabCityConfig;

#[derive(Debug, Clone)]
pub struct DaemonInfo {
    pub pid: u32,
    pub port: u16,
    pub host: String,
}

impl DaemonInfo {
    pub fn base_url(&self) -> String {
        format!("http://{}:{}", self.host, self.port)
    }
}

/// Check if a daemon is already running by reading PID/port files and
/// verifying the process is alive.
pub fn check_daemon(config: &CrabCityConfig) -> Option<DaemonInfo> {
    let pid_path = config.daemon_pid_path();
    let port_path = config.daemon_port_path();

    let pid_str = std::fs::read_to_string(&pid_path).ok()?;
    let pid: u32 = pid_str.trim().parse().ok()?;

    // Verify process is alive (kill with signal 0 = check existence)
    #[cfg(unix)]
    {
        use nix::sys::signal;
        use nix::unistd::Pid;
        if signal::kill(Pid::from_raw(pid as i32), None).is_err() {
            // Process is dead, clean up stale files
            let _ = std::fs::remove_file(&pid_path);
            let _ = std::fs::remove_file(&port_path);
            return None;
        }
    }

    let port_str = std::fs::read_to_string(&port_path).ok()?;
    let port: u16 = port_str.trim().parse().ok()?;

    Some(DaemonInfo {
        pid,
        port,
        host: "127.0.0.1".to_string(),
    })
}

/// Start a new daemon process in the background.
///
/// Uses `which crab` to find the server binary on PATH (unlike the CLI which
/// uses `current_exe()`). This keeps the Tauri app decoupled from the server
/// binary location.
pub fn start_daemon(config: &CrabCityConfig) -> Result<()> {
    let crab_exe = which::which("crab")
        .context("Could not find 'crab' on PATH. Install crab_city or add it to your PATH.")?;

    std::fs::create_dir_all(&config.logs_dir)?;

    let log_file = std::fs::File::create(config.daemon_log_path())
        .context("Failed to create daemon log file")?;
    let err_file = std::fs::File::create(config.daemon_err_path())
        .context("Failed to create daemon error log file")?;

    let mut cmd = std::process::Command::new(&crab_exe);
    cmd.arg("server")
        .arg("--port")
        .arg("0")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::from(log_file))
        .stderr(std::process::Stdio::from(err_file));

    // Pass data-dir if non-default
    let default_data_dir = dirs::home_dir()
        .expect("Could not find home directory")
        .join(".crabcity");
    if config.data_dir != default_data_dir {
        cmd.arg("--data-dir").arg(&config.data_dir);
    }

    // Create a new session so the daemon survives if the Tauri app exits
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            cmd.pre_exec(|| {
                nix::libc::setsid();
                Ok(())
            });
        }
    }

    cmd.spawn().context("Failed to spawn daemon process")?;

    Ok(())
}

/// Health check the daemon via GET /health.
pub async fn health_check(info: &DaemonInfo) -> bool {
    let url = format!("{}/health", info.base_url());
    match reqwest::get(&url).await {
        Ok(resp) => resp.status().is_success(),
        Err(_) => false,
    }
}

/// Ensure a daemon is running. Start one if needed, then poll for readiness.
/// Returns `(DaemonInfo, we_started)` — the bool indicates whether we spawned
/// the daemon (and therefore should stop it on exit).
pub async fn ensure_daemon(config: &CrabCityConfig) -> Result<(DaemonInfo, bool)> {
    // Check if already running
    if let Some(info) = check_daemon(config) {
        if health_check(&info).await {
            return Ok((info, false));
        }
        // Process exists but not healthy — clean up and restart
        let _ = std::fs::remove_file(config.daemon_pid_path());
        let _ = std::fs::remove_file(config.daemon_port_path());
    }

    tracing::info!("Starting crab daemon...");
    start_daemon(config)?;

    // Poll for the daemon to become healthy
    let port_path = config.daemon_port_path();
    let start = std::time::Instant::now();
    let timeout = std::time::Duration::from_secs(10);
    let poll_interval = std::time::Duration::from_millis(100);

    loop {
        if start.elapsed() > timeout {
            anyhow::bail!(
                "Timed out waiting for daemon to start. Check logs at: {}",
                config.daemon_err_path().display()
            );
        }

        tokio::time::sleep(poll_interval).await;

        if let Some(info) = check_daemon(config) {
            if health_check(&info).await {
                tracing::info!("Daemon running on port {}", info.port);
                return Ok((info, true));
            }
        }

        // Also check if port file exists even if PID check fails (race condition)
        if port_path.exists() {
            if let Some(info) = check_daemon(config) {
                if health_check(&info).await {
                    tracing::info!("Daemon running on port {}", info.port);
                    return Ok((info, true));
                }
            }
        }
    }
}

/// Try to rediscover a running daemon from PID/port files on disk.
/// Used when the current DaemonInfo is stale (e.g. server restarted on a new port).
pub async fn rediscover_daemon(config: &CrabCityConfig) -> Option<DaemonInfo> {
    let info = check_daemon(config)?;
    if health_check(&info).await {
        Some(info)
    } else {
        None
    }
}

/// Send SIGTERM to the daemon process for a graceful shutdown.
pub fn stop_daemon(info: &DaemonInfo) {
    #[cfg(unix)]
    {
        use nix::sys::signal::{self, Signal};
        use nix::unistd::Pid;
        let _ = signal::kill(Pid::from_raw(info.pid as i32), Signal::SIGTERM);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a throwaway `CrabCityConfig` rooted in a temp directory.
    fn temp_config() -> (CrabCityConfig, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path().to_path_buf();
        let logs_dir = data_dir.join("logs");
        std::fs::create_dir_all(&logs_dir).unwrap();
        std::fs::create_dir_all(data_dir.join("state")).unwrap();
        let config = CrabCityConfig { data_dir, logs_dir };
        (config, tmp)
    }

    /// Write daemon PID and port files (test helper).
    fn write_daemon_files(config: &CrabCityConfig, pid: u32, port: u16) {
        std::fs::write(config.daemon_pid_path(), pid.to_string()).unwrap();
        std::fs::write(config.daemon_port_path(), port.to_string()).unwrap();
    }

    /// Spawn a minimal HTTP server that responds 200 on /health.
    async fn spawn_health_server() -> (u16, tokio::sync::oneshot::Sender<()>) {
        use axum::{Router, routing::get};

        let app = Router::new().route("/health", get(|| async { "ok" }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = rx.await;
                })
                .await
                .unwrap();
        });
        (port, tx)
    }

    // -- DaemonInfo URL builders --

    fn make_daemon_info(host: &str, port: u16) -> DaemonInfo {
        DaemonInfo {
            pid: 12345,
            port,
            host: host.to_string(),
        }
    }

    #[test]
    fn base_url_formats_correctly() {
        let info = make_daemon_info("127.0.0.1", 9000);
        assert_eq!(info.base_url(), "http://127.0.0.1:9000");
    }

    #[test]
    fn base_url_custom_host() {
        let info = make_daemon_info("0.0.0.0", 8080);
        assert_eq!(info.base_url(), "http://0.0.0.0:8080");
    }

    // -- check_daemon --

    #[test]
    fn check_daemon_returns_none_when_no_files() {
        let (config, _tmp) = temp_config();
        assert!(check_daemon(&config).is_none());
    }

    #[test]
    fn check_daemon_cleans_up_stale_pid() {
        let (config, _tmp) = temp_config();
        // PID 999999999 is almost certainly not alive
        write_daemon_files(&config, 999_999_999, 8080);

        assert!(check_daemon(&config).is_none());
        // Stale files should have been cleaned up
        assert!(!config.daemon_pid_path().exists());
        assert!(!config.daemon_port_path().exists());
    }

    #[test]
    fn check_daemon_returns_info_for_live_process() {
        let (config, _tmp) = temp_config();
        // Use our own PID — guaranteed alive
        let pid = std::process::id();
        write_daemon_files(&config, pid, 9876);

        let info = check_daemon(&config).unwrap();
        assert_eq!(info.pid, pid);
        assert_eq!(info.port, 9876);
        assert_eq!(info.host, "127.0.0.1");
    }

    // -- health_check --

    #[tokio::test]
    async fn health_check_returns_false_for_unreachable() {
        let info = make_daemon_info("127.0.0.1", 1); // port 1: nothing there
        assert!(!health_check(&info).await);
    }

    #[tokio::test]
    async fn health_check_returns_true_for_healthy_server() {
        let (port, _shutdown) = spawn_health_server().await;
        let info = make_daemon_info("127.0.0.1", port);
        assert!(health_check(&info).await);
    }

    // -- rediscover_daemon --

    #[tokio::test]
    async fn rediscover_finds_healthy_daemon() {
        let (config, _tmp) = temp_config();
        let (port, _shutdown) = spawn_health_server().await;
        let pid = std::process::id();
        write_daemon_files(&config, pid, port);

        let info = rediscover_daemon(&config).await;
        assert!(info.is_some());
        assert_eq!(info.unwrap().port, port);
    }

    #[tokio::test]
    async fn rediscover_returns_none_when_no_files() {
        let (config, _tmp) = temp_config();
        assert!(rediscover_daemon(&config).await.is_none());
    }

    #[tokio::test]
    async fn rediscover_returns_none_when_server_dead() {
        let (config, _tmp) = temp_config();
        let pid = std::process::id();
        write_daemon_files(&config, pid, 1); // port 1: nothing there

        assert!(rediscover_daemon(&config).await.is_none());
    }

    #[tokio::test]
    async fn rediscover_picks_up_new_port_after_restart() {
        let (config, _tmp) = temp_config();
        let pid = std::process::id();

        let (port1, shutdown1) = spawn_health_server().await;
        write_daemon_files(&config, pid, port1);
        assert_eq!(rediscover_daemon(&config).await.unwrap().port, port1);

        // "Restart": shut down old server, start new one
        let _ = shutdown1.send(());
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let (port2, _shutdown2) = spawn_health_server().await;
        assert_ne!(port1, port2);
        write_daemon_files(&config, pid, port2);

        assert_eq!(rediscover_daemon(&config).await.unwrap().port, port2);
    }
}
