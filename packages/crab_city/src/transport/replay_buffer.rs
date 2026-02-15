//! Bounded ring buffer for connection reconnection replay.
//!
//! The server stores recent outbound messages so that when a client
//! reconnects, it can replay missed messages from a given sequence number
//! instead of sending a full state snapshot.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Default maximum number of entries in the replay buffer.
const DEFAULT_MAX_ENTRIES: usize = 1000;

/// Default maximum age for replay entries.
const DEFAULT_MAX_AGE: Duration = Duration::from_secs(300); // 5 minutes

/// A bounded ring buffer of serialized messages keyed by sequence number.
pub struct ReplayBuffer {
    buffer: VecDeque<Entry>,
    max_entries: usize,
    max_age: Duration,
}

struct Entry {
    seq: u64,
    bytes: Vec<u8>,
    timestamp: Instant,
}

impl ReplayBuffer {
    pub fn new() -> Self {
        Self {
            buffer: VecDeque::new(),
            max_entries: DEFAULT_MAX_ENTRIES,
            max_age: DEFAULT_MAX_AGE,
        }
    }

    /// Push a serialized message into the buffer.
    pub fn push(&mut self, seq: u64, bytes: Vec<u8>) {
        // Evict old entries first
        self.evict_expired();

        // Enforce capacity
        while self.buffer.len() >= self.max_entries {
            self.buffer.pop_front();
        }

        self.buffer.push_back(Entry {
            seq,
            bytes,
            timestamp: Instant::now(),
        });
    }

    /// Return all messages with seq > `last_seq`.
    ///
    /// Returns `None` if `last_seq` is too old (not in the buffer),
    /// meaning the caller should send a full state snapshot instead.
    pub fn replay_since(&self, last_seq: u64) -> Option<Vec<&[u8]>> {
        if self.buffer.is_empty() {
            // No messages at all — if client says seq 0, that's fine (nothing to replay)
            return if last_seq == 0 {
                Some(Vec::new())
            } else {
                None
            };
        }

        let oldest_seq = self.buffer.front().map(|e| e.seq).unwrap_or(0);
        if last_seq > 0 && last_seq < oldest_seq {
            // Client's last_seq is older than our oldest entry — can't replay
            return None;
        }

        let messages: Vec<&[u8]> = self
            .buffer
            .iter()
            .filter(|e| e.seq > last_seq)
            .map(|e| e.bytes.as_slice())
            .collect();

        Some(messages)
    }

    /// Remove entries older than `max_age`.
    pub fn evict_expired(&mut self) {
        let cutoff = Instant::now() - self.max_age;
        while let Some(front) = self.buffer.front() {
            if front.timestamp < cutoff {
                self.buffer.pop_front();
            } else {
                break;
            }
        }
    }

    /// Current number of entries.
    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    /// The highest sequence number in the buffer, or 0 if empty.
    pub fn head_seq(&self) -> u64 {
        self.buffer.back().map(|e| e.seq).unwrap_or(0)
    }
}

impl Default for ReplayBuffer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_and_replay() {
        let mut buf = ReplayBuffer::new();
        buf.push(1, b"msg1".to_vec());
        buf.push(2, b"msg2".to_vec());
        buf.push(3, b"msg3".to_vec());

        // Replay since seq 1 → should get msg2 and msg3
        let replay = buf.replay_since(1).unwrap();
        assert_eq!(replay.len(), 2);
        assert_eq!(replay[0], b"msg2");
        assert_eq!(replay[1], b"msg3");
    }

    #[test]
    fn replay_since_zero() {
        let mut buf = ReplayBuffer::new();
        buf.push(1, b"msg1".to_vec());
        buf.push(2, b"msg2".to_vec());

        let replay = buf.replay_since(0).unwrap();
        assert_eq!(replay.len(), 2);
    }

    #[test]
    fn replay_too_old_returns_none() {
        let mut buf = ReplayBuffer::new();
        buf.push(10, b"msg10".to_vec());
        buf.push(11, b"msg11".to_vec());

        // Client last saw seq 5, but our oldest is 10 — can't replay
        assert!(buf.replay_since(5).is_none());
    }

    #[test]
    fn empty_buffer_seq_zero_ok() {
        let buf = ReplayBuffer::new();
        let replay = buf.replay_since(0).unwrap();
        assert!(replay.is_empty());
    }

    #[test]
    fn empty_buffer_nonzero_returns_none() {
        let buf = ReplayBuffer::new();
        assert!(buf.replay_since(5).is_none());
    }

    #[test]
    fn capacity_eviction() {
        let mut buf = ReplayBuffer {
            max_entries: 3,
            ..ReplayBuffer::new()
        };

        buf.push(1, b"a".to_vec());
        buf.push(2, b"b".to_vec());
        buf.push(3, b"c".to_vec());
        buf.push(4, b"d".to_vec());

        assert_eq!(buf.len(), 3);
        // Oldest (seq 1) was evicted
        assert!(buf.replay_since(0).unwrap()[0] == b"b");
    }

    #[test]
    fn head_seq() {
        let mut buf = ReplayBuffer::new();
        assert_eq!(buf.head_seq(), 0);
        buf.push(5, b"x".to_vec());
        assert_eq!(buf.head_seq(), 5);
        buf.push(10, b"y".to_vec());
        assert_eq!(buf.head_seq(), 10);
    }

    #[test]
    fn replay_up_to_date() {
        let mut buf = ReplayBuffer::new();
        buf.push(1, b"msg1".to_vec());
        buf.push(2, b"msg2".to_vec());

        // Client already at head — no messages to replay
        let replay = buf.replay_since(2).unwrap();
        assert!(replay.is_empty());
    }
}
