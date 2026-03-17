use std::path::PathBuf;

use anyhow::{Context, Result};

/// Minimal config for locating daemon files. Mirrors the path layout of
/// `crab_city::config::CrabCityConfig` without pulling in the full server
/// config stack.
#[derive(Clone, Debug)]
pub struct CrabCityConfig {
    pub data_dir: PathBuf,
    pub logs_dir: PathBuf,
}

impl CrabCityConfig {
    pub fn new(custom_dir: Option<PathBuf>) -> Result<Self> {
        let data_dir = custom_dir.unwrap_or_else(|| {
            dirs::home_dir()
                .expect("Could not find home directory")
                .join(".crabcity")
        });

        std::fs::create_dir_all(&data_dir)
            .with_context(|| format!("Failed to create data directory: {:?}", data_dir))?;

        let logs_dir = data_dir.join("logs");
        std::fs::create_dir_all(&logs_dir)
            .with_context(|| format!("Failed to create logs directory: {:?}", logs_dir))?;

        let state_dir = data_dir.join("state");
        std::fs::create_dir_all(&state_dir)
            .with_context(|| format!("Failed to create state directory: {:?}", state_dir))?;

        Ok(Self { data_dir, logs_dir })
    }

    pub fn state_dir(&self) -> PathBuf {
        self.data_dir.join("state")
    }

    pub fn daemon_pid_path(&self) -> PathBuf {
        self.state_dir().join("daemon.pid")
    }

    pub fn daemon_port_path(&self) -> PathBuf {
        self.state_dir().join("daemon.port")
    }

    pub fn daemon_log_path(&self) -> PathBuf {
        self.logs_dir.join("daemon.log")
    }

    pub fn daemon_err_path(&self) -> PathBuf {
        self.logs_dir.join("daemon.err")
    }
}
