use anyhow::{Context, Result};
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
mod import;
mod inference;
mod instance_actor;
mod instance_manager;
mod metrics;
mod models;
mod notes;
mod onboarding;
mod persistence;
mod repository;
mod terminal;
mod views;
pub mod websocket_proxy;
mod ws;

use std::collections::HashMap;
use std::path::PathBuf;
use tokio::sync::Mutex;

use crate::auth::AuthState;
use crate::config::{AuthConfig, CrabCityConfig, ServerConfig};
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
}

#[derive(Parser)]
struct ServerArgs {
    /// Port for the web server (0 = auto-select)
    #[arg(short, long, default_value = "0")]
    port: u16,

    /// Host to bind to
    #[arg(short = 'b', long, default_value = "127.0.0.1")]
    host: String,

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

    /// Reset the admin account password
    #[arg(long)]
    reset_admin: bool,

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
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let config = CrabCityConfig::new(cli.data_dir.clone())?;

    match cli.command {
        None => {
            // Bare `crab`: create new instance in cwd and attach
            cli::default_command(&config).await
        }
        Some(Commands::Attach(args)) => cli::attach_command(&config, args.target).await,
        Some(Commands::List(args)) => cli::list_command(&config, args.json).await,
        Some(Commands::Kill(args)) => cli::kill_command(&config, &args.target).await,
        Some(Commands::KillServer(args)) => cli::kill_server_command(&config, args.force).await,
        Some(Commands::Server(args)) => run_server(args, config).await,
    }
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

    // Initialize server runtime config
    let server_config = Arc::new(ServerConfig::from_env());

    // Create instance manager
    let instance_manager = Arc::new(InstanceManager::new(
        default_command,
        args.instance_base_port,
        server_config.instance.max_buffer_bytes,
    ));

    // Initialize notes storage
    let notes_storage = Arc::new(notes::NotesStorage::new(&config.data_dir)?);

    // Initialize global state manager for multiplexed WebSocket
    let state_broadcast = ws::create_state_broadcast();
    let global_state_manager = Arc::new(ws::GlobalStateManager::new(state_broadcast));
    info!(
        "Server config: max_history={}KB, max_buffer={}MB",
        server_config.websocket.max_history_replay_bytes / 1024,
        server_config.instance.max_buffer_bytes / (1024 * 1024)
    );

    // Initialize auth config
    let auth_config_raw = AuthConfig::from_env();
    if auth_config_raw.enabled {
        info!(
            "Authentication ENABLED (session TTL: {}s)",
            auth_config_raw.session_ttl_secs
        );
    } else {
        info!("Authentication disabled (set CRAB_CITY_AUTH_ENABLED=true to enable)");
    }

    // First-run onboarding
    onboarding::maybe_run_onboarding(&repository, &auth_config_raw).await?;

    // Reset admin password if requested
    if args.reset_admin {
        onboarding::reset_admin(&repository).await?;
    }

    let auth_config = Arc::new(auth_config_raw);

    // Initialize metrics
    let metrics = Arc::new(ServerMetrics::new());

    let app_state = AppState {
        instance_manager,
        conversation_watchers: Arc::new(Mutex::new(HashMap::new())),
        config: config.clone(),
        server_config,
        auth_config: auth_config.clone(),
        metrics,
        db: db.clone(),
        repository: repository.clone(),
        persistence_service: persistence_service.clone(),
        instance_persistors: Arc::new(Mutex::new(HashMap::new())),
        notes_storage,
        global_state_manager,
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
        .route("/api/instances/{id}/ws", get(handlers::websocket_handler))
        .route("/api/ws", get(handlers::multiplexed_websocket_handler))
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
        // Instance permission / invitation endpoints
        .route(
            "/api/instances/{id}/invite",
            post(handlers::create_invitation),
        )
        .route(
            "/api/invitations/{token}/accept",
            post(handlers::accept_invitation),
        )
        .route(
            "/api/instances/{id}/collaborators/{user_id}",
            delete(handlers::remove_collaborator),
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
        .route(
            "/api/admin/invites",
            post(handlers::create_server_invite_handler).get(handlers::list_server_invites_handler),
        )
        .route(
            "/api/admin/invites/{token}",
            delete(handlers::revoke_server_invite_handler),
        )
        // Health endpoints
        .route("/health", get(handlers::health_handler))
        .route("/health/live", get(handlers::health_live_handler))
        .route("/health/ready", get(handlers::health_ready_handler))
        .route("/metrics", get(handlers::metrics_handler));

    // Merge auth routes
    app = app.merge(auth::auth_routes().with_state(auth_state.clone()));

    // Apply auth middleware if enabled
    if auth_config.enabled {
        app = app.layer(axum::middleware::from_fn_with_state(
            auth_state,
            auth::auth_middleware,
        ));
    }

    // Spawn periodic expired session cleanup
    if auth_config.enabled {
        let cleanup_repo = repository.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(3600));
            loop {
                interval.tick().await;
                match cleanup_repo.cleanup_expired_sessions().await {
                    Ok(n) if n > 0 => info!("Cleaned up {} expired sessions", n),
                    _ => {}
                }
            }
        });
    }

    // Clone references needed for shutdown cleanup
    let persistence_for_shutdown = persistence_service.clone();
    let instances_for_shutdown = app_state.instance_manager.clone();
    let config_for_shutdown = config.clone();

    let app = app
        .layer(TraceLayer::new_for_http().make_span_with(RequestIdMakeSpan))
        .layer(CorsLayer::permissive())
        .with_state(app_state);

    // Serve SPA when embedded
    #[cfg(feature = "embedded-ui")]
    let app = app.fallback_service(embedded_ui::spa_router());

    #[cfg(feature = "embedded-ui")]
    info!("Embedded UI enabled - serving SPA at /");

    let addr = format!("{}:{}", args.host, args.port).parse::<SocketAddr>()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let actual_addr = listener.local_addr()?;

    // Write daemon PID and port files so clients can discover us
    let pid = std::process::id();
    cli::daemon::write_daemon_files(&config_for_shutdown, pid, actual_addr.port())?;

    info!("Crab City listening on http://{}", actual_addr);
    info!("");
    info!("Web UI: http://{}/", actual_addr);
    info!("API endpoints:");
    info!("  GET    /api/instances       - List all instances");
    info!("  POST   /api/instances       - Create new instance");
    info!("  GET    /api/instances/:id   - Get instance details");
    info!("  DELETE /api/instances/:id   - Stop instance");
    info!("  GET    /api/instances/:id/ws - WebSocket connection to instance");

    // Create shutdown signal handler
    let shutdown_signal = async {
        tokio::signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl+C handler");
        info!("Received shutdown signal, cleaning up...");
    };

    // Run server with graceful shutdown
    let server_result = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal)
    .await
    .context("Server error");

    // Perform cleanup after shutdown
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
    server_result
}
