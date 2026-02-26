/// Crab binary version. Bump this when releasing.
pub const VERSION: &str = "0.42.0";

use anyhow::Result;
use axum::{
    Router,
    routing::{delete, get, patch, post},
};
use clap::{Parser, Subcommand};
use instance_manager::InstanceManager;
use std::{net::SocketAddr, sync::Arc};
use tower_http::cors::CorsLayer;
use tower_http::trace::MakeSpan;
use tower_http::trace::TraceLayer;
use tracing::{info, warn};
use tracing_subscriber::prelude::*;
use uuid::Uuid;

mod auth;
mod cli;
mod config;
mod db;
#[cfg(feature = "embedded-ui")]
mod embedded_ui;
mod files;
mod git;
mod handlers;
mod identity;
mod import;
mod inference;
mod instance_actor;
mod instance_manager;
mod interconnect;
mod metrics;
mod models;
mod notes;
mod persistence;
mod repository;
mod terminal;
mod transport;
mod views;
mod virtual_terminal;
mod ws;

#[cfg(test)]
mod test_helpers;

use std::collections::HashMap;
use std::path::PathBuf;
use tokio::sync::Mutex;

use crate::auth::AuthState;
use crate::config::{
    AuthConfig, CrabCityConfig, FileConfig, Profile, RuntimeOverrides, ServerConfig,
    TransportConfig, load_config,
};
use crate::identity::InstanceIdentity;

use crate::db::Database;
use crate::metrics::ServerMetrics;
use crate::persistence::{InstancePersistor, PersistenceService};
use crate::repository::ConversationRepository;

/// Custom span maker that adds a unique request ID to each incoming request
#[derive(Clone)]
struct RequestIdMakeSpan;

impl<B> MakeSpan<B> for RequestIdMakeSpan {
    fn make_span(&mut self, request: &axum::http::Request<B>) -> tracing::Span {
        let request_id = Uuid::new_v4().to_string();
        tracing::info_span!(
            "request",
            method = %request.method(),
            uri = %request.uri(),
            request_id = %request_id,
        )
    }
}

#[derive(Parser)]
#[command(name = "crab")]
#[command(version = VERSION)]
#[command(about = "Terminal multiplexer for Claude Code instances")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Custom data directory (defaults to ~/.crabcity)
    #[arg(long, global = true)]
    data_dir: Option<PathBuf>,
}

#[derive(Subcommand)]
enum Commands {
    /// Run the daemon server in the foreground
    Server(ServerArgs),

    /// Attach to an existing instance
    Attach(AttachArgs),

    /// List running instances
    List(ListArgs),

    /// Kill a specific session
    Kill(KillArgs),

    /// Stop the daemon and all sessions
    KillServer(KillServerArgs),

    /// Manage invite tokens
    Invite(InviteArgs),

    /// Connect to a remote crab instance
    Connect(ConnectArgs),

    /// List or switch between local and remote Crab City contexts
    Switch(SwitchArgs),
}

#[derive(Parser)]
struct ServerArgs {
    /// Port for the web server (0 = auto-select; overrides profile/config)
    #[arg(short, long)]
    port: Option<u16>,

    /// Host to bind to (overrides profile/config)
    #[arg(short = 'b', long)]
    host: Option<String>,

    /// Configuration profile (sets defaults for host/auth/https)
    #[arg(long, value_enum)]
    profile: Option<Profile>,

    /// Base port for Claude instances (will increment for each instance)
    #[arg(long, default_value = "9000")]
    instance_base_port: u16,

    /// Default command to run for new instances
    #[arg(long)]
    default_command: Option<String>,

    /// Enable debug logging
    #[arg(short, long)]
    debug: bool,

    /// Clean start - reset database (prompt for confirmation)
    #[arg(long)]
    reset_db: bool,

    /// Import all existing Claude conversations from the system
    #[arg(long)]
    import_all: bool,

    /// Import conversations from a specific project directory
    #[arg(long)]
    import_from: Option<PathBuf>,
}

#[derive(Parser)]
struct AttachArgs {
    /// Instance name, ID, or ID prefix to attach to (default: most recent)
    target: Option<String>,
}

#[derive(Parser)]
struct ListArgs {
    /// Output as JSON
    #[arg(long)]
    json: bool,
}

#[derive(Parser)]
struct KillArgs {
    /// Instance name, ID, or ID prefix to kill
    target: String,
}

#[derive(Parser)]
struct KillServerArgs {
    /// Skip confirmation prompt
    #[arg(short, long)]
    force: bool,
}

#[derive(Parser)]
struct InviteArgs {
    #[command(subcommand)]
    command: InviteCommands,
}

#[derive(Subcommand)]
enum InviteCommands {
    /// Create an invite token
    Create(InviteCreateArgs),
    /// List active invites
    List,
    /// Revoke an invite
    Revoke(InviteRevokeArgs),
}

#[derive(Parser)]
struct InviteCreateArgs {
    /// Capability level for the invite
    #[arg(short, long, default_value = "collaborate")]
    capability: String,
    /// Maximum number of uses (0 = unlimited)
    #[arg(short, long, default_value = "1")]
    max_uses: u32,
    /// Expiration in seconds (default: 1 hour)
    #[arg(short, long, default_value = "3600")]
    expires: u64,
    /// Human-readable label (e.g. --for alice)
    #[arg(short, long, alias = "for")]
    label: Option<String>,
}

#[derive(Parser)]
struct InviteRevokeArgs {
    /// Invite nonce (hex) to revoke
    nonce: String,
}

#[derive(Parser)]
struct ConnectArgs {
    /// Connection token (base32)
    token: Option<String>,
    /// Node public key (hex, 64 chars)
    #[arg(long)]
    node: Option<String>,
    /// Invite nonce (hex, 32 chars)
    #[arg(long)]
    invite: Option<String>,
    /// Relay URL hint (for private relays)
    #[arg(long)]
    relay: Option<String>,
    /// Display name
    #[arg(long)]
    name: Option<String>,
    /// Skip confirmation prompt
    #[arg(short, long)]
    yes: bool,
}

#[derive(Parser)]
struct SwitchArgs {
    /// Remote name to switch to (omit to list, "home" for local)
    target: Option<String>,
}

#[derive(Clone)]
#[allow(dead_code)]
pub(crate) struct AppState {
    pub instance_manager: Arc<InstanceManager>,
    /// Conversation watchers per instance (keyed by instance ID)
    pub conversation_watchers: Arc<Mutex<HashMap<String, claude_convo::ConversationWatcher>>>,
    pub config: Arc<CrabCityConfig>,
    /// Server runtime configuration
    pub server_config: Arc<ServerConfig>,
    /// Authentication configuration
    pub auth_config: Arc<AuthConfig>,
    /// Server metrics for observability
    pub metrics: Arc<ServerMetrics>,
    pub db: Arc<Database>,
    pub repository: Arc<ConversationRepository>,
    pub persistence_service: Arc<PersistenceService>,
    pub instance_persistors: Arc<Mutex<HashMap<String, Arc<InstancePersistor>>>>,
    pub notes_storage: Arc<notes::NotesStorage>,
    /// Global state manager for multiplexed WebSocket
    pub global_state_manager: Arc<ws::GlobalStateManager>,
    /// Ephemeral runtime overrides (from TUI/API)
    pub runtime_overrides: Arc<tokio::sync::RwLock<RuntimeOverrides>>,
    /// Instance identity (ed25519 keypair) for interconnect auth
    pub identity: Option<Arc<InstanceIdentity>>,
    /// The iroh node ID (ed25519 public key) when transport is active
    pub iroh_node_id: Option<[u8; 32]>,
    /// Human-readable instance name (for invite tokens and federation)
    pub instance_name: String,
    /// Federation connection manager (outbound tunnels to remote Crab Cities)
    pub connection_manager: Option<Arc<interconnect::manager::ConnectionManager>>,
    /// Send to trigger an HTTP server restart (config reload)
    pub restart_tx: Arc<tokio::sync::watch::Sender<()>>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let config = CrabCityConfig::new(cli.data_dir.clone())?;

    // Install crash diagnostics (terminal restore + crash report file)
    install_panic_hook(config.logs_dir.clone());

    // Initialize CLI-side file logging (server sets up its own in run_server)
    if !matches!(cli.command, Some(Commands::Server(_))) {
        init_cli_tracing(&config.logs_dir);
    }

    match cli.command {
        None => {
            // Bare `crab`: create new instance in cwd and attach
            cli::default_command(&config).await
        }
        Some(Commands::Attach(args)) => cli::attach_command(&config, args.target).await,
        Some(Commands::List(args)) => cli::list_command(&config, args.json).await,
        Some(Commands::Kill(args)) => cli::kill_command(&config, &args.target).await,
        Some(Commands::KillServer(args)) => cli::kill_server_command(&config, args.force).await,
        Some(Commands::Invite(args)) => {
            let daemon = cli::daemon::require_running_daemon(&config).await?;
            match args.command {
                InviteCommands::Create(create_args) => {
                    cli::invite::invite_create_command(
                        &daemon,
                        &create_args.capability,
                        create_args.max_uses,
                        Some(create_args.expires),
                        create_args.label.as_deref(),
                    )
                    .await?;
                    Ok(())
                }
                InviteCommands::List => {
                    cli::invite::invite_list_command(&daemon).await?;
                    Ok(())
                }
                InviteCommands::Revoke(revoke_args) => {
                    cli::invite::invite_revoke_command(&daemon, &revoke_args.nonce).await?;
                    Ok(())
                }
            }
        }
        Some(Commands::Connect(args)) => {
            cli::connect::connect_command(
                &config,
                args.token,
                args.node,
                args.invite,
                args.relay,
                args.name,
                args.yes,
            )
            .await
        }
        Some(Commands::Switch(args)) => {
            let daemon = cli::daemon::require_running_daemon(&config).await?;
            cli::switch::switch_command(&daemon, args.target.as_deref()).await
        }
        Some(Commands::Server(args)) => run_server(args, config).await,
    }
}

/// Install a panic hook that restores the terminal and saves a crash report.
fn install_panic_hook(logs_dir: std::path::PathBuf) {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // 1. Restore terminal — critical so the user's shell isn't left in raw mode.
        //    Use raw ANSI sequences to avoid depending on ratatui/crossterm state.
        let _ = ratatui::crossterm::terminal::disable_raw_mode();
        let mut stdout = std::io::stdout();
        // Leave alternate screen + show cursor
        let _ = std::io::Write::write_all(&mut stdout, b"\x1b[?1049l\x1b[?25h");
        let _ = std::io::Write::flush(&mut stdout);

        // 2. Capture backtrace (always, regardless of RUST_BACKTRACE)
        let backtrace = std::backtrace::Backtrace::force_capture();

        // 3. Write crash report
        let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
        let crash_path = logs_dir.join(format!("crash-{timestamp}.log"));
        if let Ok(mut f) = std::fs::File::create(&crash_path) {
            use std::io::Write;
            let _ = writeln!(f, "=== Crab City Crash Report ===");
            let _ = writeln!(f, "Time: {}", chrono::Local::now());
            let _ = writeln!(f, "Version: {VERSION}");
            let _ = writeln!(f, "");
            let _ = writeln!(f, "Panic: {info}");
            if let Some(loc) = info.location() {
                let _ = writeln!(
                    f,
                    "Location: {}:{}:{}",
                    loc.file(),
                    loc.line(),
                    loc.column()
                );
            }
            let _ = writeln!(f, "");
            let _ = writeln!(f, "Backtrace:\n{backtrace}");

            eprintln!("\n[crab] crash report saved to {}", crash_path.display());
        }

        // 4. Chain to default hook for the standard panic output
        default_hook(info);
    }));
}

/// Initialize file-based tracing for CLI commands (server has its own in run_server).
fn init_cli_tracing(logs_dir: &std::path::Path) {
    let log_path = logs_dir.join("cli.log");
    let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
    else {
        return; // best-effort — don't block CLI startup
    };

    let writer = std::sync::Mutex::new(file);
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("crab=debug,warn"));

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(writer)
                .with_ansi(false),
        )
        .with(env_filter)
        .init();
}

async fn run_server(args: ServerArgs, config: CrabCityConfig) -> Result<()> {
    // Setup logging
    let default_directive = if args.debug {
        "crab=debug,tower_http=debug,info"
    } else {
        "crab=info,tower_http=info,warn"
    };
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(default_directive));
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(env_filter)
        .init();

    info!("Starting Crab City - Claude Code Instance Manager");

    let config = Arc::new(config);

    // Handle database reset if requested
    if args.reset_db && config.db_path.exists() {
        println!("This will delete all stored conversations!");
        print!("Are you sure? (yes/no): ");
        use std::io::{self, Write};
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        if input.trim() == "yes" {
            config.reset_database()?;
            println!("Database reset.");
        } else {
            println!("Cancelled.");
        }
    }

    // === Init once: these survive across server restarts ===

    // Initialize database
    info!("Initializing database...");
    let db = Arc::new(Database::new(&config).await?);

    // Create repository and persistence service
    let repository = Arc::new(ConversationRepository::new(db.pool.clone()));
    let persistence_service = Arc::new(PersistenceService::new(repository.clone()));

    // Start persistence service background task
    persistence_service.clone().start().await;

    // Handle import - always run on startup to keep database in sync with JSONL files
    {
        let importer = import::ConversationImporter::new(repository.as_ref().clone());

        let stats = if let Some(project_path) = args.import_from {
            info!("Importing from project: {}", project_path.display());
            importer.import_from_project(&project_path).await?
        } else {
            info!("Syncing conversations from Claude Code...");
            importer.import_all_projects().await?
        };

        if stats.imported > 0 || stats.updated > 0 || stats.failed > 0 || args.import_all {
            info!("Import complete!");
            info!("   Imported: {} conversations", stats.imported);
            info!("   Updated:  {} (re-imported changed files)", stats.updated);
            info!("   Skipped:  {} (already imported)", stats.skipped);
            info!("   Failed:   {}", stats.failed);
            info!("   Total:    {} sessions processed", stats.total());
        } else if stats.skipped > 0 {
            info!("Database in sync ({} conversations)", stats.skipped);
        }
    }

    // Determine default command: try claude first, then parent shell, then bash
    let default_command = args.default_command.unwrap_or_else(|| {
        info!("Detecting available commands...");
        let claude_check = std::process::Command::new("which").arg("claude").output();

        if let Ok(output) = claude_check {
            if output.status.success() {
                let claude_path = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !claude_path.is_empty() {
                    info!("Found 'claude' command at: {}", claude_path);
                    return claude_path;
                }
            }
        }

        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
        info!("'claude' not found or inaccessible, using shell: {}", shell);
        shell
    });

    info!("Default command configured: {}", default_command);

    // Create instance manager (long-lived, survives restarts)
    let cli_profile = args.profile.clone();
    let fc_initial: FileConfig = load_config(&config.data_dir, cli_profile.as_ref())
        .extract()
        .unwrap_or_default();
    let initial_server_config = ServerConfig::from_file(&fc_initial.server);

    let instance_manager = Arc::new(InstanceManager::new(
        default_command,
        args.instance_base_port,
        initial_server_config.instance.max_buffer_bytes,
    ));

    // Initialize notes storage
    let notes_storage = Arc::new(notes::NotesStorage::new(&config.data_dir)?);

    // Initialize global state manager for multiplexed WebSocket
    let state_broadcast = ws::create_state_broadcast();
    let global_state_manager = Arc::new(ws::GlobalStateManager::new(state_broadcast));

    // Initialize metrics
    let metrics = Arc::new(ServerMetrics::new());

    // Shared mutable state across restarts
    let conversation_watchers = Arc::new(Mutex::new(HashMap::new()));
    let instance_persistors = Arc::new(Mutex::new(HashMap::new()));

    // Restart channel
    let (restart_tx, mut restart_rx) = tokio::sync::watch::channel(());
    let restart_tx = Arc::new(restart_tx);

    // Runtime overrides (ephemeral, from TUI/API)
    let runtime_overrides = Arc::new(tokio::sync::RwLock::new(RuntimeOverrides::default()));

    // Load or generate persistent instance identity (ed25519 keypair)
    let identity = Arc::new(InstanceIdentity::load_or_generate(&config.data_dir)?);
    info!("Instance identity: {}", identity.public_key.fingerprint());

    // Ensure the instance's own identity exists in member_identities.
    // This is required before any invite operations (the issuer FK).
    if repository
        .get_identity(identity.public_key.as_bytes())
        .await?
        .is_none()
    {
        repository
            .create_identity(identity.public_key.as_bytes(), "Instance Owner")
            .await?;
    }

    // First-run detection: if no active grants exist, generate an owner invite
    first_run_bootstrap(&identity, &repository).await;

    // Conditionally start iroh transport
    let transport_config = TransportConfig::from_file(&fc_initial.transport);
    let iroh_transport = if transport_config.enabled {
        let relay =
            transport::relay::EmbeddedRelay::start(transport_config.relay_bind_addr).await?;
        info!(
            "Embedded relay started on {}",
            transport_config.relay_bind_addr
        );

        // Decide relay mode: public relays (default) or private embedded relay only
        let relay_url = if transport_config.use_public_relays {
            None // RelayMode::Default — iroh's public relay network
        } else {
            Some(relay.url().clone()) // RelayMode::Custom — airgapped/private
        };

        let iroh = transport::iroh_transport::IrohTransport::start(
            identity.clone(),
            relay_url,
            repository.as_ref().clone(),
            global_state_manager.clone(),
            instance_manager.clone(),
            Some(Arc::new(ServerConfig::from_file(&fc_initial.server))),
            fc_initial.transport.instance_name.clone(),
        )
        .await?;

        info!(
            "iroh transport accepting connections at {:?}",
            iroh.endpoint_addr()
        );
        Some((iroh, relay))
    } else {
        None
    };

    // Start federation ConnectionManager if iroh is active
    let connection_manager = if let Some((ref iroh, _)) = iroh_transport {
        let mgr = Arc::new(interconnect::manager::ConnectionManager::new(
            iroh.endpoint().clone(),
            identity.clone(),
            fc_initial.transport.instance_name.clone(),
            repository.as_ref().clone(),
        ));
        if let Err(e) = mgr.start().await {
            warn!("Failed to start federation auto-connect: {}", e);
        }
        Some(mgr)
    } else {
        None
    };

    // CLI --host/--port override everything (applied after figment + runtime overrides)
    let cli_host = args.host.clone();
    let cli_port = args.port;
    // Track the actual host/port after first bind so restarts reuse the same address
    let mut bound_port: Option<u16> = None;
    let mut bound_host: Option<String> = None;

    // Clone references needed for shutdown cleanup
    let persistence_for_shutdown = persistence_service.clone();
    let instances_for_shutdown = instance_manager.clone();
    let config_for_shutdown = config.clone();

    // === Server loop: reload config and rebuild router on each iteration ===
    let mut first_iteration = true;
    loop {
        // Load config from figment (defaults → profile → config.toml → env vars)
        let fc: FileConfig = load_config(&config.data_dir, cli_profile.as_ref())
            .extract()
            .unwrap_or_default();
        let overrides = runtime_overrides.read().await.clone();

        // Resolve effective host/port: runtime overrides > CLI flags > figment config > defaults
        let effective_host = overrides
            .host
            .as_deref()
            .or(cli_host.as_deref())
            .or(fc.server.host.as_deref())
            .unwrap_or("127.0.0.1");
        let effective_port = overrides.port.or(cli_port).or(fc.server.port).unwrap_or(0);

        // Resolve effective auth/https with runtime overrides
        let mut auth_config_raw = AuthConfig::from_file(&fc.auth);
        if let Some(auth_enabled) = overrides.auth_enabled {
            auth_config_raw.enabled = auth_enabled;
        }
        if let Some(https) = overrides.https {
            auth_config_raw.https = https;
        }

        let server_config = Arc::new(ServerConfig::from_file(&fc.server));

        // If effective host changed from last iteration, force rebind
        let host_changed = bound_host.as_ref().map_or(false, |h| h != effective_host);
        if host_changed {
            bound_port = None;
        }

        info!(
            "Server config: max_history={}KB, max_buffer={}MB",
            server_config.websocket.max_history_replay_bytes / 1024,
            server_config.instance.max_buffer_bytes / (1024 * 1024)
        );

        let auth_config = Arc::new(auth_config_raw);

        let app_state = AppState {
            instance_manager: instance_manager.clone(),
            conversation_watchers: conversation_watchers.clone(),
            config: config.clone(),
            server_config,
            auth_config: auth_config.clone(),
            metrics: metrics.clone(),
            db: db.clone(),
            repository: repository.clone(),
            persistence_service: persistence_service.clone(),
            instance_persistors: instance_persistors.clone(),
            notes_storage: notes_storage.clone(),
            global_state_manager: global_state_manager.clone(),
            identity: Some(identity.clone()),
            iroh_node_id: iroh_transport.as_ref().map(|(iroh, _)| iroh.node_id()),
            instance_name: fc_initial.transport.instance_name.clone(),
            connection_manager: connection_manager.clone(),
            runtime_overrides: runtime_overrides.clone(),
            restart_tx: restart_tx.clone(),
        };

        // Build auth sub-state
        let auth_state = AuthState {
            repository: repository.clone(),
            auth_config: auth_config.clone(),
        };

        // Build routes
        #[cfg(not(feature = "embedded-ui"))]
        let app = Router::new()
            .route("/", get(views::index_page))
            .route("/settings", get(views::settings_page))
            .route("/history", get(views::history_page))
            .route("/conversation/{id}", get(views::conversation_detail_page));

        #[cfg(feature = "embedded-ui")]
        let app = Router::new();

        let mut app = app
            // Instance routes
            .route("/api/instances", get(handlers::list_instances))
            .route("/api/instances", post(handlers::create_instance))
            .route("/api/instances/{id}", get(handlers::get_instance))
            .route("/api/instances/{id}", delete(handlers::delete_instance))
            .route("/api/instances/{id}/name", patch(handlers::set_custom_name))
            .route("/api/ws", get(handlers::multiplexed_websocket_handler))
            .route("/api/preview", get(handlers::preview_websocket_handler))
            .route(
                "/api/instances/{id}/output",
                get(handlers::get_instance_output),
            )
            // File routes
            .route("/api/instances/{id}/files", get(files::list_instance_files))
            .route(
                "/api/instances/{id}/files/search",
                get(files::search_instance_files),
            )
            .route(
                "/api/instances/{id}/files/content",
                get(files::get_instance_file_content),
            )
            // Git routes
            .route("/api/instances/{id}/git/log", get(git::get_git_log))
            .route(
                "/api/instances/{id}/git/branches",
                get(git::get_git_branches),
            )
            .route("/api/instances/{id}/git/status", get(git::get_git_status))
            .route("/api/instances/{id}/git/diff", get(git::get_git_diff))
            // Live conversation routes
            .route(
                "/api/instances/{id}/conversation",
                get(handlers::get_conversation),
            )
            .route(
                "/api/instances/{id}/conversation/poll",
                get(handlers::poll_conversation),
            )
            // Database conversation endpoints
            .route("/api/conversations", get(handlers::list_conversations))
            .route(
                "/api/conversations/search",
                get(handlers::search_conversations_handler),
            )
            .route(
                "/api/conversations/{id}",
                get(handlers::get_conversation_by_id),
            )
            .route(
                "/api/conversations/{id}/comments",
                post(handlers::add_comment).get(handlers::get_comments),
            )
            .route(
                "/api/conversations/{id}/share",
                post(handlers::create_share),
            )
            .route("/api/share/{token}", get(handlers::get_shared_conversation))
            // Notes endpoints
            .route(
                "/api/notes/{session_id}",
                get(handlers::get_notes).post(handlers::create_note),
            )
            .route(
                "/api/notes/{session_id}/{note_id}",
                post(handlers::update_note).delete(handlers::delete_note),
            )
            // Task endpoints
            .route(
                "/api/tasks",
                get(handlers::list_tasks_handler).post(handlers::create_task_handler),
            )
            .route("/api/tasks/migrate", post(handlers::migrate_tasks_handler))
            .route(
                "/api/tasks/{id}",
                get(handlers::get_task_handler)
                    .patch(handlers::update_task_handler)
                    .delete(handlers::delete_task_handler),
            )
            .route("/api/tasks/{id}/send", post(handlers::send_task_handler))
            .route(
                "/api/tasks/{id}/dispatch",
                post(handlers::create_dispatch_handler),
            )
            .route("/api/tasks/{id}/tags", post(handlers::add_task_tag_handler))
            .route(
                "/api/tasks/{id}/tags/{tag_id}",
                delete(handlers::remove_task_tag_handler),
            )
            // Admin endpoints
            .route("/api/admin/stats", get(handlers::get_database_stats))
            .route("/api/admin/import", post(handlers::trigger_import))
            .route("/api/admin/restart", post(handlers::restart_handler))
            .route(
                "/api/admin/config",
                get(handlers::get_config_handler).patch(handlers::patch_config_handler),
            )
            // Invite endpoints
            .route(
                "/api/invites",
                get(handlers::list_invites_handler).post(handlers::create_invite_handler),
            )
            .route(
                "/api/invites/{nonce}",
                delete(handlers::revoke_invite_handler),
            )
            // Federation connection endpoints
            .route(
                "/api/federation/connections",
                get(handlers::list_connections_handler),
            )
            // Remote Crab City management endpoints
            .route("/api/remotes", get(handlers::list_remotes_handler))
            .route(
                "/api/remotes/connect",
                post(handlers::connect_remote_handler),
            )
            .route(
                "/api/remotes/{host_node_id}",
                delete(handlers::remove_remote_handler),
            )
            .route(
                "/api/remotes/{host_node_id}/status",
                get(handlers::remote_status_handler),
            )
            // Health endpoints
            .route("/health", get(handlers::health_handler))
            .route("/health/live", get(handlers::health_live_handler))
            .route("/health/ready", get(handlers::health_ready_handler))
            .route("/metrics", get(handlers::metrics_handler));

        // Auth middleware: loopback → Owner access, public routes → pass through,
        // all other HTTP → 401 (must use WS challenge-response or iroh transport).
        app = app.layer(axum::middleware::from_fn_with_state(
            auth_state,
            auth::auth_middleware,
        ));

        let app = app
            .layer(TraceLayer::new_for_http().make_span_with(RequestIdMakeSpan))
            .layer(CorsLayer::permissive())
            .with_state(app_state);

        // Serve SPA when embedded
        #[cfg(feature = "embedded-ui")]
        let app = app.fallback_service(embedded_ui::spa_router());

        #[cfg(feature = "embedded-ui")]
        if first_iteration {
            info!("Embedded UI enabled - serving SPA at /");
        }

        let port = bound_port.unwrap_or(effective_port);
        let addr = format!("{}:{}", effective_host, port).parse::<SocketAddr>()?;
        let listener = tokio::net::TcpListener::bind(addr).await?;
        let actual_addr = listener.local_addr()?;
        bound_port = Some(actual_addr.port());
        bound_host = Some(effective_host.to_string());

        // Write daemon PID and port files so clients can discover us
        let pid = std::process::id();
        cli::daemon::write_daemon_files(&config_for_shutdown, pid, actual_addr.port())?;

        if first_iteration {
            info!("Crab City listening on http://{}", actual_addr);
            info!("");
            info!("Web UI: http://{}/", actual_addr);
            info!("API endpoints:");
            info!("  GET    /api/instances       - List all instances");
            info!("  POST   /api/instances       - Create new instance");
            info!("  GET    /api/instances/:id   - Get instance details");
            info!("  DELETE /api/instances/:id   - Stop instance");
            info!("  GET    /api/ws              - Multiplexed WebSocket");
        } else {
            info!("Server restarted on http://{}", actual_addr);
        }

        // Create shutdown signal handler
        let shutdown_signal = async {
            tokio::signal::ctrl_c()
                .await
                .expect("Failed to install Ctrl+C handler");
            info!("Received shutdown signal, cleaning up...");
        };

        // Race: serve vs restart signal
        tokio::select! {
            result = axum::serve(
                listener,
                app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            ).with_graceful_shutdown(shutdown_signal) => {
                // Real shutdown (Ctrl-C / SIGTERM)
                if let Err(e) = result {
                    warn!("Server error: {}", e);
                }
                break;
            }
            _ = restart_rx.changed() => {
                info!("Restarting HTTP server with new config...");
                first_iteration = false;
                continue;
            }
        }
    }

    // === Cleanup ===

    // Shut down federation connections first
    if let Some(ref mgr) = connection_manager {
        mgr.shutdown().await;
        info!("federation connection manager shut down");
    }

    // Shut down iroh transport (stops accepting new connections)
    if let Some((iroh, relay)) = iroh_transport {
        iroh.shutdown().await;
        relay.shutdown().await;
        info!("iroh transport and relay shut down");
    }

    info!("Flushing persistence buffer...");
    if let Err(e) = persistence_for_shutdown.flush_all().await {
        warn!("Failed to flush persistence buffer during shutdown: {}", e);
    }

    info!("Stopping running instances...");
    let instances = instances_for_shutdown.list().await;
    for instance in instances.iter().filter(|i| i.running) {
        if !instances_for_shutdown.stop(&instance.id).await {
            warn!("Failed to stop instance {} during shutdown", instance.id);
        }
    }
    info!("Stopped {} instances", instances.len());

    // Clean up daemon files on shutdown
    cli::daemon::cleanup_daemon_files(&config_for_shutdown);

    info!("Shutdown complete");
    Ok(())
}

/// Detect first-run (no active grants) and generate an owner invite.
async fn first_run_bootstrap(
    identity: &Arc<InstanceIdentity>,
    repository: &Arc<ConversationRepository>,
) {
    use crab_city_auth::{Capability, Invite};

    // Check if any active grants exist
    let grants = match repository.list_grants().await {
        Ok(g) => g,
        Err(e) => {
            warn!("Failed to check grants for first-run detection: {}", e);
            return;
        }
    };

    if !grants.is_empty() {
        return;
    }

    // First run — generate an owner invite
    info!("First run detected — generating owner invite...");

    let expires_at = Some(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 3600, // 1 hour
    );

    let invite = Invite::create_flat(
        identity.signing_key(),
        &identity.public_key,
        Capability::Owner,
        1,
        expires_at,
        &mut rand::rng(),
    );

    let nonce = invite.links[0].nonce;
    let token = invite.to_base32();
    let chain_blob = invite.to_bytes();

    let expires_str = expires_at.map(|ts| {
        chrono::DateTime::from_timestamp(ts as i64, 0)
            .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_default()
    });

    if let Err(e) = repository
        .store_invite(
            &nonce,
            identity.public_key.as_bytes(),
            "owner",
            1,
            expires_str.as_deref(),
            &chain_blob,
            Some("First-run owner invite"),
        )
        .await
    {
        warn!("Failed to store first-run invite: {}", e);
        return;
    }

    info!("");
    info!("=== FIRST RUN — OWNER INVITE ===");
    info!("Token: {}", token);
    info!("Expires in 1 hour, single use.");
    info!("Join at: /join#{}", token);
    info!("=================================");
    info!("");
}
