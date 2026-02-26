pub mod admin;
pub mod conversations;
pub mod health;
pub mod instances;
pub mod notes;
pub mod tasks;
pub mod websocket;

// Re-export all handlers for easy route registration
pub use admin::{
    create_server_invite_handler, get_config_handler, get_database_stats,
    list_server_invites_handler, patch_config_handler, restart_handler,
    revoke_server_invite_handler, trigger_import,
};
pub use conversations::{
    add_comment, create_share, extract_title_from_turn, get_comments, get_conversation,
    get_conversation_by_id, get_shared_conversation, list_conversations, poll_conversation,
    process_watcher_entries, search_conversations_handler,
};
pub use health::{health_handler, health_live_handler, health_ready_handler, metrics_handler};
pub use instances::{
    accept_invitation, create_instance, create_invitation, delete_instance, get_instance,
    get_instance_output, list_instances, remove_collaborator, set_custom_name,
};
pub use notes::{create_note, delete_note, get_notes, update_note};
pub use tasks::{
    add_task_tag_handler, create_dispatch_handler, create_task_handler, delete_task_handler,
    get_task_handler, list_tasks_handler, migrate_tasks_handler, remove_task_tag_handler,
    send_task_handler, update_task_handler,
};
pub use websocket::{multiplexed_websocket_handler, websocket_handler};
