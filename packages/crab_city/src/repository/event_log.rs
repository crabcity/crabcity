//! Repository layer for the append-only, hash-chained event log.

use anyhow::{Context, Result};
use sqlx::Row;

use crab_city_auth::event::{Event, EventCheckpoint, EventType};
use crab_city_auth::keys::{PublicKey, SigningKey};

use super::ConversationRepository;

/// Result of verifying a chain segment.
#[derive(Debug, Clone)]
pub struct ChainVerification {
    pub valid: bool,
    pub events_checked: u64,
    pub error: Option<String>,
}

/// An event with its inclusion proof: the event itself plus the nearest checkpoint.
#[derive(Debug, Clone)]
pub struct EventProof {
    pub event: Event,
    pub nearest_checkpoint: Option<StoredCheckpoint>,
}

/// DB row for a checkpoint.
#[derive(Debug, Clone)]
pub struct StoredCheckpoint {
    pub event_id: i64,
    pub chain_head_hash: Vec<u8>,
    pub signature: Vec<u8>,
    pub created_at: String,
}

impl ConversationRepository {
    /// Append an event to the log inside a transaction.
    /// Computes the hash chain automatically.
    pub async fn append_event(
        &self,
        event_type: EventType,
        actor: Option<&PublicKey>,
        target: Option<&PublicKey>,
        payload: &serde_json::Value,
        instance_key: &PublicKey,
    ) -> Result<i64> {
        let mut tx = self.pool.begin().await?;

        // Get the previous event's hash, or compute genesis hash
        let prev = sqlx::query("SELECT id, hash FROM event_log ORDER BY id DESC LIMIT 1")
            .fetch_optional(&mut *tx)
            .await?;

        let (prev_hash, next_id): ([u8; 32], i64) = match prev {
            Some(row) => {
                let hash_bytes: Vec<u8> = row.get("hash");
                let id: i64 = row.get("id");
                let hash = hash_bytes
                    .try_into()
                    .unwrap_or(Event::genesis_prev_hash(instance_key));
                (hash, id + 1)
            }
            None => (Event::genesis_prev_hash(instance_key), 1),
        };

        let created_at = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();

        let event = Event::new(
            next_id as u64,
            prev_hash,
            event_type,
            actor.copied(),
            target.copied(),
            payload.clone(),
            created_at,
        );

        let payload_str = serde_json::to_string(&event.payload)?;

        // Insert with explicit id so the DB id matches the hash
        sqlx::query(
            r#"
            INSERT INTO event_log (id, prev_hash, event_type, actor, target, payload, created_at, hash)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(next_id)
        .bind(event.prev_hash.as_slice())
        .bind(event.event_type.to_string())
        .bind(actor.map(|pk| pk.as_bytes().to_vec()))
        .bind(target.map(|pk| pk.as_bytes().to_vec()))
        .bind(&payload_str)
        .bind(&event.created_at)
        .bind(event.hash.as_slice())
        .execute(&mut *tx)
        .await
        .context("Failed to append event")?;

        let event_id = next_id;

        tx.commit().await?;

        Ok(event_id)
    }

    /// Query events with optional filtering.
    pub async fn query_events(
        &self,
        target: Option<&[u8]>,
        event_type_prefix: Option<&str>,
        limit: u32,
        before_id: Option<i64>,
    ) -> Result<Vec<Event>> {
        // Build query dynamically based on filters
        let mut sql = String::from(
            "SELECT id, prev_hash, event_type, actor, target, payload, created_at, hash FROM event_log WHERE 1=1",
        );
        if target.is_some() {
            sql.push_str(" AND target = ?");
        }
        if event_type_prefix.is_some() {
            sql.push_str(" AND event_type LIKE ?");
        }
        if before_id.is_some() {
            sql.push_str(" AND id < ?");
        }
        sql.push_str(" ORDER BY id DESC LIMIT ?");

        let mut query = sqlx::query(&sql);

        if let Some(t) = target {
            query = query.bind(t);
        }
        if let Some(prefix) = event_type_prefix {
            query = query.bind(format!("{prefix}%"));
        }
        if let Some(bid) = before_id {
            query = query.bind(bid);
        }
        query = query.bind(limit);

        let rows = query.fetch_all(&self.pool).await?;

        let mut events: Vec<Event> = rows.into_iter().map(|r| row_to_event(&r)).collect();
        events.reverse(); // Return in ascending order
        Ok(events)
    }

    /// Verify the hash chain between two event IDs (inclusive).
    pub async fn verify_chain(
        &self,
        from_id: i64,
        to_id: i64,
        instance_key: &PublicKey,
    ) -> Result<ChainVerification> {
        let rows = sqlx::query(
            r#"
            SELECT id, prev_hash, event_type, actor, target, payload, created_at, hash
            FROM event_log
            WHERE id >= ? AND id <= ?
            ORDER BY id ASC
            "#,
        )
        .bind(from_id)
        .bind(to_id)
        .fetch_all(&self.pool)
        .await?;

        let events: Vec<Event> = rows.iter().map(|r| row_to_event(r)).collect();

        if events.is_empty() {
            return Ok(ChainVerification {
                valid: true,
                events_checked: 0,
                error: None,
            });
        }

        // Determine genesis hash
        let genesis = Event::genesis_prev_hash(instance_key);

        // If from_id > 0, we need the prev event's hash as anchor
        let anchor = if from_id > 0 {
            let prev_row = sqlx::query("SELECT hash FROM event_log WHERE id = ?")
                .bind(from_id - 1)
                .fetch_optional(&self.pool)
                .await?;

            match prev_row {
                Some(row) => {
                    let hash_bytes: Vec<u8> = row.get("hash");
                    hash_bytes.try_into().unwrap_or(genesis)
                }
                None => genesis,
            }
        } else {
            genesis
        };

        match crab_city_auth::event::verify_chain(&events, &anchor) {
            Ok(()) => Ok(ChainVerification {
                valid: true,
                events_checked: events.len() as u64,
                error: None,
            }),
            Err(e) => Ok(ChainVerification {
                valid: false,
                events_checked: events.len() as u64,
                error: Some(e.to_string()),
            }),
        }
    }

    /// Get the chain head: (last_event_id, last_event_hash).
    pub async fn get_chain_head(&self) -> Result<Option<(i64, Vec<u8>)>> {
        let row = sqlx::query("SELECT id, hash FROM event_log ORDER BY id DESC LIMIT 1")
            .fetch_optional(&self.pool)
            .await?;

        Ok(row.map(|r| {
            let id: i64 = r.get("id");
            let hash: Vec<u8> = r.get("hash");
            (id, hash)
        }))
    }

    /// Create a signed checkpoint at the given event ID.
    pub async fn create_checkpoint(&self, event_id: i64, signing_key: &SigningKey) -> Result<()> {
        // Get the hash at this event_id
        let row = sqlx::query("SELECT hash FROM event_log WHERE id = ?")
            .bind(event_id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Event not found: {}", event_id))?;

        let hash_bytes: Vec<u8> = row.get("hash");
        let chain_head_hash: [u8; 32] = hash_bytes
            .try_into()
            .map_err(|_| anyhow::anyhow!("Invalid hash length"))?;

        let created_at = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();

        let checkpoint =
            EventCheckpoint::sign(signing_key, event_id as u64, chain_head_hash, created_at);

        sqlx::query(
            r#"
            INSERT INTO event_checkpoints (event_id, chain_head_hash, signature, created_at)
            VALUES (?, ?, ?, ?)
            "#,
        )
        .bind(event_id)
        .bind(checkpoint.chain_head_hash.as_slice())
        .bind(checkpoint.signature.as_bytes().as_slice())
        .bind(&checkpoint.created_at)
        .execute(&self.pool)
        .await
        .context("Failed to create checkpoint")?;

        Ok(())
    }

    /// Get an event and its nearest checkpoint (for proof of inclusion).
    pub async fn get_event_proof(&self, event_id: i64) -> Result<EventProof> {
        let row = sqlx::query(
            r#"
            SELECT id, prev_hash, event_type, actor, target, payload, created_at, hash
            FROM event_log WHERE id = ?
            "#,
        )
        .bind(event_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Event not found: {}", event_id))?;

        let event = row_to_event(&row);

        // Find nearest checkpoint at or after this event
        let cp_row = sqlx::query(
            r#"
            SELECT event_id, chain_head_hash, signature, created_at
            FROM event_checkpoints WHERE event_id >= ?
            ORDER BY event_id ASC LIMIT 1
            "#,
        )
        .bind(event_id)
        .fetch_optional(&self.pool)
        .await?;

        let nearest_checkpoint = cp_row.map(|r| StoredCheckpoint {
            event_id: r.get("event_id"),
            chain_head_hash: r.get("chain_head_hash"),
            signature: r.get("signature"),
            created_at: r.get("created_at"),
        });

        Ok(EventProof {
            event,
            nearest_checkpoint,
        })
    }
}

fn row_to_event(r: &sqlx::sqlite::SqliteRow) -> Event {
    let id: i64 = r.get("id");
    let prev_hash_bytes: Vec<u8> = r.get("prev_hash");
    let event_type_str: String = r.get("event_type");
    let actor_bytes: Option<Vec<u8>> = r.get("actor");
    let target_bytes: Option<Vec<u8>> = r.get("target");
    let payload_str: String = r.get("payload");
    let created_at: String = r.get("created_at");
    let hash_bytes: Vec<u8> = r.get("hash");

    let prev_hash: [u8; 32] = prev_hash_bytes.try_into().unwrap_or([0u8; 32]);
    let hash: [u8; 32] = hash_bytes.try_into().unwrap_or([0u8; 32]);

    let event_type: EventType = event_type_str.parse().unwrap_or(EventType::IdentityUpdated);

    let actor = actor_bytes.and_then(|b| {
        let arr: [u8; 32] = b.try_into().ok()?;
        Some(PublicKey::from_bytes(arr))
    });
    let target = target_bytes.and_then(|b| {
        let arr: [u8; 32] = b.try_into().ok()?;
        Some(PublicKey::from_bytes(arr))
    });

    let payload: serde_json::Value =
        serde_json::from_str(&payload_str).unwrap_or(serde_json::json!({}));

    Event {
        id: id as u64,
        prev_hash,
        event_type,
        actor,
        target,
        payload,
        created_at,
        hash,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::test_helpers;

    fn test_instance_key() -> PublicKey {
        PublicKey::from_bytes([42u8; 32])
    }

    fn test_signing_key() -> SigningKey {
        let mut rng = rand::rng();
        SigningKey::generate(&mut rng)
    }

    #[tokio::test]
    async fn append_and_query_events() {
        let repo = test_helpers::test_repository().await;
        let instance = test_instance_key();
        let actor = PublicKey::from_bytes([1u8; 32]);

        let id1 = repo
            .append_event(
                EventType::MemberJoined,
                Some(&actor),
                None,
                &serde_json::json!({"display_name": "Alice"}),
                &instance,
            )
            .await
            .unwrap();

        let id2 = repo
            .append_event(
                EventType::InviteCreated,
                Some(&actor),
                None,
                &serde_json::json!({"capability": "collaborate"}),
                &instance,
            )
            .await
            .unwrap();

        assert!(id2 > id1);

        let events = repo.query_events(None, None, 10, None).await.unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_type, EventType::MemberJoined);
        assert_eq!(events[1].event_type, EventType::InviteCreated);
    }

    #[tokio::test]
    async fn query_events_by_type_prefix() {
        let repo = test_helpers::test_repository().await;
        let instance = test_instance_key();

        repo.append_event(
            EventType::MemberJoined,
            None,
            None,
            &serde_json::json!({}),
            &instance,
        )
        .await
        .unwrap();

        repo.append_event(
            EventType::InviteCreated,
            None,
            None,
            &serde_json::json!({}),
            &instance,
        )
        .await
        .unwrap();

        repo.append_event(
            EventType::InviteRedeemed,
            None,
            None,
            &serde_json::json!({}),
            &instance,
        )
        .await
        .unwrap();

        let invite_events = repo
            .query_events(None, Some("invite."), 10, None)
            .await
            .unwrap();
        assert_eq!(invite_events.len(), 2);

        let member_events = repo
            .query_events(None, Some("member."), 10, None)
            .await
            .unwrap();
        assert_eq!(member_events.len(), 1);
    }

    #[tokio::test]
    async fn query_events_by_target() {
        let repo = test_helpers::test_repository().await;
        let instance = test_instance_key();
        let target1 = PublicKey::from_bytes([10u8; 32]);
        let target2 = PublicKey::from_bytes([20u8; 32]);

        repo.append_event(
            EventType::MemberSuspended,
            None,
            Some(&target1),
            &serde_json::json!({}),
            &instance,
        )
        .await
        .unwrap();

        repo.append_event(
            EventType::MemberSuspended,
            None,
            Some(&target2),
            &serde_json::json!({}),
            &instance,
        )
        .await
        .unwrap();

        let events = repo
            .query_events(Some(target1.as_bytes()), None, 10, None)
            .await
            .unwrap();
        assert_eq!(events.len(), 1);
    }

    #[tokio::test]
    async fn verify_chain_valid() {
        let repo = test_helpers::test_repository().await;
        let instance = test_instance_key();

        for i in 0..5 {
            repo.append_event(
                EventType::MemberJoined,
                None,
                None,
                &serde_json::json!({"seq": i}),
                &instance,
            )
            .await
            .unwrap();
        }

        let verification = repo.verify_chain(1, 5, &instance).await.unwrap();
        assert!(verification.valid);
        assert_eq!(verification.events_checked, 5);
    }

    #[tokio::test]
    async fn verify_chain_detects_tamper() {
        let repo = test_helpers::test_repository().await;
        let instance = test_instance_key();

        for i in 0..5 {
            repo.append_event(
                EventType::MemberJoined,
                None,
                None,
                &serde_json::json!({"seq": i}),
                &instance,
            )
            .await
            .unwrap();
        }

        // Tamper with event 3's payload directly in DB
        sqlx::query("UPDATE event_log SET payload = '{\"tampered\": true}' WHERE id = 3")
            .execute(&repo.pool)
            .await
            .unwrap();

        let verification = repo.verify_chain(1, 5, &instance).await.unwrap();
        assert!(!verification.valid);
        assert!(verification.error.is_some());
    }

    #[tokio::test]
    async fn chain_head() {
        let repo = test_helpers::test_repository().await;

        // Empty log
        let head = repo.get_chain_head().await.unwrap();
        assert!(head.is_none());

        let instance = test_instance_key();
        repo.append_event(
            EventType::MemberJoined,
            None,
            None,
            &serde_json::json!({}),
            &instance,
        )
        .await
        .unwrap();

        let head = repo.get_chain_head().await.unwrap().unwrap();
        assert_eq!(head.0, 1); // SQLite AUTOINCREMENT starts at 1
        assert_eq!(head.1.len(), 32);
    }

    #[tokio::test]
    async fn checkpoint_roundtrip() {
        let repo = test_helpers::test_repository().await;
        let instance = test_instance_key();
        let sk = test_signing_key();

        let event_id = repo
            .append_event(
                EventType::MemberJoined,
                None,
                None,
                &serde_json::json!({}),
                &instance,
            )
            .await
            .unwrap();

        repo.create_checkpoint(event_id, &sk).await.unwrap();

        let proof = repo.get_event_proof(event_id).await.unwrap();
        assert!(proof.nearest_checkpoint.is_some());

        let cp = proof.nearest_checkpoint.unwrap();
        assert_eq!(cp.event_id, event_id);
        assert_eq!(cp.chain_head_hash.len(), 32);
        assert_eq!(cp.signature.len(), 64);
    }

    #[tokio::test]
    async fn checkpoint_signature_verifies() {
        let repo = test_helpers::test_repository().await;
        let instance = test_instance_key();
        let sk = test_signing_key();
        let pk = sk.public_key();

        let event_id = repo
            .append_event(
                EventType::InviteCreated,
                None,
                None,
                &serde_json::json!({}),
                &instance,
            )
            .await
            .unwrap();

        repo.create_checkpoint(event_id, &sk).await.unwrap();

        let proof = repo.get_event_proof(event_id).await.unwrap();
        let cp = proof.nearest_checkpoint.unwrap();

        let chain_head_hash: [u8; 32] = cp.chain_head_hash.try_into().unwrap();
        let sig_bytes: [u8; 64] = cp.signature.try_into().unwrap();
        let checkpoint = EventCheckpoint {
            event_id: cp.event_id as u64,
            chain_head_hash,
            signature: crab_city_auth::Signature::from_bytes(sig_bytes),
            created_at: cp.created_at,
        };

        assert!(checkpoint.verify(&pk).is_ok());
    }

    #[tokio::test]
    async fn event_proof_without_checkpoint() {
        let repo = test_helpers::test_repository().await;
        let instance = test_instance_key();

        let event_id = repo
            .append_event(
                EventType::MemberJoined,
                None,
                None,
                &serde_json::json!({}),
                &instance,
            )
            .await
            .unwrap();

        let proof = repo.get_event_proof(event_id).await.unwrap();
        assert!(proof.nearest_checkpoint.is_none());
        assert!(proof.event.verify_hash());
    }

    #[tokio::test]
    async fn append_100_events_and_verify() {
        let repo = test_helpers::test_repository().await;
        let instance = test_instance_key();

        for i in 0..100 {
            repo.append_event(
                EventType::MemberJoined,
                None,
                None,
                &serde_json::json!({"seq": i}),
                &instance,
            )
            .await
            .unwrap();
        }

        let verification = repo.verify_chain(1, 100, &instance).await.unwrap();
        assert!(verification.valid);
        assert_eq!(verification.events_checked, 100);
    }

    #[tokio::test]
    async fn query_events_pagination() {
        let repo = test_helpers::test_repository().await;
        let instance = test_instance_key();

        for i in 0..10 {
            repo.append_event(
                EventType::MemberJoined,
                None,
                None,
                &serde_json::json!({"seq": i}),
                &instance,
            )
            .await
            .unwrap();
        }

        // Get first 3
        let page1 = repo.query_events(None, None, 3, None).await.unwrap();
        assert_eq!(page1.len(), 3);

        // Get next 3 before the first of previous page
        let last_id = page1.first().unwrap().id as i64;
        let page2 = repo
            .query_events(None, None, 3, Some(last_id))
            .await
            .unwrap();
        assert_eq!(page2.len(), 3);

        // Pages should not overlap
        let page1_ids: Vec<u64> = page1.iter().map(|e| e.id).collect();
        let page2_ids: Vec<u64> = page2.iter().map(|e| e.id).collect();
        for id in &page2_ids {
            assert!(!page1_ids.contains(id));
        }
    }
}
