use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use chrono::{DateTime, Duration, Utc};
use rusqlite::{Connection, OptionalExtension, Row, Transaction, TransactionBehavior, params};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    auth::{generate_token, hash_token, verify_token},
    domain::{
        Actor, ActorKind, EmbeddingStatus, IssuedActor, Memory, MemoryChange, MemoryDraft,
        MemoryIndex, MemoryKind, MemoryStatus, NewActor, SourceType,
    },
    error::{CoreError, CoreResult},
};

#[derive(Debug, Clone)]
pub struct Storage {
    path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct StoredEmbedding {
    pub memory_id: Uuid,
    pub vector: Vec<f32>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct IndexStatus {
    pub pending: usize,
    pub ready: usize,
    pub failed: usize,
}

const SESSION_TTL_MINUTES: i64 = 10;
const COMMIT_CLAIM_TIMEOUT_SECONDS: i64 = 30;
const CLEANUP_BATCH_SIZE: i64 = 100;

#[derive(Debug, Clone)]
pub struct PreparedMemoryChange {
    pub change: MemoryChange,
    pub committed_result: Option<Value>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct MatchRejection {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Uuid>,
    pub code: String,
}

#[derive(Debug, Clone)]
pub enum MatchSelection {
    Accepted(Vec<Uuid>),
    Rejected {
        reasons: Vec<MatchRejection>,
        retryable: bool,
    },
}

impl Storage {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn connect(&self) -> CoreResult<Connection> {
        let connection = Connection::open(&self.path)?;
        connection.execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA journal_mode = WAL;
             PRAGMA busy_timeout = 5000;",
        )?;
        Ok(connection)
    }

    pub fn migrate(&self) -> CoreResult<()> {
        let connection = self.connect()?;
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS actors (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL UNIQUE,
                kind TEXT NOT NULL CHECK(kind IN ('user', 'agent')),
                can_read INTEGER NOT NULL,
                can_create INTEGER NOT NULL,
                can_confirm_user_changes INTEGER NOT NULL DEFAULT 0,
                can_manage INTEGER NOT NULL,
                token_hash TEXT NOT NULL,
                revoked INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL
             );

             CREATE TABLE IF NOT EXISTS memories (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                kind TEXT NOT NULL CHECK(kind IN ('rule', 'procedure')),
                body_json TEXT NOT NULL,
                body_text TEXT NOT NULL,
                status TEXT NOT NULL CHECK(status IN ('enabled', 'disabled', 'archived')),
                source_type TEXT NOT NULL CHECK(source_type IN ('user', 'agent')),
                source_agent TEXT,
                retrieval_text TEXT NOT NULL,
                body_version INTEGER NOT NULL DEFAULT 1,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
             );

             CREATE TABLE IF NOT EXISTS memory_facets (
                memory_id TEXT NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
                facet_kind TEXT NOT NULL,
                value TEXT NOT NULL,
                PRIMARY KEY(memory_id, facet_kind, value)
             );
             CREATE INDEX IF NOT EXISTS idx_memory_facets_lookup
                ON memory_facets(facet_kind, value, memory_id);

             CREATE TABLE IF NOT EXISTS memory_embeddings (
                memory_id TEXT PRIMARY KEY REFERENCES memories(id) ON DELETE CASCADE,
                model TEXT NOT NULL,
                dimensions INTEGER NOT NULL,
                vector BLOB,
                content_hash TEXT NOT NULL,
                status TEXT NOT NULL CHECK(status IN ('pending', 'ready', 'failed')),
                last_error TEXT,
                updated_at TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_embeddings_status
                ON memory_embeddings(status);

             CREATE VIRTUAL TABLE IF NOT EXISTS memory_fts USING fts5(
                memory_id UNINDEXED,
                name,
                body,
                keywords,
                retrieval_text,
                tokenize='trigram'
             );

             CREATE TABLE IF NOT EXISTS match_sessions (
                id TEXT PRIMARY KEY,
                actor_id TEXT NOT NULL REFERENCES actors(id),
                expires_at TEXT NOT NULL,
                consumed_at TEXT,
                selected_json TEXT
             );
             CREATE TABLE IF NOT EXISTS match_candidates (
                match_id TEXT NOT NULL REFERENCES match_sessions(id) ON DELETE CASCADE,
                memory_id TEXT NOT NULL,
                kind TEXT NOT NULL,
                body_version INTEGER NOT NULL,
                position INTEGER NOT NULL,
                PRIMARY KEY(match_id, memory_id)
             );

             CREATE TABLE IF NOT EXISTS memory_changes (
                id TEXT PRIMARY KEY,
                actor_id TEXT NOT NULL REFERENCES actors(id),
                change_json TEXT NOT NULL,
                preview_json TEXT NOT NULL,
                expected_body_version INTEGER,
                expected_updated_at TEXT,
                expires_at TEXT NOT NULL,
                committed_at TEXT,
                result_json TEXT
             );

             PRAGMA user_version = 2;",
        )?;
        if !table_has_column(&connection, "actors", "can_confirm_user_changes")? {
            connection.execute(
                "ALTER TABLE actors ADD COLUMN can_confirm_user_changes INTEGER NOT NULL DEFAULT 0",
                [],
            )?;
        }
        if !table_has_column(&connection, "memory_changes", "expected_updated_at")? {
            connection.execute(
                "ALTER TABLE memory_changes ADD COLUMN expected_updated_at TEXT",
                [],
            )?;
        }
        Ok(())
    }

    pub fn bootstrap_admin(&self, name: &str) -> CoreResult<Option<IssuedActor>> {
        if self.count_actors()? > 0 {
            return Ok(None);
        }
        let actor = NewActor {
            name: name.to_owned(),
            kind: ActorKind::User,
            can_read: true,
            can_create: true,
            can_confirm_user_changes: false,
        };
        self.insert_actor(actor, true).map(Some)
    }

    fn count_actors(&self) -> CoreResult<usize> {
        let connection = self.connect()?;
        let count: i64 =
            connection.query_row("SELECT COUNT(*) FROM actors", [], |row| row.get(0))?;
        usize::try_from(count).map_err(|error| CoreError::Internal(error.to_string()))
    }

    pub fn create_actor(&self, mut input: NewActor) -> CoreResult<IssuedActor> {
        input.name = input.name.trim().to_owned();
        if input.name.is_empty() {
            return Err(CoreError::Validation("actor name cannot be empty".into()));
        }
        if input.kind == ActorKind::User {
            return Err(CoreError::Validation(
                "additional user administrators are not supported in v1".into(),
            ));
        }
        self.insert_actor(input, false)
    }

    fn insert_actor(&self, input: NewActor, can_manage: bool) -> CoreResult<IssuedActor> {
        let connection = self.connect()?;
        let id = Uuid::now_v7();
        let token = generate_token();
        let token_hash = hash_token(&token)?;
        let now = Utc::now();
        connection
            .execute(
                "INSERT INTO actors
                 (id, name, kind, can_read, can_create, can_confirm_user_changes, can_manage,
                  token_hash, revoked, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0, ?9)",
                params![
                    id.to_string(),
                    input.name,
                    input.kind.as_str(),
                    input.can_read,
                    input.can_create,
                    input.can_confirm_user_changes,
                    can_manage,
                    token_hash,
                    now.to_rfc3339(),
                ],
            )
            .map_err(map_unique_conflict("actor name already exists"))?;
        Ok(IssuedActor {
            actor: Actor {
                id,
                name: input.name,
                kind: input.kind,
                can_read: input.can_read,
                can_create: input.can_create,
                can_confirm_user_changes: input.can_confirm_user_changes,
                can_manage,
                revoked: false,
                created_at: now,
            },
            token,
        })
    }

    pub fn authenticate(&self, token: &str) -> CoreResult<Actor> {
        if token.trim().is_empty() {
            return Err(CoreError::Unauthorized);
        }
        let connection = self.connect()?;
        let mut statement = connection.prepare(
            "SELECT id, name, kind, can_read, can_create, can_confirm_user_changes, can_manage,
                    revoked, created_at, token_hash
             FROM actors WHERE revoked = 0",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((actor_from_row(row)?, row.get::<_, String>(9)?))
        })?;
        for row in rows {
            let (actor, encoded_hash) = row?;
            if verify_token(token, &encoded_hash) {
                return Ok(actor);
            }
        }
        Err(CoreError::Unauthorized)
    }

    pub fn list_actors(&self) -> CoreResult<Vec<Actor>> {
        let connection = self.connect()?;
        let mut statement = connection.prepare(
            "SELECT id, name, kind, can_read, can_create, can_confirm_user_changes, can_manage,
                    revoked, created_at
             FROM actors ORDER BY created_at",
        )?;
        let rows = statement.query_map([], actor_from_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn revoke_actor(&self, id: Uuid) -> CoreResult<Actor> {
        let connection = self.connect()?;
        let changed = connection.execute(
            "UPDATE actors SET revoked = 1 WHERE id = ?1 AND kind = 'agent'",
            [id.to_string()],
        )?;
        if changed == 0 {
            return Err(CoreError::NotFound(format!("actor {id}")));
        }
        self.get_actor(id)
    }

    fn get_actor(&self, id: Uuid) -> CoreResult<Actor> {
        let connection = self.connect()?;
        connection
            .query_row(
                "SELECT id, name, kind, can_read, can_create, can_confirm_user_changes, can_manage,
                        revoked, created_at
                 FROM actors WHERE id = ?1",
                [id.to_string()],
                actor_from_row,
            )
            .optional()?
            .ok_or_else(|| CoreError::NotFound(format!("actor {id}")))
    }

    pub fn create_memory(
        &self,
        draft: MemoryDraft,
        actor: &Actor,
        model: &str,
        dimensions: usize,
        content_hash: &str,
    ) -> CoreResult<Memory> {
        let source_type = match actor.kind {
            ActorKind::User => SourceType::User,
            ActorKind::Agent => SourceType::Agent,
        };
        let source_agent = (actor.kind == ActorKind::Agent).then_some(actor.name.as_str());
        self.create_memory_with_source(
            (Uuid::now_v7(), None),
            draft,
            (source_type, source_agent),
            model,
            dimensions,
            content_hash,
        )
    }

    pub fn create_user_memory(
        &self,
        draft: MemoryDraft,
        model: &str,
        dimensions: usize,
        content_hash: &str,
    ) -> CoreResult<Memory> {
        self.create_memory_with_source(
            (Uuid::now_v7(), None),
            draft,
            (SourceType::User, None),
            model,
            dimensions,
            content_hash,
        )
    }

    pub fn create_user_memory_for_change(
        &self,
        change_id: Uuid,
        draft: MemoryDraft,
        model: &str,
        dimensions: usize,
        content_hash: &str,
    ) -> CoreResult<Memory> {
        self.create_memory_with_source(
            (change_id, Some(change_id)),
            draft,
            (SourceType::User, None),
            model,
            dimensions,
            content_hash,
        )
    }

    fn create_memory_with_source(
        &self,
        identity: (Uuid, Option<Uuid>),
        mut draft: MemoryDraft,
        source: (SourceType, Option<&str>),
        model: &str,
        dimensions: usize,
        content_hash: &str,
    ) -> CoreResult<Memory> {
        draft.validate_and_normalize()?;
        let (id, change_id) = identity;
        let mut connection = self.connect()?;
        let transaction = connection.transaction()?;
        let now = Utc::now();
        let (source_type, source_agent) = source;
        let body_json = serde_json::to_string(&draft.content)
            .map_err(|error| CoreError::Internal(error.to_string()))?;
        let body_text = draft.content.plain_text();
        transaction.execute(
            "INSERT INTO memories
             (id, name, kind, body_json, body_text, status, source_type, source_agent,
              retrieval_text, body_version, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 'enabled', ?6, ?7, ?8, 1, ?9, ?9)",
            params![
                id.to_string(),
                draft.name,
                draft.content.kind().as_str(),
                body_json,
                body_text,
                source_type.as_str(),
                source_agent,
                draft.index.retrieval_text,
                now.to_rfc3339(),
            ],
        )?;
        replace_facets(&transaction, id, &draft.index)?;
        upsert_fts(&transaction, id, &draft.name, &body_text, &draft.index)?;
        transaction.execute(
            "INSERT INTO memory_embeddings
             (memory_id, model, dimensions, vector, content_hash, status, last_error, updated_at)
             VALUES (?1, ?2, ?3, NULL, ?4, 'pending', NULL, ?5)",
            params![
                id.to_string(),
                model,
                dimensions as i64,
                content_hash,
                now.to_rfc3339()
            ],
        )?;
        let memory = load_memory(&transaction, id)?
            .ok_or_else(|| CoreError::Internal("created memory was not found".into()))?;
        if let Some(change_id) = change_id {
            store_change_result(&transaction, change_id, &to_json_value(&memory)?)?;
        }
        transaction.commit()?;
        Ok(memory)
    }

    pub fn update_memory(
        &self,
        id: Uuid,
        draft: MemoryDraft,
        model: &str,
        dimensions: usize,
        content_hash: &str,
    ) -> CoreResult<Memory> {
        self.update_memory_internal(id, draft, model, dimensions, content_hash, None)
    }

    pub fn update_memory_for_change(
        &self,
        change_id: Uuid,
        id: Uuid,
        draft: MemoryDraft,
        model: &str,
        dimensions: usize,
        content_hash: &str,
    ) -> CoreResult<Memory> {
        self.update_memory_internal(id, draft, model, dimensions, content_hash, Some(change_id))
    }

    fn update_memory_internal(
        &self,
        id: Uuid,
        mut draft: MemoryDraft,
        model: &str,
        dimensions: usize,
        content_hash: &str,
        change_id: Option<Uuid>,
    ) -> CoreResult<Memory> {
        draft.validate_and_normalize()?;
        let mut connection = self.connect()?;
        let transaction = connection.transaction()?;
        let body_json = serde_json::to_string(&draft.content)
            .map_err(|error| CoreError::Internal(error.to_string()))?;
        let body_text = draft.content.plain_text();
        let now = Utc::now().to_rfc3339();
        let changed = transaction.execute(
            "UPDATE memories SET name = ?2, kind = ?3, body_json = ?4, body_text = ?5,
             retrieval_text = ?6, body_version = body_version + 1, updated_at = ?7 WHERE id = ?1",
            params![
                id.to_string(),
                draft.name,
                draft.content.kind().as_str(),
                body_json,
                body_text,
                draft.index.retrieval_text,
                now,
            ],
        )?;
        if changed == 0 {
            return Err(CoreError::NotFound(format!("memory {id}")));
        }
        replace_facets(&transaction, id, &draft.index)?;
        upsert_fts(&transaction, id, &draft.name, &body_text, &draft.index)?;
        transaction.execute(
            "INSERT INTO memory_embeddings
             (memory_id, model, dimensions, vector, content_hash, status, last_error, updated_at)
             VALUES (?1, ?2, ?3, NULL, ?4, 'pending', NULL, ?5)
             ON CONFLICT(memory_id) DO UPDATE SET model = excluded.model,
             dimensions = excluded.dimensions, vector = NULL, content_hash = excluded.content_hash,
             status = 'pending', last_error = NULL, updated_at = excluded.updated_at",
            params![id.to_string(), model, dimensions as i64, content_hash, now],
        )?;
        let memory = load_memory(&transaction, id)?
            .ok_or_else(|| CoreError::Internal("updated memory was not found".into()))?;
        if let Some(change_id) = change_id {
            store_change_result(&transaction, change_id, &to_json_value(&memory)?)?;
        }
        transaction.commit()?;
        Ok(memory)
    }

    pub fn get_memory(&self, id: Uuid) -> CoreResult<Memory> {
        let connection = self.connect()?;
        load_memory(&connection, id)?.ok_or_else(|| CoreError::NotFound(format!("memory {id}")))
    }

    pub fn list_memories(
        &self,
        status: Option<MemoryStatus>,
        kind: Option<MemoryKind>,
        source_agent: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> CoreResult<Vec<Memory>> {
        let connection = self.connect()?;
        let mut statement = connection.prepare(
            "SELECT id FROM memories
             WHERE (?1 IS NULL OR status = ?1)
               AND (?2 IS NULL OR kind = ?2)
               AND (?3 IS NULL OR source_agent = ?3)
             ORDER BY updated_at DESC LIMIT ?4 OFFSET ?5",
        )?;
        let status = status.map(MemoryStatus::as_str);
        let kind = kind.map(MemoryKind::as_str);
        let ids = statement
            .query_map(
                params![status, kind, source_agent, limit as i64, offset as i64],
                |row| row.get::<_, String>(0),
            )?
            .collect::<Result<Vec<_>, _>>()?;
        ids.into_iter()
            .map(|id| {
                parse_uuid(&id).and_then(|id| {
                    load_memory(&connection, id)?
                        .ok_or_else(|| CoreError::NotFound(format!("memory {id}")))
                })
            })
            .collect()
    }

    pub fn set_memory_status(&self, id: Uuid, status: MemoryStatus) -> CoreResult<Memory> {
        self.set_memory_status_internal(id, status, None)
    }

    pub fn set_memory_status_for_change(
        &self,
        change_id: Uuid,
        id: Uuid,
        status: MemoryStatus,
    ) -> CoreResult<Memory> {
        self.set_memory_status_internal(id, status, Some(change_id))
    }

    fn set_memory_status_internal(
        &self,
        id: Uuid,
        status: MemoryStatus,
        change_id: Option<Uuid>,
    ) -> CoreResult<Memory> {
        let mut connection = self.connect()?;
        let transaction = connection.transaction()?;
        let changed = transaction.execute(
            "UPDATE memories SET status = ?2, updated_at = ?3 WHERE id = ?1",
            params![id.to_string(), status.as_str(), Utc::now().to_rfc3339()],
        )?;
        if changed == 0 {
            return Err(CoreError::NotFound(format!("memory {id}")));
        }
        let memory = load_memory(&transaction, id)?
            .ok_or_else(|| CoreError::Internal("updated memory was not found".into()))?;
        if let Some(change_id) = change_id {
            store_change_result(&transaction, change_id, &to_json_value(&memory)?)?;
        }
        transaction.commit()?;
        Ok(memory)
    }

    pub fn delete_memory(&self, id: Uuid) -> CoreResult<()> {
        self.delete_memory_internal(id, None)
    }

    pub fn delete_memory_for_change(&self, change_id: Uuid, id: Uuid) -> CoreResult<()> {
        self.delete_memory_internal(id, Some(change_id))
    }

    fn delete_memory_internal(&self, id: Uuid, change_id: Option<Uuid>) -> CoreResult<()> {
        let mut connection = self.connect()?;
        let transaction = connection.transaction()?;
        transaction.execute(
            "DELETE FROM memory_fts WHERE memory_id = ?1",
            [id.to_string()],
        )?;
        let changed =
            transaction.execute("DELETE FROM memories WHERE id = ?1", [id.to_string()])?;
        if changed == 0 {
            return Err(CoreError::NotFound(format!("memory {id}")));
        }
        if let Some(change_id) = change_id {
            store_change_result(
                &transaction,
                change_id,
                &json!({ "deleted": true, "id": id }),
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn enabled_memories(&self) -> CoreResult<Vec<Memory>> {
        self.list_memories(Some(MemoryStatus::Enabled), None, None, 10_000, 0)
    }

    pub fn full_text_search(&self, query: &str, limit: usize) -> CoreResult<Vec<(Uuid, f64)>> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(Vec::new());
        }
        let connection = self.connect()?;
        let mut output = Vec::new();
        if query.chars().count() < 3 {
            let escaped = query
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_");
            let pattern = format!("%{escaped}%");
            let mut statement = connection.prepare(
                "SELECT id FROM memories WHERE status = 'enabled' AND
                 (name LIKE ?1 ESCAPE '\\' OR body_text LIKE ?1 ESCAPE '\\' OR
                  retrieval_text LIKE ?1 ESCAPE '\\') LIMIT ?2",
            )?;
            let ids = statement.query_map(params![pattern, limit as i64], |row| {
                row.get::<_, String>(0)
            })?;
            for (rank, id) in ids.enumerate() {
                output.push((parse_uuid(&id?)?, 1.0 / (rank as f64 + 1.0)));
            }
            return Ok(output);
        }
        let safe_query = query
            .split_whitespace()
            .filter(|term| term.chars().count() >= 3)
            .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(" OR ");
        if safe_query.is_empty() {
            return Ok(Vec::new());
        }
        let mut statement = connection.prepare(
            "SELECT memory_id, bm25(memory_fts) AS rank FROM memory_fts
             JOIN memories ON memories.id = memory_fts.memory_id
             WHERE memory_fts MATCH ?1 AND memories.status = 'enabled'
             ORDER BY rank LIMIT ?2",
        )?;
        let rows = statement.query_map(params![safe_query, limit as i64], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?))
        })?;
        for row in rows {
            let (id, score) = row?;
            output.push((parse_uuid(&id)?, -score));
        }
        Ok(output)
    }

    pub fn structured_search(
        &self,
        facets: &[(String, String)],
        limit: usize,
    ) -> CoreResult<Vec<(Uuid, f64)>> {
        if facets.is_empty() {
            return Ok(Vec::new());
        }
        let connection = self.connect()?;
        let enabled = self.enabled_memories()?;
        let mut scored = Vec::new();
        for memory in enabled {
            let memory_facets = facet_pairs(&memory.index);
            let matches = facets
                .iter()
                .filter(|facet| memory_facets.contains(facet))
                .count();
            if matches > 0 {
                scored.push((memory.id, matches as f64 / facets.len() as f64));
            }
        }
        drop(connection);
        scored.sort_by(|left, right| right.1.total_cmp(&left.1));
        scored.truncate(limit);
        Ok(scored)
    }

    pub fn ready_embeddings(
        &self,
        model: &str,
        dimensions: usize,
    ) -> CoreResult<Vec<StoredEmbedding>> {
        let connection = self.connect()?;
        let mut statement = connection.prepare(
            "SELECT memory_id, vector FROM memory_embeddings
             JOIN memories ON memories.id = memory_embeddings.memory_id
             WHERE memory_embeddings.status = 'ready' AND model = ?1 AND dimensions = ?2
               AND memories.status = 'enabled'",
        )?;
        let rows = statement.query_map(params![model, dimensions as i64], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
        })?;
        rows.map(|row| {
            let (id, bytes) = row?;
            Ok(StoredEmbedding {
                memory_id: parse_uuid(&id)?,
                vector: decode_vector(&bytes, dimensions)?,
            })
        })
        .collect()
    }

    pub fn mark_embedding_ready(
        &self,
        id: Uuid,
        model: &str,
        vector: &[f32],
        content_hash: &str,
    ) -> CoreResult<()> {
        let connection = self.connect()?;
        let bytes = encode_vector(vector);
        connection.execute(
            "UPDATE memory_embeddings SET model = ?2, dimensions = ?3, vector = ?4,
             content_hash = ?5, status = 'ready', last_error = NULL, updated_at = ?6
             WHERE memory_id = ?1",
            params![
                id.to_string(),
                model,
                vector.len() as i64,
                bytes,
                content_hash,
                Utc::now().to_rfc3339()
            ],
        )?;
        Ok(())
    }

    pub fn mark_embedding_pending(&self, id: Uuid, error: &str) -> CoreResult<()> {
        let connection = self.connect()?;
        connection.execute(
            "UPDATE memory_embeddings SET vector = NULL, status = 'pending', last_error = ?2,
             updated_at = ?3 WHERE memory_id = ?1",
            params![
                id.to_string(),
                truncate_error(error),
                Utc::now().to_rfc3339()
            ],
        )?;
        Ok(())
    }

    pub fn memories_needing_embeddings(
        &self,
        model: &str,
        dimensions: usize,
    ) -> CoreResult<Vec<Memory>> {
        let connection = self.connect()?;
        let mut statement = connection.prepare(
            "SELECT memories.id FROM memories JOIN memory_embeddings
             ON memories.id = memory_embeddings.memory_id
             WHERE memory_embeddings.status != 'ready'
                OR memory_embeddings.model != ?1
                OR memory_embeddings.dimensions != ?2
             ORDER BY memories.updated_at",
        )?;
        let ids = statement
            .query_map(params![model, dimensions as i64], |row| {
                row.get::<_, String>(0)
            })?
            .collect::<Result<Vec<_>, _>>()?;
        ids.into_iter()
            .map(|id| {
                parse_uuid(&id).and_then(|id| {
                    load_memory(&connection, id)?
                        .ok_or_else(|| CoreError::NotFound(format!("memory {id}")))
                })
            })
            .collect()
    }

    pub fn index_status(&self) -> CoreResult<IndexStatus> {
        let connection = self.connect()?;
        let mut counts = HashMap::new();
        let mut statement =
            connection.prepare("SELECT status, COUNT(*) FROM memory_embeddings GROUP BY status")?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        for row in rows {
            let (status, count) = row?;
            counts.insert(
                status,
                usize::try_from(count).map_err(|error| CoreError::Internal(error.to_string()))?,
            );
        }
        Ok(IndexStatus {
            pending: counts.get("pending").copied().unwrap_or(0),
            ready: counts.get("ready").copied().unwrap_or(0),
            failed: counts.get("failed").copied().unwrap_or(0),
        })
    }

    pub fn create_match_session(
        &self,
        actor: &Actor,
        candidates: &[Memory],
    ) -> CoreResult<(Uuid, DateTime<Utc>)> {
        let mut connection = self.connect()?;
        cleanup_expired(&connection)?;
        let transaction = connection.transaction()?;
        let match_id = Uuid::now_v7();
        let expires_at = Utc::now() + Duration::minutes(SESSION_TTL_MINUTES);
        transaction.execute(
            "INSERT INTO match_sessions(id, actor_id, expires_at) VALUES (?1, ?2, ?3)",
            params![
                match_id.to_string(),
                actor.id.to_string(),
                expires_at.to_rfc3339()
            ],
        )?;
        for (position, memory) in candidates.iter().enumerate() {
            transaction.execute(
                "INSERT INTO match_candidates(match_id, memory_id, kind, body_version, position)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    match_id.to_string(),
                    memory.id.to_string(),
                    memory.content.kind().as_str(),
                    memory.body_version,
                    position as i64,
                ],
            )?;
        }
        transaction.commit()?;
        Ok((match_id, expires_at))
    }

    pub fn select_match(
        &self,
        actor: &Actor,
        match_id: Uuid,
        selected_ids: &[Uuid],
    ) -> CoreResult<MatchSelection> {
        let mut connection = self.connect()?;
        cleanup_expired(&connection)?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let session = transaction
            .query_row(
                "SELECT actor_id, expires_at, consumed_at, selected_json
                 FROM match_sessions WHERE id = ?1",
                [match_id.to_string()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                    ))
                },
            )
            .optional()?;
        let Some((actor_id, expires_at, consumed_at, stored_selection)) = session else {
            return Err(CoreError::NotFound(format!("match session {match_id}")));
        };
        if actor_id != actor.id.to_string() || parse_time(&expires_at)? <= Utc::now() {
            return Err(CoreError::NotFound(format!("match session {match_id}")));
        }
        let selection_key = canonical_selection(selected_ids)?;
        if consumed_at.is_some() {
            if stored_selection.as_deref() == Some(selection_key.as_str()) {
                return Ok(MatchSelection::Accepted(selected_ids.to_vec()));
            }
            return Ok(MatchSelection::Rejected {
                reasons: vec![MatchRejection {
                    id: None,
                    code: "match_already_consumed".into(),
                }],
                retryable: false,
            });
        }

        let mut reasons = Vec::new();
        if selected_ids.len() > 3 {
            reasons.push(MatchRejection {
                id: None,
                code: "too_many_selected".into(),
            });
        }
        let mut seen = std::collections::HashSet::new();
        for id in selected_ids {
            if !seen.insert(*id) {
                reasons.push(MatchRejection {
                    id: Some(*id),
                    code: "duplicate_selected_id".into(),
                });
            }
        }
        let mut procedures = 0;
        for id in seen {
            let snapshot = transaction
                .query_row(
                    "SELECT kind, body_version FROM match_candidates
                     WHERE match_id = ?1 AND memory_id = ?2",
                    params![match_id.to_string(), id.to_string()],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, u32>(1)?)),
                )
                .optional()?;
            let Some((kind, body_version)) = snapshot else {
                reasons.push(MatchRejection {
                    id: Some(id),
                    code: "not_a_candidate".into(),
                });
                continue;
            };
            if kind == MemoryKind::Procedure.as_str() {
                procedures += 1;
            }
            match load_memory(&transaction, id)? {
                None => reasons.push(MatchRejection {
                    id: Some(id),
                    code: "memory_unavailable".into(),
                }),
                Some(memory) if memory.status != MemoryStatus::Enabled => {
                    reasons.push(MatchRejection {
                        id: Some(id),
                        code: "memory_not_enabled".into(),
                    });
                }
                Some(memory) if memory.body_version != body_version => {
                    reasons.push(MatchRejection {
                        id: Some(id),
                        code: "body_version_changed".into(),
                    });
                }
                Some(_) => {}
            }
        }
        if procedures > 1 {
            reasons.push(MatchRejection {
                id: None,
                code: "too_many_procedures".into(),
            });
        }
        if !reasons.is_empty() {
            return Ok(MatchSelection::Rejected {
                reasons,
                retryable: true,
            });
        }
        transaction.execute(
            "UPDATE match_sessions SET consumed_at = ?2, selected_json = ?3 WHERE id = ?1",
            params![match_id.to_string(), Utc::now().to_rfc3339(), selection_key],
        )?;
        transaction.commit()?;
        Ok(MatchSelection::Accepted(selected_ids.to_vec()))
    }

    pub fn prepare_memory_change(
        &self,
        actor: &Actor,
        change: &MemoryChange,
        preview: &Value,
    ) -> CoreResult<(Uuid, DateTime<Utc>)> {
        let (expected_body_version, expected_updated_at) = match change {
            MemoryChange::Create { .. } => (None, None),
            MemoryChange::Update { id, .. }
            | MemoryChange::SetStatus { id, .. }
            | MemoryChange::Delete { id } => {
                let memory = self.get_memory(*id)?;
                (
                    Some(memory.body_version),
                    Some(memory.updated_at.to_rfc3339()),
                )
            }
        };
        let id = Uuid::now_v7();
        let expires_at = Utc::now() + Duration::minutes(SESSION_TTL_MINUTES);
        let change_json = serde_json::to_string(change)
            .map_err(|error| CoreError::Internal(error.to_string()))?;
        let preview_json = serde_json::to_string(preview)
            .map_err(|error| CoreError::Internal(error.to_string()))?;
        let mut connection = self.connect()?;
        cleanup_expired(&connection)?;
        let transaction = connection.transaction()?;
        transaction.execute(
            "INSERT INTO memory_changes
             (id, actor_id, change_json, preview_json, expected_body_version,
              expected_updated_at, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                id.to_string(),
                actor.id.to_string(),
                change_json,
                preview_json,
                expected_body_version,
                expected_updated_at,
                expires_at.to_rfc3339(),
            ],
        )?;
        transaction.commit()?;
        Ok((id, expires_at))
    }

    pub fn begin_memory_change_commit(
        &self,
        actor: &Actor,
        change_id: Uuid,
    ) -> CoreResult<PreparedMemoryChange> {
        let mut connection = self.connect()?;
        cleanup_expired(&connection)?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let row = transaction
            .query_row(
                "SELECT actor_id, change_json, expected_body_version, expected_updated_at,
                        expires_at, committed_at, result_json
                 FROM memory_changes WHERE id = ?1",
                [change_id.to_string()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<u32>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, Option<String>>(6)?,
                    ))
                },
            )
            .optional()?;
        let Some((
            actor_id,
            change_json,
            expected_version,
            expected_updated_at,
            expires_at,
            committed_at,
            result_json,
        )) = row
        else {
            return Err(CoreError::NotFound(format!("memory change {change_id}")));
        };
        if actor_id != actor.id.to_string() || parse_time(&expires_at)? <= Utc::now() {
            return Err(CoreError::NotFound(format!("memory change {change_id}")));
        }
        let change: MemoryChange = serde_json::from_str(&change_json).map_err(|error| {
            CoreError::Internal(format!("invalid stored memory change: {error}"))
        })?;
        if let Some(result_json) = result_json {
            let value = serde_json::from_str(&result_json).map_err(|error| {
                CoreError::Internal(format!("invalid stored memory change result: {error}"))
            })?;
            return Ok(PreparedMemoryChange {
                change,
                committed_result: Some(value),
            });
        }
        if let Some(committed_at) = committed_at {
            let claim_time = parse_time(&committed_at)?;
            if claim_time + Duration::seconds(COMMIT_CLAIM_TIMEOUT_SECONDS) > Utc::now() {
                return Err(CoreError::Conflict(
                    "memory change commit is in progress".into(),
                ));
            }
            transaction.execute(
                "UPDATE memory_changes SET committed_at = NULL
                 WHERE id = ?1 AND result_json IS NULL",
                [change_id.to_string()],
            )?;
            transaction.commit()?;
            return self.begin_memory_change_commit(actor, change_id);
        }
        if let Some(expected_version) = expected_version {
            let id = match &change {
                MemoryChange::Create { .. } => unreachable!("create has no expected version"),
                MemoryChange::Update { id, .. }
                | MemoryChange::SetStatus { id, .. }
                | MemoryChange::Delete { id } => *id,
            };
            let current = load_memory(&transaction, id)?
                .ok_or_else(|| CoreError::Conflict("memory changed after preparation".into()))?;
            if current.body_version != expected_version {
                return Err(CoreError::Conflict(
                    "memory changed after preparation".into(),
                ));
            }
            if expected_updated_at.as_deref() != Some(current.updated_at.to_rfc3339().as_str()) {
                return Err(CoreError::Conflict(
                    "memory changed after preparation".into(),
                ));
            }
        }
        transaction.execute(
            "UPDATE memory_changes SET committed_at = ?2 WHERE id = ?1",
            params![change_id.to_string(), Utc::now().to_rfc3339()],
        )?;
        transaction.commit()?;
        Ok(PreparedMemoryChange {
            change,
            committed_result: None,
        })
    }

    pub fn finish_memory_change(&self, change_id: Uuid, value: &Value) -> CoreResult<()> {
        let result_json =
            serde_json::to_string(value).map_err(|error| CoreError::Internal(error.to_string()))?;
        let connection = self.connect()?;
        connection.execute(
            "UPDATE memory_changes SET result_json = ?2 WHERE id = ?1",
            params![change_id.to_string(), result_json],
        )?;
        Ok(())
    }

    pub fn release_memory_change_commit(&self, change_id: Uuid) -> CoreResult<()> {
        let connection = self.connect()?;
        connection.execute(
            "UPDATE memory_changes SET committed_at = NULL
             WHERE id = ?1 AND result_json IS NULL",
            [change_id.to_string()],
        )?;
        Ok(())
    }
}

fn actor_from_row(row: &Row<'_>) -> rusqlite::Result<Actor> {
    let id: String = row.get(0)?;
    let kind: String = row.get(2)?;
    let created_at: String = row.get(8)?;
    Ok(Actor {
        id: Uuid::parse_str(&id).map_err(conversion_error)?,
        name: row.get(1)?,
        kind: ActorKind::try_from(kind.as_str()).map_err(conversion_error)?,
        can_read: row.get(3)?,
        can_create: row.get(4)?,
        can_confirm_user_changes: row.get(5)?,
        can_manage: row.get(6)?,
        revoked: row.get(7)?,
        created_at: DateTime::parse_from_rfc3339(&created_at)
            .map_err(conversion_error)?
            .with_timezone(&Utc),
    })
}

fn table_has_column(connection: &Connection, table: &str, column: &str) -> CoreResult<bool> {
    let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
    let names = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(names.iter().any(|name| name == column))
}

fn cleanup_expired(connection: &Connection) -> CoreResult<()> {
    let now = Utc::now().to_rfc3339();
    let stale_claim = (Utc::now() - Duration::seconds(COMMIT_CLAIM_TIMEOUT_SECONDS)).to_rfc3339();
    connection.execute(
        "DELETE FROM match_sessions WHERE id IN (
            SELECT id FROM match_sessions WHERE expires_at <= ?1 LIMIT ?2
         )",
        params![now, CLEANUP_BATCH_SIZE],
    )?;
    connection.execute(
        "DELETE FROM memory_changes WHERE id IN (
            SELECT id FROM memory_changes WHERE expires_at <= ?1 LIMIT ?2
         )",
        params![now, CLEANUP_BATCH_SIZE],
    )?;
    connection.execute(
        "UPDATE memory_changes SET committed_at = NULL WHERE id IN (
            SELECT id FROM memory_changes
            WHERE result_json IS NULL AND committed_at <= ?1 LIMIT ?2
         )",
        params![stale_claim, CLEANUP_BATCH_SIZE],
    )?;
    Ok(())
}

fn store_change_result(
    transaction: &Transaction<'_>,
    change_id: Uuid,
    value: &Value,
) -> CoreResult<()> {
    let result_json =
        serde_json::to_string(value).map_err(|error| CoreError::Internal(error.to_string()))?;
    transaction.execute(
        "UPDATE memory_changes SET result_json = ?2 WHERE id = ?1",
        params![change_id.to_string(), result_json],
    )?;
    Ok(())
}

fn to_json_value(value: &impl serde::Serialize) -> CoreResult<Value> {
    serde_json::to_value(value).map_err(|error| CoreError::Internal(error.to_string()))
}

fn load_memory(connection: &Connection, id: Uuid) -> CoreResult<Option<Memory>> {
    let row = connection
        .query_row(
            "SELECT memories.id, name, body_json, memories.status, source_type, source_agent,
             retrieval_text, body_version, memory_embeddings.status, created_at, memories.updated_at
             FROM memories JOIN memory_embeddings ON memory_embeddings.memory_id = memories.id
             WHERE memories.id = ?1",
            [id.to_string()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, u32>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, String>(10)?,
                ))
            },
        )
        .optional()?;
    let Some((
        id_text,
        name,
        body_json,
        status,
        source_type,
        source_agent,
        retrieval_text,
        body_version,
        embedding_status,
        created_at,
        updated_at,
    )) = row
    else {
        return Ok(None);
    };
    let mut index = load_index(connection, id)?;
    index.retrieval_text = retrieval_text;
    Ok(Some(Memory {
        id: parse_uuid(&id_text)?,
        name,
        content: serde_json::from_str(&body_json)
            .map_err(|error| CoreError::Internal(format!("invalid stored memory body: {error}")))?,
        status: MemoryStatus::try_from(status.as_str())?,
        source_type: SourceType::try_from(source_type.as_str())?,
        source_agent,
        index,
        body_version,
        embedding_status: EmbeddingStatus::try_from(embedding_status.as_str())?,
        created_at: parse_time(&created_at)?,
        updated_at: parse_time(&updated_at)?,
    }))
}

fn load_index(connection: &Connection, id: Uuid) -> CoreResult<MemoryIndex> {
    let mut index = MemoryIndex::default();
    let mut statement = connection.prepare(
        "SELECT facet_kind, value FROM memory_facets WHERE memory_id = ?1 ORDER BY value",
    )?;
    let rows = statement.query_map([id.to_string()], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (kind, value) = row?;
        match kind.as_str() {
            "action" => index.actions.push(value),
            "object" => index.objects.push(value),
            "task_type" => index.task_types.push(value),
            "environment" => index.environments.push(value),
            "tool" => index.tools.push(value),
            "keyword" => index.keywords.push(value),
            _ => {
                return Err(CoreError::Internal(format!(
                    "unknown stored facet kind: {kind}"
                )));
            }
        }
    }
    Ok(index)
}

fn replace_facets(transaction: &Transaction<'_>, id: Uuid, index: &MemoryIndex) -> CoreResult<()> {
    transaction.execute(
        "DELETE FROM memory_facets WHERE memory_id = ?1",
        [id.to_string()],
    )?;
    for (kind, value) in facet_pairs(index) {
        transaction.execute(
            "INSERT INTO memory_facets(memory_id, facet_kind, value) VALUES (?1, ?2, ?3)",
            params![id.to_string(), kind, value],
        )?;
    }
    Ok(())
}

fn upsert_fts(
    transaction: &Transaction<'_>,
    id: Uuid,
    name: &str,
    body: &str,
    index: &MemoryIndex,
) -> CoreResult<()> {
    transaction.execute(
        "DELETE FROM memory_fts WHERE memory_id = ?1",
        [id.to_string()],
    )?;
    transaction.execute(
        "INSERT INTO memory_fts(memory_id, name, body, keywords, retrieval_text)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            id.to_string(),
            name,
            body,
            index.keywords.join(" "),
            index.retrieval_text
        ],
    )?;
    Ok(())
}

fn facet_pairs(index: &MemoryIndex) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    for (kind, values) in [
        ("action", &index.actions),
        ("object", &index.objects),
        ("task_type", &index.task_types),
        ("environment", &index.environments),
        ("tool", &index.tools),
        ("keyword", &index.keywords),
    ] {
        pairs.extend(values.iter().map(|value| (kind.to_owned(), value.clone())));
    }
    pairs
}

fn encode_vector(vector: &[f32]) -> Vec<u8> {
    vector
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

fn decode_vector(bytes: &[u8], dimensions: usize) -> CoreResult<Vec<f32>> {
    if bytes.len() != dimensions * size_of::<f32>() {
        return Err(CoreError::Internal(
            "stored embedding has invalid length".into(),
        ));
    }
    Ok(bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect())
}

fn parse_uuid(value: &str) -> CoreResult<Uuid> {
    Uuid::parse_str(value)
        .map_err(|error| CoreError::Internal(format!("invalid stored UUID: {error}")))
}

fn parse_time(value: &str) -> CoreResult<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|time| time.with_timezone(&Utc))
        .map_err(|error| CoreError::Internal(format!("invalid stored timestamp: {error}")))
}

fn canonical_selection(ids: &[Uuid]) -> CoreResult<String> {
    let mut ids = ids.to_vec();
    ids.sort_unstable();
    serde_json::to_string(&ids).map_err(|error| CoreError::Internal(error.to_string()))
}

fn conversion_error(error: impl std::error::Error + Send + Sync + 'static) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
}

fn map_unique_conflict(message: &'static str) -> impl FnOnce(rusqlite::Error) -> CoreError {
    move |error| match error {
        rusqlite::Error::SqliteFailure(ref failure, _)
            if failure.code == rusqlite::ErrorCode::ConstraintViolation =>
        {
            CoreError::Conflict(message.into())
        }
        other => CoreError::Storage(other),
    }
}

fn truncate_error(error: &str) -> String {
    error.chars().take(500).collect()
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;
    use crate::domain::MemoryContent;

    fn setup() -> (TempDir, Storage, IssuedActor) {
        let dir = TempDir::new().unwrap();
        let storage = Storage::new(dir.path().join("test.db"));
        storage.migrate().unwrap();
        let admin = storage.bootstrap_admin("owner").unwrap().unwrap();
        (dir, storage, admin)
    }

    fn sample_draft() -> MemoryDraft {
        MemoryDraft {
            name: "压缩包文档局部编辑".into(),
            content: MemoryContent::Procedure {
                steps: vec!["创建备份。".into(), "重新打包。".into()],
            },
            index: MemoryIndex {
                actions: vec!["edit".into()],
                objects: vec!["archive_document".into()],
                keywords: vec!["xmind".into()],
                retrieval_text: "编辑 XMind 压缩包文档".into(),
                ..MemoryIndex::default()
            },
        }
    }

    #[test]
    fn crud_and_chinese_search_work() {
        let (_dir, storage, admin) = setup();
        let memory = storage
            .create_memory(sample_draft(), &admin.actor, "model", 3, "hash")
            .unwrap();
        assert_eq!(storage.get_memory(memory.id).unwrap().name, memory.name);
        assert_eq!(storage.full_text_search("思维导图", 10).unwrap().len(), 0);
        assert_eq!(
            storage.full_text_search("压缩包", 10).unwrap()[0].0,
            memory.id
        );
        assert_eq!(
            storage.full_text_search("备份", 10).unwrap()[0].0,
            memory.id
        );
        storage.delete_memory(memory.id).unwrap();
        assert!(storage.get_memory(memory.id).is_err());
    }

    #[test]
    fn model_change_marks_ready_embeddings_for_rebuild() {
        let (_dir, storage, admin) = setup();
        let memory = storage
            .create_memory(sample_draft(), &admin.actor, "model-a", 3, "hash")
            .unwrap();
        storage
            .mark_embedding_ready(memory.id, "model-a", &[0.1, 0.2, 0.3], "hash")
            .unwrap();

        assert!(
            storage
                .memories_needing_embeddings("model-a", 3)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            storage
                .memories_needing_embeddings("model-b", 3)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            storage
                .memories_needing_embeddings("model-a", 4)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn agent_token_authenticates_and_can_be_revoked() {
        let (_dir, storage, _admin) = setup();
        let issued = storage
            .create_actor(NewActor {
                name: "codex".into(),
                kind: ActorKind::Agent,
                can_read: true,
                can_create: true,
                can_confirm_user_changes: false,
            })
            .unwrap();
        assert_eq!(storage.authenticate(&issued.token).unwrap().name, "codex");
        storage.revoke_actor(issued.actor.id).unwrap();
        assert!(matches!(
            storage.authenticate(&issued.token),
            Err(CoreError::Unauthorized)
        ));
    }

    #[test]
    fn lazy_cleanup_removes_expired_sessions_and_candidates() {
        let (_dir, storage, admin) = setup();
        let memory = storage
            .create_memory(sample_draft(), &admin.actor, "model", 3, "hash")
            .unwrap();
        let (old_match, _) = storage
            .create_match_session(&admin.actor, std::slice::from_ref(&memory))
            .unwrap();
        let (old_change, _) = storage
            .prepare_memory_change(
                &admin.actor,
                &MemoryChange::Create {
                    draft: sample_draft(),
                },
                &Value::Null,
            )
            .unwrap();
        let connection = storage.connect().unwrap();
        connection
            .execute(
                "UPDATE match_sessions SET expires_at = ?2 WHERE id = ?1",
                params![old_match.to_string(), "2000-01-01T00:00:00Z"],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE memory_changes SET expires_at = ?2 WHERE id = ?1",
                params![old_change.to_string(), "2000-01-01T00:00:00Z"],
            )
            .unwrap();
        drop(connection);

        storage
            .create_match_session(&admin.actor, &[memory])
            .unwrap();
        let connection = storage.connect().unwrap();
        let match_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM match_sessions WHERE id = ?1",
                [old_match.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        let candidate_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM match_candidates WHERE match_id = ?1",
                [old_match.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        let change_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM memory_changes WHERE id = ?1",
                [old_change.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!((match_count, candidate_count, change_count), (0, 0, 0));
    }

    #[test]
    fn failed_and_stale_commit_claims_can_be_retried() {
        let (_dir, storage, admin) = setup();
        let (change_id, _) = storage
            .prepare_memory_change(
                &admin.actor,
                &MemoryChange::Create {
                    draft: sample_draft(),
                },
                &Value::Null,
            )
            .unwrap();
        storage
            .begin_memory_change_commit(&admin.actor, change_id)
            .unwrap();
        assert!(matches!(
            storage.begin_memory_change_commit(&admin.actor, change_id),
            Err(CoreError::Conflict(_))
        ));
        storage.release_memory_change_commit(change_id).unwrap();
        storage
            .begin_memory_change_commit(&admin.actor, change_id)
            .unwrap();

        let connection = storage.connect().unwrap();
        connection
            .execute(
                "UPDATE memory_changes SET committed_at = ?2 WHERE id = ?1",
                params![change_id.to_string(), "2000-01-01T00:00:00Z"],
            )
            .unwrap();
        drop(connection);
        storage
            .begin_memory_change_commit(&admin.actor, change_id)
            .unwrap();
    }

    #[test]
    fn committed_mutation_is_retryable_before_embedding_finishes() {
        let (_dir, storage, admin) = setup();
        let memory = storage
            .create_memory(sample_draft(), &admin.actor, "model", 3, "hash")
            .unwrap();
        let mut updated = sample_draft();
        updated.name = "updated atomically".into();
        let (change_id, _) = storage
            .prepare_memory_change(
                &admin.actor,
                &MemoryChange::Update {
                    id: memory.id,
                    draft: updated.clone(),
                },
                &Value::Null,
            )
            .unwrap();
        storage
            .begin_memory_change_commit(&admin.actor, change_id)
            .unwrap();
        storage
            .update_memory_for_change(change_id, memory.id, updated, "model", 3, "new-hash")
            .unwrap();

        let retried = storage
            .begin_memory_change_commit(&admin.actor, change_id)
            .unwrap();
        let value = retried
            .committed_result
            .expect("mutation result must be durable with the update");
        let stored: Memory = serde_json::from_value(value).unwrap();
        assert_eq!(stored.name, "updated atomically");
        assert_eq!(stored.body_version, memory.body_version + 1);
    }
}
