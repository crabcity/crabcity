use anyhow::{Context, Result};
use std::path::PathBuf;
use std::time::Duration;
use tracing::info;

/// Server configuration for runtime behavior
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct ServerConfig {
    /// Instance-related settings
    pub instance: InstanceConfig,
    /// WebSocket-related settings
    pub websocket: WebSocketConfig,
    /// State detection settings
    pub state: StateConfig,
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct InstanceConfig {
    /// Maximum output buffer per instance in bytes (default: 1MB)
    pub max_buffer_bytes: usize,
    /// Consider instance hung after this duration without output (None = disabled)
    pub hang_timeout: Option<Duration>,
    /// Number of PTY spawn retries
    pub spawn_retries: usize,
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct WebSocketConfig {
    /// Channel capacity for messages to client
    pub send_channel_capacity: usize,
    /// Broadcast channel capacity for state updates
    pub state_broadcast_capacity: usize,
    /// Maximum history bytes to send on focus switch (default: 64KB)
    pub max_history_replay_bytes: usize,
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct StateConfig {
    /// Idle timeout for staleness detection
    pub idle_timeout: Duration,
    /// Conversation poll interval
    pub poll_interval: Duration,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            instance: InstanceConfig {
                max_buffer_bytes: 25 * 1024 * 1024,           // 25MB
                hang_timeout: Some(Duration::from_secs(300)), // 5 minutes
                spawn_retries: 2,
            },
            websocket: WebSocketConfig {
                send_channel_capacity: 100,
                state_broadcast_capacity: 256,
                max_history_replay_bytes: 64 * 1024, // 64KB
            },
            state: StateConfig {
                idle_timeout: Duration::from_secs(10),
                poll_interval: Duration::from_millis(500),
            },
        }
    }
}

impl ServerConfig {
    /// Create config from environment variables (with defaults)
    pub fn from_env() -> Self {
        let mut config = Self::default();

        if let Ok(val) = std::env::var("CRAB_CITY_MAX_BUFFER_MB") {
            if let Ok(mb) = val.parse::<usize>() {
                config.instance.max_buffer_bytes = mb * 1024 * 1024;
            }
        }

        if let Ok(val) = std::env::var("CRAB_CITY_MAX_HISTORY_KB") {
            if let Ok(kb) = val.parse::<usize>() {
                config.websocket.max_history_replay_bytes = kb * 1024;
            }
        }

        if let Ok(val) = std::env::var("CRAB_CITY_HANG_TIMEOUT_SECS") {
            if let Ok(secs) = val.parse::<u64>() {
                config.instance.hang_timeout = if secs == 0 {
                    None
                } else {
                    Some(Duration::from_secs(secs))
                };
            }
        }

        config
    }
}

/// Authentication configuration
#[derive(Clone, Debug)]
pub struct AuthConfig {
    /// Whether authentication is enabled
    pub enabled: bool,
    /// Session time-to-live in seconds (default: 7 days)
    pub session_ttl_secs: u64,
    /// Whether new user registration is open (default: true)
    pub allow_registration: bool,
    /// Whether to set Secure flag on cookies
    pub https: bool,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            session_ttl_secs: 604800, // 7 days
            allow_registration: true,
            https: false,
        }
    }
}

impl AuthConfig {
    /// Create auth config from environment variables
    pub fn from_env() -> Self {
        let mut config = Self::default();

        if let Ok(val) = std::env::var("CRAB_CITY_AUTH_ENABLED") {
            config.enabled = val == "true" || val == "1";
        }

        if let Ok(val) = std::env::var("CRAB_CITY_SESSION_TTL") {
            if let Ok(secs) = val.parse::<u64>() {
                config.session_ttl_secs = secs;
            }
        }

        if let Ok(val) = std::env::var("CRAB_CITY_ALLOW_REGISTRATION") {
            config.allow_registration = val != "false" && val != "0";
        }

        if let Ok(val) = std::env::var("CRAB_CITY_HTTPS") {
            config.https = val == "true" || val == "1";
        }

        config
    }
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct CrabCityConfig {
    pub data_dir: PathBuf,
    pub db_path: PathBuf,
    pub exports_dir: PathBuf,
    pub logs_dir: PathBuf,
}

impl CrabCityConfig {
    pub fn new(custom_dir: Option<PathBuf>) -> Result<Self> {
        // Use custom dir, or default to ~/.crabcity
        let data_dir = custom_dir.unwrap_or_else(|| {
            dirs::home_dir()
                .expect("Could not find home directory")
                .join(".crabcity")
        });

        // Create directory structure
        std::fs::create_dir_all(&data_dir)
            .with_context(|| format!("Failed to create data directory: {:?}", data_dir))?;

        let exports_dir = data_dir.join("exports");
        std::fs::create_dir_all(&exports_dir)
            .with_context(|| format!("Failed to create exports directory: {:?}", exports_dir))?;

        let logs_dir = data_dir.join("logs");
        std::fs::create_dir_all(&logs_dir)
            .with_context(|| format!("Failed to create logs directory: {:?}", logs_dir))?;

        let db_path = data_dir.join("crabcity.db");

        info!("📁 Data directory: {}", data_dir.display());

        Ok(Self {
            data_dir,
            db_path,
            exports_dir,
            logs_dir,
        })
    }

    pub fn db_url(&self) -> String {
        // Enable WAL mode and foreign keys
        // Note: We'll set additional pragmas after connection
        format!("sqlite://{}?mode=rwc", self.db_path.display())
    }

    pub fn reset_database(&self) -> Result<()> {
        if self.db_path.exists() {
            std::fs::remove_file(&self.db_path)
                .with_context(|| format!("Failed to delete database: {:?}", self.db_path))?;
            info!("Database reset: {:?}", self.db_path);

            // Also remove WAL and SHM files if they exist
            let wal_path = self.db_path.with_extension("db-wal");
            if wal_path.exists() {
                std::fs::remove_file(&wal_path)?;
            }
            let shm_path = self.db_path.with_extension("db-shm");
            if shm_path.exists() {
                std::fs::remove_file(&shm_path)?;
            }
        }
        Ok(())
    }

    pub fn daemon_pid_path(&self) -> PathBuf {
        self.data_dir.join("daemon.pid")
    }

    pub fn daemon_port_path(&self) -> PathBuf {
        self.data_dir.join("daemon.port")
    }

    pub fn daemon_log_path(&self) -> PathBuf {
        self.logs_dir.join("daemon.log")
    }

    pub fn daemon_err_path(&self) -> PathBuf {
        self.logs_dir.join("daemon.err")
    }
}
