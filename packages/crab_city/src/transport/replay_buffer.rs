//! Bounded ring buffer for reconnection replay.
//!
//! Stores recent serialized `ServerMessage` payloads keyed by monotonic sequence
//! number. On reconnect, a client sends `last_seq` and gets back everything since.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Maximum entries in the replay buffer.
const DEFAULT_MAX_ENTRIES: usize = 1000;
/// Maximum age before eviction.
const DEFAULT_MAX_AGE: Duration = Duration::from_secs(5 * 60);

pub struct ReplayBuffer {
    buffer: VecDeque<ReplayEntry>,
    next_seq: u64,
    max_entries: usize,
    max_age: Duration,
}

struct ReplayEntry {
    seq: u64,
    data: Vec<u8>,
    inserted_at: Instant,
}

impl ReplayBuffer {
    pub fn new() -> Self {
        Self {
            buffer: VecDeque::new(),
            next_seq: 1,
            max_entries: DEFAULT_MAX_ENTRIES,
            max_age: DEFAULT_MAX_AGE,
        }
    }

    /// Push a serialized message into the buffer. Returns the assigned sequence number.
    pub fn push(&mut self, msg: &[u8]) -> u64 {
        let seq = self.next_seq;
        self.next_seq += 1;

        self.buffer.push_back(ReplayEntry {
            seq,
            data: msg.to_vec(),
            inserted_at: Instant::now(),
        });

        // Evict by capacity
        while self.buffer.len() > self.max_entries {
            self.buffer.pop_front();
        }

        seq
    }

    /// Get all messages with sequence number > `last_seq`.
    /// Returns `None` if `last_seq` is too old (before earliest buffered entry).
    pub fn replay_since(&self, last_seq: u64) -> Option<Vec<(u64, &[u8])>> {
        if self.buffer.is_empty() {
            return if last_seq == 0 { Some(vec![]) } else { None };
        }

        let oldest = self.buffer.front().unwrap().seq;
        if last_seq < oldest.saturating_sub(1) {
            // Gap — client missed messages that were already evicted
            return None;
        }

        let result: Vec<(u64, &[u8])> = self
            .buffer
            .iter()
            .filter(|e| e.seq > last_seq)
            .map(|e| (e.seq, e.data.as_slice()))
            .collect();

        Some(result)
    }

    /// Remove entries older than `max_age`.
    pub fn evict_expired(&mut self) {
        let cutoff = Instant::now() - self.max_age;
        while let Some(front) = self.buffer.front() {
            if front.inserted_at < cutoff {
                self.buffer.pop_front();
            } else {
                break;
            }
        }
    }

    /// Current sequence number (last assigned).
    pub fn current_seq(&self) -> u64 {
        self.next_seq.saturating_sub(1)
    }

    /// Number of buffered entries.
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.buffer.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_and_replay() {
        let mut buf = ReplayBuffer::new();
        let s1 = buf.push(b"hello");
        let s2 = buf.push(b"world");
        assert_eq!(s1, 1);
        assert_eq!(s2, 2);

        // Replay from 0 → get both
        let msgs = buf.replay_since(0).unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0], (1, b"hello".as_slice()));
        assert_eq!(msgs[1], (2, b"world".as_slice()));

        // Replay from 1 → get only second
        let msgs = buf.replay_since(1).unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0], (2, b"world".as_slice()));

        // Replay from 2 → empty
        let msgs = buf.replay_since(2).unwrap();
        assert!(msgs.is_empty());
    }

    #[test]
    fn capacity_eviction() {
        let mut buf = ReplayBuffer::new();
        buf.max_entries = 3;

        buf.push(b"a");
        buf.push(b"b");
        buf.push(b"c");
        buf.push(b"d"); // evicts "a"

        assert_eq!(buf.len(), 3);

        // seq 1 was evicted, so replaying from 0 fails
        assert!(buf.replay_since(0).is_none());

        // Replay from 1 (the evicted boundary) succeeds
        let msgs = buf.replay_since(1).unwrap();
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[0].1, b"b");
    }

    #[test]
    fn empty_buffer() {
        let buf = ReplayBuffer::new();
        assert!(buf.replay_since(0).unwrap().is_empty());
        assert!(buf.replay_since(1).is_none());
    }

    #[test]
    fn current_seq() {
        let mut buf = ReplayBuffer::new();
        assert_eq!(buf.current_seq(), 0);
        buf.push(b"x");
        assert_eq!(buf.current_seq(), 1);
        buf.push(b"y");
        assert_eq!(buf.current_seq(), 2);
    }
}
