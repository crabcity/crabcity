//! Focus Handling
//!
//! Functions for handling focus switches between instances and sending conversation data.

use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::instance_manager::InstanceManager;

use super::conversation_watcher::run_session_discovery;
use super::protocol::ServerMessage;
use super::state_manager::{ConversationEvent, GlobalStateManager};

/// Streaming UTF-8 decoder that buffers incomplete multi-byte sequences
/// across chunk boundaries so that raw PTY reads never produce replacement
/// characters from split characters.
pub(crate) struct Utf8StreamDecoder {
    buf: Vec<u8>,
}

impl Utf8StreamDecoder {
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }

    /// Drop any buffered incomplete bytes (e.g. after broadcast lag
    /// where the continuation bytes were lost).
    pub fn clear(&mut self) {
        self.buf.clear();
    }

    /// Feed a chunk of bytes and return the longest valid UTF-8 string.
    /// Any trailing incomplete multi-byte sequence is retained for the next call.
    /// Genuinely invalid bytes (non-UTF-8 data) are replaced with U+FFFD.
    pub fn decode(&mut self, chunk: &[u8]) -> String {
        self.buf.extend_from_slice(chunk);
        let mut result = String::new();

        loop {
            match std::str::from_utf8(&self.buf) {
                Ok(s) => {
                    result.push_str(s);
                    self.buf.clear();
                    break;
                }
                Err(e) => {
                    let valid_up_to = e.valid_up_to();
                    if valid_up_to > 0 {
                        // Safety: from_utf8 validated these bytes
                        result.push_str(std::str::from_utf8(&self.buf[..valid_up_to]).unwrap());
                    }

                    match e.error_len() {
                        None => {
                            // Incomplete sequence at end — keep for next call
                            self.buf = self.buf[valid_up_to..].to_vec();
                            break;
                        }
                        Some(len) => {
                            // Genuinely invalid bytes (non-UTF-8 data from PTY,
                            // or stale bytes after broadcast lag missed by clear).
                            result.push('\u{FFFD}');
                            self.buf = self.buf[valid_up_to + len..].to_vec();
                            // Continue loop to process remaining bytes
                        }
                    }
                }
            }
        }

        result
    }
}

/// Send conversation entries since a given UUID (or full conversation if None).
///
/// Reads from the server-owned conversation store (maintained by the server watcher),
/// not directly from the JSONL file. This ensures a single source of truth.
pub async fn send_conversation_since(
    instance_id: &str,
    since_uuid: Option<&str>,
    state_manager: &Arc<GlobalStateManager>,
    tx: &mpsc::Sender<ServerMessage>,
    _repository: Option<&Arc<crate::repository::ConversationRepository>>,
) -> Result<(), String> {
    let all_turns = state_manager.get_conversation_snapshot(instance_id).await;

    if all_turns.is_empty() {
        debug!(
            "[CONVO-SYNC {}] Server store empty, nothing to send",
            instance_id
        );
        return Ok(());
    }

    if let Some(since) = since_uuid {
        // Find the position of since_uuid in the stored turns and send everything after it.
        let since_idx = all_turns
            .iter()
            .position(|t| t.get("uuid").and_then(|v| v.as_str()) == Some(since));

        let new_turns: Vec<_> = match since_idx {
            Some(idx) => all_turns.into_iter().skip(idx + 1).collect(),
            None => all_turns, // UUID not found → send full sync
        };

        if !new_turns.is_empty() {
            info!(
                "[CONVO-SYNC {}] Sending ConversationUpdate with {} turns (since {:?})",
                instance_id,
                new_turns.len(),
                since
            );
            let _ = tx
                .send(ServerMessage::ConversationUpdate {
                    instance_id: instance_id.to_string(),
                    turns: new_turns,
                })
                .await;
        }
    } else {
        info!(
            "[CONVO-SYNC {}] Sending ConversationFull with {} turns",
            instance_id,
            all_turns.len()
        );
        let _ = tx
            .send(ServerMessage::ConversationFull {
                instance_id: instance_id.to_string(),
                turns: all_turns,
            })
            .await;
    }

    Ok(())
}

/// Handle focus switch to a new instance - runs until cancelled or error
pub async fn handle_focus(
    instance_id: String,
    _since_uuid: Option<String>,
    cancel: CancellationToken,
    state_manager: Arc<GlobalStateManager>,
    instance_manager: Arc<InstanceManager>,
    tx: mpsc::Sender<ServerMessage>,
    session_rx: Arc<tokio::sync::Mutex<mpsc::Receiver<String>>>,
    max_history_bytes: usize,
) {
    debug!("Focusing on instance: {}", instance_id);

    // Get current claude_state to include in FocusAck (prevents race condition)
    let current_state = instance_manager
        .get(&instance_id)
        .await
        .and_then(|inst| inst.claude_state);

    // Send focus acknowledgment with current state
    if tx
        .send(ServerMessage::FocusAck {
            instance_id: instance_id.clone(),
            claude_state: current_state.clone(),
        })
        .await
        .is_err()
    {
        return;
    }

    // Get the instance info
    let (handle, working_dir, created_at, is_claude) = match state_manager
        .get_tracker_info(&instance_id)
        .await
    {
        Some(info) => info,
        None => {
            if tx
                .send(ServerMessage::Error {
                    instance_id: Some(instance_id.clone()),
                    message: format!("Instance {} not found", instance_id),
                })
                .await
                .is_err()
            {
                warn!(instance = %instance_id, "Failed to send error (instance not found) - channel closed");
            }
            return;
        }
    };

    // Send terminal history (bounded by config)
    let history = handle.get_recent_output(max_history_bytes).await;
    if !history.is_empty() {
        if tx
            .send(ServerMessage::OutputHistory {
                instance_id: instance_id.clone(),
                data: history.join(""),
            })
            .await
            .is_err()
        {
            warn!(instance = %instance_id, "Failed to send OutputHistory - channel closed");
            return;
        }
    }

    // Note: claude_state is already sent in FocusAck to prevent race conditions.
    // No need to send a separate StateChange here.

    // Subscribe to PTY output
    let mut output_rx = match handle.subscribe_output().await {
        Ok(rx) => rx,
        Err(e) => {
            error!("Failed to subscribe to PTY output: {}", e);
            return;
        }
    };

    // Conversation data: get snapshot from server-owned watcher, subscribe for updates.
    // If the session hasn't been discovered yet and this is Claude, run session discovery
    // (handles the ambiguous multi-session case by asking the user).
    let discovery_task = if is_claude {
        // Check if session already discovered
        let has_session = handle.get_session_id().await.is_some();

        if !has_session {
            let tx_disc = tx.clone();
            let cancel_disc = cancel.clone();
            let state_mgr = state_manager.clone();
            let instance_id_disc = instance_id.clone();
            let session_rx = session_rx.clone();

            Some(tokio::spawn(async move {
                run_session_discovery(
                    instance_id_disc,
                    working_dir,
                    created_at,
                    cancel_disc,
                    state_mgr,
                    tx_disc,
                    session_rx,
                )
                .await;
            }))
        } else {
            None
        }
    } else {
        None
    };

    // Subscribe to conversation broadcast BEFORE reading the snapshot.
    // This prevents a race where the server watcher broadcasts between
    // our snapshot read and subscription, causing us to miss the data.
    let mut convo_rx = state_manager.subscribe_conversation(&instance_id).await;

    // Send current conversation snapshot from server store.
    if is_claude {
        let turns = state_manager.get_conversation_snapshot(&instance_id).await;
        if !turns.is_empty() {
            info!(
                "[FOCUS {}] Sending ConversationFull with {} turns from server store",
                instance_id,
                turns.len()
            );
            let _ = tx
                .send(ServerMessage::ConversationFull {
                    instance_id: instance_id.clone(),
                    turns,
                })
                .await;
        } else {
            debug!(
                "[FOCUS {}] Server store empty, will receive via broadcast when ready",
                instance_id
            );
        }
    }

    // Forward PTY output and conversation updates to client until cancelled.
    // State tracking is handled by the background PTY reader in InstanceTracker.
    let tx_output = tx.clone();
    let instance_id_output = instance_id.clone();
    let mut decoder = Utf8StreamDecoder::new();

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                debug!("Focus cancelled for instance {}", instance_id);
                break;
            }
            result = output_rx.recv() => {
                match result {
                    Ok(event) => {
                        let data = decoder.decode(&event.data);
                        if !data.is_empty() {
                            if tx_output.send(ServerMessage::Output {
                                instance_id: instance_id_output.clone(),
                                data,
                            }).await.is_err() {
                                break;
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        decoder.clear();
                        warn!(instance = %instance_id_output, "PTY output lagged by {} messages", n);
                        if tx_output.send(ServerMessage::OutputLagged {
                            instance_id: instance_id_output.clone(),
                            dropped_count: n,
                        }).await.is_err() {
                            warn!(instance = %instance_id_output, "Failed to send OutputLagged notification - channel closed");
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        debug!("PTY output channel closed");
                        break;
                    }
                }
            }
            // Forward conversation events from server-owned watcher
            event = async {
                if let Some(ref mut rx) = convo_rx {
                    rx.recv().await
                } else {
                    // No conversation subscription — park forever
                    std::future::pending().await
                }
            } => {
                match event {
                    Ok(ConversationEvent::Full { instance_id: ref iid, turns }) if iid == &instance_id => {
                        info!(
                            "[FOCUS {}] Forwarding ConversationFull ({} turns)",
                            instance_id, turns.len()
                        );
                        if tx.send(ServerMessage::ConversationFull {
                            instance_id: instance_id.clone(),
                            turns,
                        }).await.is_err() {
                            break;
                        }
                    }
                    Ok(ConversationEvent::Update { instance_id: ref iid, turns }) if iid == &instance_id => {
                        if tx.send(ServerMessage::ConversationUpdate {
                            instance_id: instance_id.clone(),
                            turns,
                        }).await.is_err() {
                            break;
                        }
                    }
                    Ok(_) => {
                        // Event for a different instance — ignore
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!(instance = %instance_id, "Conversation broadcast lagged by {} messages, sending full sync", n);
                        // Re-sync from server store
                        let turns = state_manager.get_conversation_snapshot(&instance_id).await;
                        if !turns.is_empty() {
                            let _ = tx.send(ServerMessage::ConversationFull {
                                instance_id: instance_id.clone(),
                                turns,
                            }).await;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        debug!("Conversation broadcast channel closed for instance {}", instance_id);
                        convo_rx = None; // Stop polling closed channel
                    }
                }
            }
        }
    }

    // Clean up session discovery task
    if let Some(task) = discovery_task {
        task.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Utf8StreamDecoder ──────────────────────────────────────────────

    #[test]
    fn decode_clean_ascii() {
        let mut dec = Utf8StreamDecoder::new();
        assert_eq!(dec.decode(b"hello world"), "hello world");
    }

    #[test]
    fn decode_clean_multibyte() {
        let mut dec = Utf8StreamDecoder::new();
        assert_eq!(dec.decode("─".as_bytes()), "─");
        assert_eq!(dec.decode("🦀".as_bytes()), "🦀");
    }

    #[test]
    fn decode_3byte_split_2_1() {
        let mut dec = Utf8StreamDecoder::new();
        let bytes = "─".as_bytes(); // [0xE2, 0x94, 0x80]
        assert_eq!(bytes, &[0xE2, 0x94, 0x80]);

        let r1 = dec.decode(&bytes[..2]);
        assert_eq!(r1, ""); // incomplete — held
        let r2 = dec.decode(&bytes[2..]);
        assert_eq!(r2, "─"); // completed
    }

    #[test]
    fn decode_3byte_split_1_2() {
        let mut dec = Utf8StreamDecoder::new();
        let bytes = "─".as_bytes();

        let r1 = dec.decode(&bytes[..1]);
        assert_eq!(r1, "");
        let r2 = dec.decode(&bytes[1..]);
        assert_eq!(r2, "─");
    }

    #[test]
    fn decode_3byte_split_1_1_1() {
        let mut dec = Utf8StreamDecoder::new();
        let bytes = "─".as_bytes();

        assert_eq!(dec.decode(&bytes[..1]), "");
        assert_eq!(dec.decode(&bytes[1..2]), "");
        assert_eq!(dec.decode(&bytes[2..3]), "─");
    }

    #[test]
    fn decode_4byte_split_2_2() {
        let mut dec = Utf8StreamDecoder::new();
        let bytes = "🦀".as_bytes(); // [0xF0, 0x9F, 0xA6, 0x80]
        assert_eq!(bytes.len(), 4);

        assert_eq!(dec.decode(&bytes[..2]), "");
        assert_eq!(dec.decode(&bytes[2..]), "🦀");
    }

    #[test]
    fn decode_4byte_split_3_1() {
        let mut dec = Utf8StreamDecoder::new();
        let bytes = "🦀".as_bytes();

        assert_eq!(dec.decode(&bytes[..3]), "");
        assert_eq!(dec.decode(&bytes[3..]), "🦀");
    }

    #[test]
    fn decode_4byte_split_1_1_1_1() {
        let mut dec = Utf8StreamDecoder::new();
        let bytes = "🦀".as_bytes();

        assert_eq!(dec.decode(&bytes[..1]), "");
        assert_eq!(dec.decode(&bytes[1..2]), "");
        assert_eq!(dec.decode(&bytes[2..3]), "");
        assert_eq!(dec.decode(&bytes[3..4]), "🦀");
    }

    #[test]
    fn decode_ascii_before_split() {
        let mut dec = Utf8StreamDecoder::new();
        // "abc─def" — split so ─ straddles the boundary
        let bytes = "abc─def".as_bytes();
        // "abc" = 3 bytes, "─" = 3 bytes, "def" = 3 bytes

        let r1 = dec.decode(&bytes[..4]); // b"abc\xE2"
        assert_eq!(r1, "abc");
        let r2 = dec.decode(&bytes[4..]); // b"\x94\x80def"
        assert_eq!(r2, "─def");
    }

    #[test]
    fn decode_consecutive_multibyte_split() {
        let mut dec = Utf8StreamDecoder::new();
        // "──" = 6 bytes, split at byte 4
        let bytes = "──".as_bytes();
        assert_eq!(bytes.len(), 6);

        let r1 = dec.decode(&bytes[..4]); // first ─ complete, 1 byte of second
        assert_eq!(r1, "─");
        let r2 = dec.decode(&bytes[4..]); // remaining 2 bytes of second ─
        assert_eq!(r2, "─");
    }

    #[test]
    fn decode_no_replacement_chars_ever() {
        let bytes = "─".as_bytes();

        // Split every possible way; concatenated result must never contain U+FFFD
        for split_at in 1..bytes.len() {
            let mut d = Utf8StreamDecoder::new();
            let r1 = d.decode(&bytes[..split_at]);
            let r2 = d.decode(&bytes[split_at..]);
            let combined = format!("{}{}", r1, r2);
            assert!(
                !combined.contains('\u{FFFD}'),
                "split_at={}: got replacement char in {:?}",
                split_at,
                combined,
            );
            assert_eq!(combined, "─", "split_at={}", split_at);
        }
    }

    #[test]
    fn decode_no_replacement_chars_4byte() {
        // Same as above but for a 4-byte character
        let bytes = "🦀".as_bytes();
        for split_at in 1..bytes.len() {
            let mut d = Utf8StreamDecoder::new();
            let r1 = d.decode(&bytes[..split_at]);
            let r2 = d.decode(&bytes[split_at..]);
            let combined = format!("{}{}", r1, r2);
            assert!(
                !combined.contains('\u{FFFD}'),
                "split_at={}: got replacement char in {:?}",
                split_at,
                combined,
            );
            assert_eq!(combined, "🦀", "split_at={}", split_at);
        }
    }

    #[test]
    fn decode_prompt_box_line_split() {
        // Simulate exactly the user's scenario: a line of box-drawing chars
        // gets split at an arbitrary byte boundary during PTY read()
        let prompt_line = "─".repeat(40); // 120 bytes of box-drawing
        let bytes = prompt_line.as_bytes();

        // Try every possible split point
        for split_at in 1..bytes.len() {
            let mut d = Utf8StreamDecoder::new();
            let r1 = d.decode(&bytes[..split_at]);
            let r2 = d.decode(&bytes[split_at..]);
            let combined = format!("{}{}", r1, r2);
            assert!(
                !combined.contains('\u{FFFD}'),
                "split_at={}: replacement char in reconstructed prompt line",
                split_at,
            );
            assert_eq!(combined, prompt_line, "split_at={}", split_at);
        }
    }

    #[test]
    fn decode_mixed_ascii_and_multibyte_all_splits() {
        // Simulate realistic Claude Code output: ANSI escapes + box drawing
        let content = "abc─def─ghi";
        let bytes = content.as_bytes();

        for split_at in 1..bytes.len() {
            let mut d = Utf8StreamDecoder::new();
            let r1 = d.decode(&bytes[..split_at]);
            let r2 = d.decode(&bytes[split_at..]);
            let combined = format!("{}{}", r1, r2);
            assert!(
                !combined.contains('\u{FFFD}'),
                "split_at={}: got replacement char in {:?}",
                split_at,
                combined,
            );
            assert_eq!(combined, content, "split_at={}", split_at);
        }
    }

    #[test]
    fn decode_three_way_split() {
        // Three chunks: common when PTY output is bursty
        let content = "hello─world";
        let bytes = content.as_bytes();

        for s1 in 1..bytes.len() - 1 {
            for s2 in s1 + 1..bytes.len() {
                let mut d = Utf8StreamDecoder::new();
                let r1 = d.decode(&bytes[..s1]);
                let r2 = d.decode(&bytes[s1..s2]);
                let r3 = d.decode(&bytes[s2..]);
                let combined = format!("{}{}{}", r1, r2, r3);
                assert!(
                    !combined.contains('\u{FFFD}'),
                    "splits=({},{}): got replacement char",
                    s1,
                    s2,
                );
                assert_eq!(combined, content, "splits=({},{})", s1, s2);
            }
        }
    }

    #[test]
    fn decode_empty_chunk() {
        let mut dec = Utf8StreamDecoder::new();
        assert_eq!(dec.decode(b""), "");
        assert_eq!(dec.decode(b"hello"), "hello");
    }

    #[test]
    fn decode_repeated_calls_clean() {
        let mut dec = Utf8StreamDecoder::new();
        assert_eq!(dec.decode(b"chunk1 "), "chunk1 ");
        assert_eq!(dec.decode(b"chunk2 "), "chunk2 ");
        assert_eq!(dec.decode("├──┤".as_bytes()), "├──┤");
    }

    #[test]
    fn decode_invalid_byte_0xff() {
        // 0xFF is never valid in UTF-8 — should produce U+FFFD
        let mut dec = Utf8StreamDecoder::new();
        let r = dec.decode(&[0xFF, b'A', b'B']);
        assert_eq!(r, "\u{FFFD}AB");
    }

    #[test]
    fn decode_stale_buffer_after_missed_clear() {
        // Simulates broadcast lag where decoder.clear() wasn't called:
        // 2 bytes of ─ buffered, continuation lost, next chunk starts fresh.
        let mut dec = Utf8StreamDecoder::new();
        let r1 = dec.decode(&[0xE2, 0x94]);
        assert_eq!(r1, ""); // incomplete, buffered

        // Continuation byte was in a dropped broadcast message.
        // Next chunk starts with escape sequences.
        let mut new_data = Vec::new();
        new_data.extend_from_slice(b"\x1b[H\x1b[2J");
        new_data.extend_from_slice("───".as_bytes());
        let r2 = dec.decode(&new_data);

        // Stale bytes produce U+FFFD, then valid content follows
        assert!(r2.starts_with("\u{FFFD}"));
        assert!(r2.contains("\x1b[H\x1b[2J"));
        assert!(r2.contains("───"));
    }

    #[test]
    fn decode_incomplete_lead_then_noncontination() {
        // Lead byte buffered, next chunk doesn't continue the sequence
        let mut dec = Utf8StreamDecoder::new();
        let r1 = dec.decode(&[0xE2]); // 3-byte lead
        assert_eq!(r1, "");
        let r2 = dec.decode(b"hello");
        assert_eq!(r2, "\u{FFFD}hello");
    }

    #[test]
    fn decode_incomplete_4byte_then_noncontination() {
        // 3 bytes of a 4-byte char buffered, then ASCII
        let mut dec = Utf8StreamDecoder::new();
        let crab = "🦀".as_bytes(); // [0xF0, 0x9F, 0xA6, 0x80]
        let r1 = dec.decode(&crab[..3]);
        assert_eq!(r1, "");
        let r2 = dec.decode(b"ok");
        assert_eq!(r2, "\u{FFFD}ok");
    }

    #[test]
    fn decode_multiple_invalid_sequences() {
        // Two interrupted sequences in a row
        let mut dec = Utf8StreamDecoder::new();
        // Buffer 2 bytes of ─
        dec.decode(&[0xE2, 0x94]);
        // Next: another lead byte (0xE2) then ASCII 'x'
        // Buffer becomes [0xE2, 0x94, 0xE2, 0x78]
        // [0xE2, 0x94] invalid (0xE2 not continuation) → U+FFFD
        // [0xE2] invalid (0x78 not continuation) → U+FFFD
        // [0x78] = "x"
        let r = dec.decode(&[0xE2, b'x']);
        assert_eq!(r, "\u{FFFD}\u{FFFD}x");
    }

    // ── send_conversation_since ────────────────────────────────────────

    use std::sync::Arc;
    use tokio::sync::mpsc;

    use crate::instance_actor::InstanceHandle;
    use crate::ws::state_manager::{GlobalStateManager, create_state_broadcast};

    /// Helper: create a state manager with conversation turns pre-loaded.
    async fn setup_state_with_turns(turns: Vec<serde_json::Value>) -> Arc<GlobalStateManager> {
        let mgr = Arc::new(GlobalStateManager::new(create_state_broadcast()));
        let (handle, _vt) = InstanceHandle::spawn_test(24, 80, 4096);
        mgr.insert_test_tracker("inst-1", handle).await;
        mgr.set_test_conversation_turns("inst-1", turns).await;
        mgr
    }

    #[tokio::test]
    async fn send_full_when_no_since_uuid() {
        let turns = vec![
            serde_json::json!({"uuid": "t1", "role": "user"}),
            serde_json::json!({"uuid": "t2", "role": "assistant"}),
        ];
        let mgr = setup_state_with_turns(turns).await;
        let (tx, mut rx) = mpsc::channel(16);

        send_conversation_since("inst-1", None, &mgr, &tx, None)
            .await
            .unwrap();

        match rx.recv().await.unwrap() {
            ServerMessage::ConversationFull { instance_id, turns } => {
                assert_eq!(instance_id, "inst-1");
                assert_eq!(turns.len(), 2);
            }
            other => panic!("expected ConversationFull, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn send_update_since_known_uuid() {
        let turns = vec![
            serde_json::json!({"uuid": "t1", "role": "user"}),
            serde_json::json!({"uuid": "t2", "role": "assistant"}),
            serde_json::json!({"uuid": "t3", "role": "user"}),
        ];
        let mgr = setup_state_with_turns(turns).await;
        let (tx, mut rx) = mpsc::channel(16);

        send_conversation_since("inst-1", Some("t1"), &mgr, &tx, None)
            .await
            .unwrap();

        match rx.recv().await.unwrap() {
            ServerMessage::ConversationUpdate { instance_id, turns } => {
                assert_eq!(instance_id, "inst-1");
                assert_eq!(turns.len(), 2);
                assert_eq!(turns[0]["uuid"], "t2");
                assert_eq!(turns[1]["uuid"], "t3");
            }
            other => panic!("expected ConversationUpdate, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn send_full_when_since_uuid_unknown() {
        let turns = vec![serde_json::json!({"uuid": "t1", "role": "user"})];
        let mgr = setup_state_with_turns(turns).await;
        let (tx, mut rx) = mpsc::channel(16);

        // Unknown UUID → full sync (all turns sent as Update)
        send_conversation_since("inst-1", Some("nonexistent"), &mgr, &tx, None)
            .await
            .unwrap();

        match rx.recv().await.unwrap() {
            ServerMessage::ConversationUpdate { turns, .. } => {
                assert_eq!(turns.len(), 1);
                assert_eq!(turns[0]["uuid"], "t1");
            }
            other => panic!(
                "expected ConversationUpdate for unknown UUID, got {:?}",
                other
            ),
        }
    }

    #[tokio::test]
    async fn send_nothing_when_since_is_last_uuid() {
        let turns = vec![
            serde_json::json!({"uuid": "t1", "role": "user"}),
            serde_json::json!({"uuid": "t2", "role": "assistant"}),
        ];
        let mgr = setup_state_with_turns(turns).await;
        let (tx, mut rx) = mpsc::channel(16);

        send_conversation_since("inst-1", Some("t2"), &mgr, &tx, None)
            .await
            .unwrap();

        // Nothing should be sent — no new turns after t2
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn send_nothing_when_store_empty() {
        let mgr = Arc::new(GlobalStateManager::new(create_state_broadcast()));
        let (handle, _vt) = InstanceHandle::spawn_test(24, 80, 4096);
        mgr.insert_test_tracker("inst-1", handle).await;
        let (tx, mut rx) = mpsc::channel(16);

        send_conversation_since("inst-1", None, &mgr, &tx, None)
            .await
            .unwrap();

        assert!(rx.try_recv().is_err());
    }
}
