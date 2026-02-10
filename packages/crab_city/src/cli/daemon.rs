use anyhow::{Context, Result};

use crate::config::CrabCityConfig;

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct DaemonInfo {
    pub pid: u32,
    pub port: u16,
    pub host: String,
}

impl DaemonInfo {
    pub fn base_url(&self) -> String {
        format!("http://{}:{}", self.host, self.port)
    }

    pub fn ws_url(&self, instance_id: &str) -> String {
        format!(
            "ws://{}:{}/api/instances/{}/ws",
            self.host, self.port, instance_id
        )
    }

    pub fn mux_ws_url(&self) -> String {
        format!("ws://{}:{}/api/ws", self.host, self.port)
    }
}

/// Check if a daemon is already running by reading PID/port files and verifying the process.
pub fn check_daemon(config: &CrabCityConfig) -> Option<DaemonInfo> {
    let pid_path = config.daemon_pid_path();
    let port_path = config.daemon_port_path();

    // Read PID file
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

    // Read port file
    let port_str = std::fs::read_to_string(&port_path).ok()?;
    let port: u16 = port_str.trim().parse().ok()?;

    Some(DaemonInfo {
        pid,
        port,
        host: "127.0.0.1".to_string(),
    })
}

/// Start a new daemon process in the background.
pub fn start_daemon(config: &CrabCityConfig) -> Result<()> {
    let exe = std::env::current_exe().context("Failed to determine current executable")?;

    // Ensure log directory exists
    std::fs::create_dir_all(&config.logs_dir)?;

    let log_file = std::fs::File::create(config.daemon_log_path())
        .context("Failed to create daemon log file")?;
    let err_file = std::fs::File::create(config.daemon_err_path())
        .context("Failed to create daemon error log file")?;

    let mut cmd = std::process::Command::new(&exe);
    cmd.arg("server")
        .arg("--port")
        .arg("0") // auto-select port
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

    // On Unix, create a new session so the daemon doesn't die with the terminal
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

/// Ensure a daemon is running. Start one if needed, then wait for it to be healthy.
pub async fn ensure_daemon(config: &CrabCityConfig) -> Result<DaemonInfo> {
    // First check if already running
    if let Some(info) = check_daemon(config) {
        // Verify it's actually healthy
        if health_check(&info).await {
            return Ok(info);
        }
        // Process exists but not healthy - clean up and restart
        let _ = std::fs::remove_file(config.daemon_pid_path());
        let _ = std::fs::remove_file(config.daemon_port_path());
    }

    eprintln!("Starting crab daemon...");
    start_daemon(config)?;

    // Poll for the port file to appear (daemon writes it after binding)
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
                eprintln!("Daemon running on port {}", info.port);
                return Ok(info);
            }
        }

        // Also check if port file exists even if PID check fails (race condition)
        if port_path.exists() {
            if let Some(info) = check_daemon(config) {
                if health_check(&info).await {
                    eprintln!("Daemon running on port {}", info.port);
                    return Ok(info);
                }
            }
        }
    }
}

/// Health check the daemon via GET /health.
async fn health_check(info: &DaemonInfo) -> bool {
    let url = format!("{}/health", info.base_url());
    match reqwest::get(&url).await {
        Ok(resp) => resp.status().is_success(),
        Err(_) => false,
    }
}

/// Clean up daemon PID and port files.
pub fn cleanup_daemon_files(config: &CrabCityConfig) {
    let _ = std::fs::remove_file(config.daemon_pid_path());
    let _ = std::fs::remove_file(config.daemon_port_path());
}

/// Write daemon PID and port files after the server binds.
pub fn write_daemon_files(config: &CrabCityConfig, pid: u32, port: u16) -> Result<()> {
    std::fs::write(config.daemon_pid_path(), pid.to_string())
        .context("Failed to write daemon PID file")?;
    std::fs::write(config.daemon_port_path(), port.to_string())
        .context("Failed to write daemon port file")?;
    Ok(())
}
