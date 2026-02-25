pub mod admin;
pub mod conversations;
pub mod health;
pub mod instances;
pub mod interconnect;
pub mod notes;
pub mod preview;
pub mod tasks;
pub mod websocket;

// Re-export all handlers for easy route registration
pub use admin::{
    get_config_handler, get_database_stats, patch_config_handler, restart_handler, trigger_import,
};
pub use conversations::{
    add_comment, create_share, format_entry, format_entry_with_attribution, get_comments,
    get_conversation, get_conversation_by_id, get_shared_conversation, list_conversations,
    poll_conversation, search_conversations_handler,
};
pub use health::{health_handler, health_live_handler, health_ready_handler, metrics_handler};
pub use instances::{
    create_instance, delete_instance, get_instance, get_instance_output, list_instances,
    set_custom_name,
};
pub use interconnect::{
    create_invite_handler, list_connections_handler, list_invites_handler, revoke_invite_handler,
};
pub use notes::{create_note, delete_note, get_notes, update_note};
pub use preview::preview_websocket_handler;
pub use tasks::{
    add_task_tag_handler, create_dispatch_handler, create_task_handler, delete_task_handler,
    get_task_handler, list_tasks_handler, migrate_tasks_handler, remove_task_tag_handler,
    send_task_handler, update_task_handler,
};
pub use websocket::multiplexed_websocket_handler;
