use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tracing::info;

// =============================================================================
// Unified config (figment-deserialized from defaults / config.toml / env vars)
// =============================================================================
//
// Three equivalent ways to configure:
//
//   config.toml:     [auth]
//                    enabled = true
//
//   env var:         CRAB_AUTH__ENABLED=true   (double underscore = nesting)
//
//   (single underscore stays within field names: CRAB_AUTH__SESSION_TTL_SECS)

/// Top-level tunable configuration, deserialized by figment.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct FileConfig {
    #[serde(default)]
    pub auth: AuthFileConfig,
    #[serde(default)]
    pub server: ServerFileConfig,
}

/// Auth-related tunables (lives under `[auth]` in config.toml).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AuthFileConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_session_ttl")]
    pub session_ttl_secs: u64,
    #[serde(default = "default_allow_registration")]
    pub allow_registration: bool,
    #[serde(default)]
    pub https: bool,
}

impl Default for AuthFileConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            session_ttl_secs: default_session_ttl(),
            allow_registration: default_allow_registration(),
            https: false,
        }
    }
}

/// Server tuning knobs (lives under `[server]` in config.toml).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ServerFileConfig {
    #[serde(default = "default_max_buffer_mb")]
    pub max_buffer_mb: usize,
    #[serde(default = "default_max_history_kb")]
    pub max_history_kb: usize,
    #[serde(default = "default_hang_timeout_secs")]
    pub hang_timeout_secs: u64,
}

impl Default for ServerFileConfig {
    fn default() -> Self {
        Self {
            max_buffer_mb: default_max_buffer_mb(),
            max_history_kb: default_max_history_kb(),
            hang_timeout_secs: default_hang_timeout_secs(),
        }
    }
}

fn default_session_ttl() -> u64 {
    604800
}
fn default_allow_registration() -> bool {
    true
}
fn default_max_buffer_mb() -> usize {
    25
}
fn default_max_history_kb() -> usize {
    64
}
fn default_hang_timeout_secs() -> u64 {
    300
}

/// Build a figment that layers: defaults → config.toml → CRAB_* env vars.
///
/// Env vars use double-underscore for nesting into sections:
///   `CRAB_AUTH__ENABLED=true`  →  `auth.enabled = true`
///   `CRAB_SERVER__MAX_BUFFER_MB=50`  →  `server.max_buffer_mb = 50`
pub fn load_config(data_dir: &Path) -> figment::Figment {
    use figment::{
        Figment,
        providers::{Env, Format, Serialized, Toml},
    };

    Figment::from(Serialized::defaults(FileConfig::default()))
        .merge(Toml::file(data_dir.join("config.toml")))
        .merge(Env::prefixed("CRAB_").split("__"))
}

// =============================================================================
// Runtime config structs (derived from FileConfig, used throughout the server)
// =============================================================================

/// Authentication configuration (runtime view).
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

impl AuthConfig {
    pub fn from_file(fc: &AuthFileConfig) -> Self {
        Self {
            enabled: fc.enabled,
            session_ttl_secs: fc.session_ttl_secs,
            allow_registration: fc.allow_registration,
            https: fc.https,
        }
    }
}

/// Server configuration for runtime behavior.
#[derive(Clone, Debug)]
pub struct ServerConfig {
    /// Instance-related settings
    pub instance: InstanceConfig,
    /// WebSocket-related settings
    pub websocket: WebSocketConfig,
    /// State detection settings
    #[allow(dead_code)]
    pub state: StateConfig,
}

#[derive(Clone, Debug)]
pub struct InstanceConfig {
    /// Maximum output buffer per instance in bytes
    pub max_buffer_bytes: usize,
    /// Consider instance hung after this duration without output (None = disabled)
    #[allow(dead_code)]
    pub hang_timeout: Option<Duration>,
    /// Number of PTY spawn retries
    #[allow(dead_code)]
    pub spawn_retries: usize,
}

#[derive(Clone, Debug)]
pub struct WebSocketConfig {
    /// Channel capacity for messages to client
    #[allow(dead_code)]
    pub send_channel_capacity: usize,
    /// Broadcast channel capacity for state updates
    #[allow(dead_code)]
    pub state_broadcast_capacity: usize,
    /// Maximum history bytes to send on focus switch
    pub max_history_replay_bytes: usize,
}

#[derive(Clone, Debug)]
pub struct StateConfig {
    /// Idle timeout for staleness detection
    #[allow(dead_code)]
    pub idle_timeout: Duration,
    /// Conversation poll interval
    #[allow(dead_code)]
    pub poll_interval: Duration,
}

impl ServerConfig {
    pub fn from_file(fc: &ServerFileConfig) -> Self {
        Self {
            instance: InstanceConfig {
                max_buffer_bytes: fc.max_buffer_mb * 1024 * 1024,
                hang_timeout: if fc.hang_timeout_secs == 0 {
                    None
                } else {
                    Some(Duration::from_secs(fc.hang_timeout_secs))
                },
                spawn_retries: 2,
            },
            websocket: WebSocketConfig {
                send_channel_capacity: 100,
                state_broadcast_capacity: 256,
                max_history_replay_bytes: fc.max_history_kb * 1024,
            },
            state: StateConfig {
                idle_timeout: Duration::from_secs(10),
                poll_interval: Duration::from_millis(500),
            },
        }
    }
}

// =============================================================================
// Directory layout config (not tunable via figment — derived from --data-dir)
// =============================================================================

#[derive(Clone, Debug)]
pub struct CrabCityConfig {
    pub data_dir: PathBuf,
    pub db_path: PathBuf,
    #[allow(dead_code)]
    pub exports_dir: PathBuf,
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

        let exports_dir = data_dir.join("exports");
        std::fs::create_dir_all(&exports_dir)
            .with_context(|| format!("Failed to create exports directory: {:?}", exports_dir))?;

        let logs_dir = data_dir.join("logs");
        std::fs::create_dir_all(&logs_dir)
            .with_context(|| format!("Failed to create logs directory: {:?}", logs_dir))?;

        let db_path = data_dir.join("crabcity.db");

        info!("Data directory: {}", data_dir.display());

        Ok(Self {
            data_dir,
            db_path,
            exports_dir,
            logs_dir,
        })
    }

    pub fn db_url(&self) -> String {
        format!("sqlite://{}?mode=rwc", self.db_path.display())
    }

    pub fn reset_database(&self) -> Result<()> {
        if self.db_path.exists() {
            std::fs::remove_file(&self.db_path)
                .with_context(|| format!("Failed to delete database: {:?}", self.db_path))?;
            info!("Database reset: {:?}", self.db_path);

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

    pub fn config_toml_path(&self) -> PathBuf {
        self.data_dir.join("config.toml")
    }
}
