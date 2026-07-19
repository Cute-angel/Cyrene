use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    domain::{Actor, Memory, MemoryChange, MemoryDraft, MemoryKind, MemoryStatus, NewActor},
    embedding::{EmbeddingProvider, SharedEmbeddingProvider, content_hash},
    error::{CoreError, CoreResult},
    protocol::{PROTOCOL_VERSION, RequestEnvelope, ResponseEnvelope},
    search::{
        ManualSearchRequest, MatchCandidatesRequest, RecallSource, SearchFilters, SearchHit,
        SearchResponse, fuse, semantic_ranking,
    },
    storage::{MatchRejection, MatchSelection, Storage},
};

const ROUTE_LIMIT: usize = 30;
const AUTO_CANDIDATE_LIMIT: usize = 6;

#[derive(Clone)]
pub struct Application {
    storage: Storage,
    embedding: Option<SharedEmbeddingProvider>,
    embedding_model: String,
    embedding_dimensions: usize,
}

impl Application {
    #[must_use]
    pub fn new(
        storage: Storage,
        embedding: Option<Arc<dyn EmbeddingProvider>>,
        embedding_model: String,
        embedding_dimensions: usize,
    ) -> Self {
        Self {
            storage,
            embedding,
            embedding_model,
            embedding_dimensions,
        }
    }

    #[must_use]
    pub fn storage(&self) -> &Storage {
        &self.storage
    }

    pub fn authenticate(&self, token: &str) -> CoreResult<Actor> {
        self.storage.authenticate(token)
    }

    pub async fn execute(&self, actor: &Actor, request: RequestEnvelope) -> ResponseEnvelope {
        let request_id = request.request_id.clone();
        let result = self.execute_inner(actor, request).await;
        match result {
            Ok(data) => ResponseEnvelope::success(request_id, data),
            Err(error) => ResponseEnvelope::failure(request_id, &error),
        }
    }

    async fn execute_inner(&self, actor: &Actor, request: RequestEnvelope) -> CoreResult<Value> {
        if request.protocol_version != PROTOCOL_VERSION {
            return Err(CoreError::UnsupportedProtocol(request.protocol_version));
        }
        match request.action.as_str() {
            "health" => Ok(json!({
                "status": "ok",
                "protocol_version": PROTOCOL_VERSION,
                "embedding_configured": self.embedding.is_some(),
            })),
            "core.shutdown" => {
                require_manage(actor)?;
                Ok(json!({ "status": "shutting_down" }))
            }
            "memory.create" => {
                require_create(actor)?;
                let draft = payload::<MemoryDraft>(request.payload)?;
                let memory = self.create_memory(actor, draft).await?;
                to_value(memory)
            }
            "memory.get" => {
                require_read(actor)?;
                let input = payload::<IdRequest>(request.payload)?;
                to_value(self.storage.get_memory(input.id)?)
            }
            "memory.list" => {
                require_read(actor)?;
                let input = payload::<ListMemoriesRequest>(request.payload)?;
                let limit = input.limit.clamp(1, 200);
                to_value(self.storage.list_memories(
                    input.status,
                    input.kind,
                    input.source_agent.as_deref(),
                    limit,
                    input.offset,
                )?)
            }
            "memory.update" => {
                require_manage(actor)?;
                let input = payload::<UpdateMemoryRequest>(request.payload)?;
                let memory = self.update_memory(input.id, input.draft).await?;
                to_value(memory)
            }
            "memory.set_status" => {
                require_manage(actor)?;
                let input = payload::<SetStatusRequest>(request.payload)?;
                to_value(self.storage.set_memory_status(input.id, input.status)?)
            }
            "memory.delete" => {
                require_manage(actor)?;
                let input = payload::<IdRequest>(request.payload)?;
                self.storage.delete_memory(input.id)?;
                Ok(json!({ "deleted": true, "id": input.id }))
            }
            "search.manual" => {
                require_read(actor)?;
                let input = payload::<ManualSearchRequest>(request.payload)?;
                to_value(self.manual_search(input).await?)
            }
            "match.candidates" => {
                require_read(actor)?;
                let input = payload::<MatchCandidatesRequest>(request.payload)?;
                to_value(self.match_candidates(actor, input).await?)
            }
            "match.select" => {
                require_read(actor)?;
                let input = payload::<MatchSelectRequest>(request.payload)?;
                to_value(self.match_select(actor, input)?)
            }
            "memory.change.prepare" => {
                require_confirm_user_changes(actor)?;
                let change = payload::<MemoryChange>(request.payload)?;
                to_value(self.prepare_memory_change(actor, change)?)
            }
            "memory.change.commit" => {
                require_confirm_user_changes(actor)?;
                let input = payload::<CommitMemoryChangeRequest>(request.payload)?;
                self.commit_memory_change(actor, input.change_id).await
            }
            "index.status" => {
                require_read(actor)?;
                to_value(self.storage.index_status()?)
            }
            "index.rebuild" => {
                require_manage(actor)?;
                to_value(self.rebuild_index().await?)
            }
            "actor.create" => {
                require_manage(actor)?;
                to_value(
                    self.storage
                        .create_actor(payload::<NewActor>(request.payload)?)?,
                )
            }
            "actor.list" => {
                require_manage(actor)?;
                to_value(self.storage.list_actors()?)
            }
            "actor.revoke" => {
                require_manage(actor)?;
                let input = payload::<IdRequest>(request.payload)?;
                to_value(self.storage.revoke_actor(input.id)?)
            }
            action => Err(CoreError::Validation(format!("unknown action: {action}"))),
        }
    }

    async fn create_memory(&self, actor: &Actor, mut draft: MemoryDraft) -> CoreResult<Memory> {
        draft.validate_and_normalize()?;
        let text = draft
            .index
            .embedding_text(&draft.name, &draft.content.plain_text());
        let hash = content_hash(&text);
        let memory = self.storage.create_memory(
            draft,
            actor,
            &self.embedding_model,
            self.embedding_dimensions,
            &hash,
        )?;
        self.index_memory(memory, &text, &hash).await
    }

    async fn update_memory(&self, id: Uuid, mut draft: MemoryDraft) -> CoreResult<Memory> {
        draft.validate_and_normalize()?;
        let text = draft
            .index
            .embedding_text(&draft.name, &draft.content.plain_text());
        let hash = content_hash(&text);
        let memory = self.storage.update_memory(
            id,
            draft,
            &self.embedding_model,
            self.embedding_dimensions,
            &hash,
        )?;
        self.index_memory(memory, &text, &hash).await
    }

    async fn index_memory(&self, memory: Memory, text: &str, hash: &str) -> CoreResult<Memory> {
        let Some(provider) = &self.embedding else {
            self.storage
                .mark_embedding_pending(memory.id, "embedding provider is not configured")?;
            return self.storage.get_memory(memory.id);
        };
        match provider.embed(&[text.to_owned()]).await {
            Ok(vectors) => {
                let vector = vectors.into_iter().next().ok_or_else(|| {
                    CoreError::Embedding("embedding provider returned no vector".into())
                })?;
                if vector.len() != self.embedding_dimensions {
                    self.storage.mark_embedding_pending(
                        memory.id,
                        "embedding provider returned an unexpected vector length",
                    )?;
                } else {
                    self.storage.mark_embedding_ready(
                        memory.id,
                        &self.embedding_model,
                        &vector,
                        hash,
                    )?;
                }
            }
            Err(error) => {
                self.storage
                    .mark_embedding_pending(memory.id, &error.to_string())?;
            }
        }
        self.storage.get_memory(memory.id)
    }

    async fn manual_search(&self, input: ManualSearchRequest) -> CoreResult<SearchResponse> {
        let limit = input.limit.clamp(1, 200);
        let description_text = input.description.search_text();
        let search_text = [input.query.trim(), description_text.trim()]
            .into_iter()
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        self.search(
            &search_text,
            &input.description.facets(),
            &input.filters,
            limit,
            input.offset,
        )
        .await
    }

    async fn match_candidates(
        &self,
        actor: &Actor,
        input: MatchCandidatesRequest,
    ) -> CoreResult<MatchCandidatesResponse> {
        let text = input.description.search_text();
        let response = self
            .search(
                &text,
                &input.description.facets(),
                &SearchFilters::default(),
                AUTO_CANDIDATE_LIMIT,
                0,
            )
            .await?;
        let candidates = response
            .hits
            .iter()
            .map(|hit| hit.memory.clone())
            .collect::<Vec<_>>();
        let (match_id, expires_at) = self.storage.create_match_session(actor, &candidates)?;
        Ok(MatchCandidatesResponse {
            match_id,
            expires_at,
            hits: response.hits,
            degraded: response.degraded,
            degradation_reason: response.degradation_reason,
        })
    }

    fn match_select(
        &self,
        actor: &Actor,
        input: MatchSelectRequest,
    ) -> CoreResult<MatchSelectResponse> {
        let (status, accepted_ids, rejected, retryable) =
            match self
                .storage
                .select_match(actor, input.match_id, &input.selected_ids)?
            {
                MatchSelection::Accepted(ids) => ("accepted", ids, Vec::new(), false),
                MatchSelection::Rejected { reasons, retryable } => {
                    ("rejected", Vec::new(), reasons, retryable)
                }
            };
        Ok(MatchSelectResponse {
            match_id: input.match_id,
            status,
            accepted_ids,
            rejected,
            retryable,
        })
    }

    fn prepare_memory_change(
        &self,
        actor: &Actor,
        mut change: MemoryChange,
    ) -> CoreResult<PrepareMemoryChangeResponse> {
        match &mut change {
            MemoryChange::Create { draft } | MemoryChange::Update { draft, .. } => {
                draft.validate_and_normalize()?;
            }
            MemoryChange::SetStatus { .. } | MemoryChange::Delete { .. } => {}
        }
        let preview = match &change {
            MemoryChange::Create { draft } => json!({
                "operation": "create",
                "after": draft,
            }),
            MemoryChange::Update { id, draft } => json!({
                "operation": "update",
                "id": id,
                "before": self.storage.get_memory(*id)?,
                "after": draft,
            }),
            MemoryChange::SetStatus { id, status } => json!({
                "operation": "set_status",
                "id": id,
                "before": self.storage.get_memory(*id)?,
                "status": status,
            }),
            MemoryChange::Delete { id } => json!({
                "operation": "delete",
                "id": id,
                "before": self.storage.get_memory(*id)?,
            }),
        };
        let (change_id, expires_at) = self
            .storage
            .prepare_memory_change(actor, &change, &preview)?;
        Ok(PrepareMemoryChangeResponse {
            change_id,
            expires_at,
            preview,
        })
    }

    async fn commit_memory_change(&self, actor: &Actor, change_id: Uuid) -> CoreResult<Value> {
        let prepared = self.storage.begin_memory_change_commit(actor, change_id)?;
        if let Some(value) = prepared.committed_result {
            return Ok(value);
        }
        let mut durable_result = false;
        let result: CoreResult<Value> = async {
            match prepared.change {
                MemoryChange::Create { draft } => {
                    let text = draft
                        .index
                        .embedding_text(&draft.name, &draft.content.plain_text());
                    let hash = content_hash(&text);
                    let memory = match self.storage.get_memory(change_id) {
                        Ok(memory) => memory,
                        Err(CoreError::NotFound(_)) => self.storage.create_user_memory_for_change(
                            change_id,
                            draft,
                            &self.embedding_model,
                            self.embedding_dimensions,
                            &hash,
                        )?,
                        Err(error) => return Err(error),
                    };
                    self.storage
                        .finish_memory_change(change_id, &to_value(&memory)?)?;
                    durable_result = true;
                    let value = to_value(self.index_memory(memory, &text, &hash).await?)?;
                    self.storage.finish_memory_change(change_id, &value)?;
                    Ok(value)
                }
                MemoryChange::Update { id, draft } => {
                    let text = draft
                        .index
                        .embedding_text(&draft.name, &draft.content.plain_text());
                    let hash = content_hash(&text);
                    let memory = self.storage.update_memory_for_change(
                        change_id,
                        id,
                        draft,
                        &self.embedding_model,
                        self.embedding_dimensions,
                        &hash,
                    )?;
                    durable_result = true;
                    let value = to_value(self.index_memory(memory, &text, &hash).await?)?;
                    self.storage.finish_memory_change(change_id, &value)?;
                    Ok(value)
                }
                MemoryChange::SetStatus { id, status } => {
                    let value = to_value(
                        self.storage
                            .set_memory_status_for_change(change_id, id, status)?,
                    )?;
                    durable_result = true;
                    Ok(value)
                }
                MemoryChange::Delete { id } => {
                    self.storage.delete_memory_for_change(change_id, id)?;
                    let value = json!({ "deleted": true, "id": id });
                    durable_result = true;
                    Ok(value)
                }
            }
        }
        .await;
        if result.is_err() && !durable_result {
            self.storage.release_memory_change_commit(change_id)?;
        }
        result
    }

    async fn search(
        &self,
        text: &str,
        facets: &[(String, String)],
        filters: &SearchFilters,
        limit: usize,
        offset: usize,
    ) -> CoreResult<SearchResponse> {
        if text.trim().is_empty() && facets.is_empty() {
            return Err(CoreError::Validation(
                "search requires text or structured facets".into(),
            ));
        }
        let structured = self.storage.structured_search(facets, ROUTE_LIMIT)?;
        let full_text = self.storage.full_text_search(text, ROUTE_LIMIT)?;
        let mut routes = vec![
            (RecallSource::Structured, 2.0, structured),
            (RecallSource::FullText, 1.2, full_text),
        ];
        let mut degraded = false;
        let mut reason = None;
        if let Some(provider) = &self.embedding {
            match provider.embed(&[text.to_owned()]).await {
                Ok(mut vectors) => {
                    if let Some(query_vector) = vectors.pop() {
                        let candidates = self
                            .storage
                            .ready_embeddings(&self.embedding_model, self.embedding_dimensions)?
                            .into_iter()
                            .map(|item| (item.memory_id, item.vector));
                        routes.push((
                            RecallSource::Semantic,
                            1.0,
                            semantic_ranking(&query_vector, candidates),
                        ));
                    }
                }
                Err(error) => {
                    degraded = true;
                    reason = Some(error.to_string());
                }
            }
        } else {
            degraded = true;
            reason = Some("embedding provider is not configured".into());
        }
        Ok(SearchResponse {
            hits: fuse(&self.storage, &routes, filters, limit, offset)?,
            degraded,
            degradation_reason: reason,
        })
    }

    async fn rebuild_index(&self) -> CoreResult<RebuildReport> {
        let memories = self
            .storage
            .memories_needing_embeddings(&self.embedding_model, self.embedding_dimensions)?;
        let total = memories.len();
        let Some(provider) = &self.embedding else {
            return Ok(RebuildReport {
                total,
                indexed: 0,
                pending: total,
                error: Some("embedding provider is not configured".into()),
            });
        };
        let mut indexed = 0;
        let mut last_error = None;
        for chunk in memories.chunks(32) {
            let inputs = chunk.iter().map(Memory::embedding_text).collect::<Vec<_>>();
            match provider.embed(&inputs).await {
                Ok(vectors) if vectors.len() == chunk.len() => {
                    for (memory, vector) in chunk.iter().zip(vectors) {
                        let hash = content_hash(&memory.embedding_text());
                        self.storage.mark_embedding_ready(
                            memory.id,
                            &self.embedding_model,
                            &vector,
                            &hash,
                        )?;
                        indexed += 1;
                    }
                }
                Ok(_) => {
                    last_error =
                        Some("embedding provider returned an unexpected batch size".into());
                }
                Err(error) => {
                    last_error = Some(error.to_string());
                }
            }
        }
        Ok(RebuildReport {
            total,
            indexed,
            pending: total - indexed,
            error: last_error,
        })
    }
}

fn payload<T: DeserializeOwned>(value: Value) -> CoreResult<T> {
    serde_json::from_value(value)
        .map_err(|error| CoreError::Validation(format!("invalid action payload: {error}")))
}

fn to_value(value: impl Serialize) -> CoreResult<Value> {
    serde_json::to_value(value).map_err(|error| CoreError::Internal(error.to_string()))
}

fn require_read(actor: &Actor) -> CoreResult<()> {
    if actor.can_read && !actor.revoked {
        Ok(())
    } else {
        Err(CoreError::Forbidden)
    }
}

fn require_create(actor: &Actor) -> CoreResult<()> {
    if actor.can_create && !actor.revoked {
        Ok(())
    } else {
        Err(CoreError::Forbidden)
    }
}

fn require_manage(actor: &Actor) -> CoreResult<()> {
    if actor.can_manage && !actor.revoked {
        Ok(())
    } else {
        Err(CoreError::Forbidden)
    }
}

fn require_confirm_user_changes(actor: &Actor) -> CoreResult<()> {
    if (actor.can_confirm_user_changes || actor.can_manage) && !actor.revoked {
        Ok(())
    } else {
        Err(CoreError::Forbidden)
    }
}

#[derive(Deserialize)]
struct IdRequest {
    id: Uuid,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct ListMemoriesRequest {
    status: Option<MemoryStatus>,
    kind: Option<MemoryKind>,
    source_agent: Option<String>,
    #[serde(default = "default_list_limit")]
    limit: usize,
    offset: usize,
}

const fn default_list_limit() -> usize {
    50
}

#[derive(Deserialize)]
struct UpdateMemoryRequest {
    id: Uuid,
    draft: MemoryDraft,
}

#[derive(Deserialize)]
struct SetStatusRequest {
    id: Uuid,
    status: MemoryStatus,
}

#[derive(Deserialize)]
struct MatchSelectRequest {
    match_id: Uuid,
    selected_ids: Vec<Uuid>,
}

#[derive(Serialize)]
struct MatchCandidatesResponse {
    match_id: Uuid,
    expires_at: DateTime<Utc>,
    hits: Vec<SearchHit>,
    degraded: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    degradation_reason: Option<String>,
}

#[derive(Serialize)]
struct MatchSelectResponse {
    match_id: Uuid,
    status: &'static str,
    accepted_ids: Vec<Uuid>,
    rejected: Vec<MatchRejection>,
    retryable: bool,
}

#[derive(Serialize)]
struct PrepareMemoryChangeResponse {
    change_id: Uuid,
    expires_at: DateTime<Utc>,
    preview: Value,
}

#[derive(Deserialize)]
struct CommitMemoryChangeRequest {
    change_id: Uuid,
}

#[derive(Serialize)]
struct RebuildReport {
    total: usize,
    indexed: usize,
    pending: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;
    use crate::{
        domain::{ActorKind, MemoryContent, MemoryIndex, NewActor},
        embedding::tests::FakeEmbeddingProvider,
    };

    fn setup(provider: FakeEmbeddingProvider) -> (TempDir, Application, Actor) {
        let dir = TempDir::new().unwrap();
        let storage = Storage::new(dir.path().join("app.db"));
        storage.migrate().unwrap();
        let admin = storage.bootstrap_admin("owner").unwrap().unwrap().actor;
        let app = Application::new(storage, Some(Arc::new(provider)), "fake".into(), 4);
        (dir, app, admin)
    }

    #[tokio::test]
    async fn failed_embedding_keeps_memory_pending() {
        let (_dir, app, admin) = setup(FakeEmbeddingProvider {
            fail: true,
            dimensions: 4,
        });
        let memory = app
            .create_memory(
                &admin,
                MemoryDraft {
                    name: "minimal edits".into(),
                    content: MemoryContent::Rule {
                        text: "Only edit requested behavior.".into(),
                    },
                    index: MemoryIndex::default(),
                },
            )
            .await
            .unwrap();
        assert_eq!(
            memory.embedding_status,
            crate::domain::EmbeddingStatus::Pending
        );
    }

    #[tokio::test]
    async fn agent_cannot_update_existing_memory() {
        let (_dir, app, admin) = setup(FakeEmbeddingProvider {
            fail: false,
            dimensions: 4,
        });
        let issued = app
            .storage
            .create_actor(NewActor {
                name: "codex".into(),
                kind: ActorKind::Agent,
                can_read: true,
                can_create: true,
                can_confirm_user_changes: false,
            })
            .unwrap();
        let response = app
            .execute(
                &issued.actor,
                RequestEnvelope {
                    request_id: "1".into(),
                    protocol_version: PROTOCOL_VERSION.into(),
                    action: "memory.update".into(),
                    payload: json!({
                        "id": Uuid::new_v4(),
                        "draft": {
                            "name": "x",
                            "content": {"type": "rule", "text": "y"}
                        }
                    }),
                },
            )
            .await;
        assert!(!response.ok);
        assert_eq!(response.error.unwrap().code, "forbidden");
        assert!(admin.can_manage);
    }

    #[tokio::test]
    async fn candidate_matching_fuses_available_routes() {
        let (_dir, app, admin) = setup(FakeEmbeddingProvider {
            fail: false,
            dimensions: 4,
        });
        app.create_memory(
            &admin,
            MemoryDraft {
                name: "XMind package edits".into(),
                content: MemoryContent::Procedure {
                    steps: vec!["Create a backup".into(), "Repack the archive".into()],
                },
                index: MemoryIndex {
                    actions: vec!["edit".into()],
                    objects: vec!["archive_document".into()],
                    keywords: vec!["xmind".into()],
                    retrieval_text: "edit xmind archive document".into(),
                    ..MemoryIndex::default()
                },
            },
        )
        .await
        .unwrap();

        let result = app
            .match_candidates(
                &admin,
                MatchCandidatesRequest {
                    description: crate::search::QueryDescription {
                        action: Some("edit".into()),
                        object: Some("archive_document".into()),
                        keywords: vec!["xmind".into()],
                        ..crate::search::QueryDescription::default()
                    },
                },
            )
            .await
            .unwrap();
        assert!(!result.degraded);
        assert_eq!(result.hits.len(), 1);
        assert!(result.hits[0].sources.contains(&RecallSource::Structured));
        assert!(result.hits[0].sources.contains(&RecallSource::FullText));
        assert!(result.hits[0].sources.contains(&RecallSource::Semantic));
    }

    #[tokio::test]
    async fn match_selection_is_atomic_consumed_and_idempotent() {
        let (_dir, app, admin) = setup(FakeEmbeddingProvider {
            fail: false,
            dimensions: 4,
        });
        let memory = app
            .create_memory(
                &admin,
                MemoryDraft {
                    name: "small safe edits".into(),
                    content: MemoryContent::Rule {
                        text: "Keep unrelated content unchanged.".into(),
                    },
                    index: MemoryIndex {
                        actions: vec!["edit".into()],
                        retrieval_text: "edit safely".into(),
                        ..MemoryIndex::default()
                    },
                },
            )
            .await
            .unwrap();
        let candidates = app
            .match_candidates(
                &admin,
                MatchCandidatesRequest {
                    description: crate::search::QueryDescription {
                        action: Some("edit".into()),
                        ..crate::search::QueryDescription::default()
                    },
                },
            )
            .await
            .unwrap();
        let accepted = app
            .match_select(
                &admin,
                MatchSelectRequest {
                    match_id: candidates.match_id,
                    selected_ids: vec![memory.id],
                },
            )
            .unwrap();
        assert_eq!(accepted.status, "accepted");
        assert_eq!(accepted.accepted_ids, [memory.id]);

        let repeated = app
            .match_select(
                &admin,
                MatchSelectRequest {
                    match_id: candidates.match_id,
                    selected_ids: vec![memory.id],
                },
            )
            .unwrap();
        assert_eq!(repeated.status, "accepted");

        let changed = app
            .match_select(
                &admin,
                MatchSelectRequest {
                    match_id: candidates.match_id,
                    selected_ids: Vec::new(),
                },
            )
            .unwrap();
        assert_eq!(changed.status, "rejected");
        assert!(changed.accepted_ids.is_empty());
        assert!(!changed.retryable);
    }

    #[tokio::test]
    async fn stale_candidate_is_rejected_without_content() {
        let (_dir, app, admin) = setup(FakeEmbeddingProvider {
            fail: false,
            dimensions: 4,
        });
        let draft = MemoryDraft {
            name: "versioned".into(),
            content: MemoryContent::Rule {
                text: "first".into(),
            },
            index: MemoryIndex {
                actions: vec!["edit".into()],
                ..MemoryIndex::default()
            },
        };
        let memory = app.create_memory(&admin, draft.clone()).await.unwrap();
        let candidates = app
            .match_candidates(
                &admin,
                MatchCandidatesRequest {
                    description: crate::search::QueryDescription {
                        action: Some("edit".into()),
                        ..crate::search::QueryDescription::default()
                    },
                },
            )
            .await
            .unwrap();
        app.update_memory(
            memory.id,
            MemoryDraft {
                name: "versioned".into(),
                content: MemoryContent::Rule {
                    text: "second".into(),
                },
                ..draft
            },
        )
        .await
        .unwrap();
        let outcome = app
            .match_select(
                &admin,
                MatchSelectRequest {
                    match_id: candidates.match_id,
                    selected_ids: vec![memory.id],
                },
            )
            .unwrap();
        let encoded = serde_json::to_string(&outcome).unwrap();
        assert_eq!(outcome.status, "rejected");
        assert!(outcome.accepted_ids.is_empty());
        assert!(encoded.contains("body_version_changed"));
        assert!(!encoded.contains("second"));
    }

    #[tokio::test]
    async fn confirmed_create_is_user_sourced_and_commit_is_idempotent() {
        let (_dir, app, _admin) = setup(FakeEmbeddingProvider {
            fail: false,
            dimensions: 4,
        });
        let adapter = app
            .storage
            .create_actor(NewActor {
                name: "adapter".into(),
                kind: ActorKind::Agent,
                can_read: true,
                can_create: false,
                can_confirm_user_changes: true,
            })
            .unwrap()
            .actor;
        let prepared = app
            .prepare_memory_change(
                &adapter,
                MemoryChange::Create {
                    draft: MemoryDraft {
                        name: "Remembered by user".into(),
                        content: MemoryContent::Rule {
                            text: "Ask before changing memory.".into(),
                        },
                        index: MemoryIndex::default(),
                    },
                },
            )
            .unwrap();
        let first = app
            .commit_memory_change(&adapter, prepared.change_id)
            .await
            .unwrap();
        let second = app
            .commit_memory_change(&adapter, prepared.change_id)
            .await
            .unwrap();
        assert_eq!(first, second);
        let memory: Memory = serde_json::from_value(first).unwrap();
        assert_eq!(memory.source_type, crate::domain::SourceType::User);
        assert!(memory.source_agent.is_none());
    }

    #[tokio::test]
    async fn confirmed_change_revalidates_the_prepared_memory() {
        let (_dir, app, admin) = setup(FakeEmbeddingProvider {
            fail: false,
            dimensions: 4,
        });
        let memory = app
            .create_memory(
                &admin,
                MemoryDraft {
                    name: "status race".into(),
                    content: MemoryContent::Rule {
                        text: "original".into(),
                    },
                    index: MemoryIndex::default(),
                },
            )
            .await
            .unwrap();
        let prepared = app
            .prepare_memory_change(&admin, MemoryChange::Delete { id: memory.id })
            .unwrap();
        app.storage
            .set_memory_status(memory.id, MemoryStatus::Disabled)
            .unwrap();
        assert!(matches!(
            app.commit_memory_change(&admin, prepared.change_id).await,
            Err(CoreError::Conflict(_))
        ));
        assert!(app.storage.get_memory(memory.id).is_ok());
    }

    #[tokio::test]
    async fn only_administrators_can_request_core_shutdown() {
        let (_dir, app, admin) = setup(FakeEmbeddingProvider {
            fail: false,
            dimensions: 4,
        });
        let issued = app
            .storage
            .create_actor(NewActor {
                name: "adapter".into(),
                kind: ActorKind::Agent,
                can_read: true,
                can_create: false,
                can_confirm_user_changes: true,
            })
            .unwrap();
        let request = |id: &str| RequestEnvelope {
            request_id: id.into(),
            protocol_version: PROTOCOL_VERSION.into(),
            action: "core.shutdown".into(),
            payload: json!({}),
        };
        assert!(app.execute(&admin, request("admin")).await.ok);
        let denied = app.execute(&issued.actor, request("adapter")).await;
        assert!(!denied.ok);
        assert_eq!(denied.error.unwrap().code, "forbidden");
    }
}
