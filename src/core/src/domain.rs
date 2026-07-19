use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{CoreError, CoreResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    Rule,
    Procedure,
}

impl MemoryKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Rule => "rule",
            Self::Procedure => "procedure",
        }
    }
}

impl TryFrom<&str> for MemoryKind {
    type Error = CoreError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "rule" => Ok(Self::Rule),
            "procedure" => Ok(Self::Procedure),
            _ => Err(CoreError::Validation(format!(
                "unknown memory kind: {value}"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MemoryContent {
    Rule { text: String },
    Procedure { steps: Vec<String> },
}

impl MemoryContent {
    pub fn validate(&self) -> CoreResult<()> {
        match self {
            Self::Rule { text } if text.trim().is_empty() => {
                Err(CoreError::Validation("rule text cannot be empty".into()))
            }
            Self::Rule { .. } => Ok(()),
            Self::Procedure { steps } if steps.is_empty() => Err(CoreError::Validation(
                "procedure must contain at least one step".into(),
            )),
            Self::Procedure { steps } if steps.iter().any(|step| step.trim().is_empty()) => Err(
                CoreError::Validation("procedure steps cannot be empty".into()),
            ),
            Self::Procedure { .. } => Ok(()),
        }
    }

    #[must_use]
    pub const fn kind(&self) -> MemoryKind {
        match self {
            Self::Rule { .. } => MemoryKind::Rule,
            Self::Procedure { .. } => MemoryKind::Procedure,
        }
    }

    #[must_use]
    pub fn plain_text(&self) -> String {
        match self {
            Self::Rule { text } => text.trim().to_owned(),
            Self::Procedure { steps } => steps
                .iter()
                .enumerate()
                .map(|(index, step)| format!("{}. {}", index + 1, step.trim()))
                .collect::<Vec<_>>()
                .join("\n"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryStatus {
    Enabled,
    Disabled,
    Archived,
}

impl MemoryStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Enabled => "enabled",
            Self::Disabled => "disabled",
            Self::Archived => "archived",
        }
    }
}

impl TryFrom<&str> for MemoryStatus {
    type Error = CoreError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "enabled" => Ok(Self::Enabled),
            "disabled" => Ok(Self::Disabled),
            "archived" => Ok(Self::Archived),
            _ => Err(CoreError::Validation(format!(
                "unknown memory status: {value}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceType {
    User,
    Agent,
}

impl SourceType {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Agent => "agent",
        }
    }
}

impl TryFrom<&str> for SourceType {
    type Error = CoreError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "user" => Ok(Self::User),
            "agent" => Ok(Self::Agent),
            _ => Err(CoreError::Validation(format!(
                "unknown source type: {value}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MemoryIndex {
    pub actions: Vec<String>,
    pub objects: Vec<String>,
    pub task_types: Vec<String>,
    pub environments: Vec<String>,
    pub tools: Vec<String>,
    pub keywords: Vec<String>,
    pub retrieval_text: String,
}

impl MemoryIndex {
    pub fn validate_and_normalize(&mut self) -> CoreResult<()> {
        const ACTIONS: &[&str] = &[
            "inspect", "create", "edit", "debug", "refactor", "test", "build", "run", "generate",
            "convert", "migrate", "review", "deploy", "delete",
        ];
        const OBJECTS: &[&str] = &[
            "code",
            "config",
            "dependency",
            "database",
            "document",
            "archive_document",
            "spreadsheet",
            "presentation",
            "image",
            "ui",
            "git_history",
            "environment",
        ];
        const TASK_TYPES: &[&str] = &[
            "analysis",
            "code_change",
            "diagnosis",
            "document_editing",
            "generation",
            "testing",
            "migration",
            "review",
            "deployment",
        ];
        normalize_controlled(&mut self.actions, ACTIONS, "action")?;
        normalize_controlled(&mut self.objects, OBJECTS, "object")?;
        normalize_controlled(&mut self.task_types, TASK_TYPES, "task_type")?;
        normalize_open(&mut self.environments);
        normalize_open(&mut self.tools);
        normalize_open(&mut self.keywords);
        self.retrieval_text = self.retrieval_text.trim().to_owned();
        Ok(())
    }

    #[must_use]
    pub fn embedding_text(&self, name: &str, body: &str) -> String {
        [name.trim(), body.trim(), self.retrieval_text.trim()]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn normalize_controlled(values: &mut Vec<String>, allowed: &[&str], field: &str) -> CoreResult<()> {
    normalize_open(values);
    if let Some(value) = values
        .iter()
        .find(|value| !allowed.contains(&value.as_str()))
    {
        return Err(CoreError::Validation(format!(
            "unsupported {field} classification: {value}"
        )));
    }
    Ok(())
}

fn normalize_open(values: &mut Vec<String>) {
    let normalized = values
        .drain(..)
        .map(|value| value.trim().to_lowercase())
        .filter(|value| !value.is_empty())
        .collect::<BTreeSet<_>>();
    values.extend(normalized);
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryDraft {
    pub name: String,
    pub content: MemoryContent,
    #[serde(default)]
    pub index: MemoryIndex,
}

impl MemoryDraft {
    pub fn validate_and_normalize(&mut self) -> CoreResult<()> {
        self.name = self.name.trim().to_lowercase().to_owned();
        if self.name.is_empty() {
            return Err(CoreError::Validation("memory name cannot be empty".into()));
        }
        if self.name.chars().count() > 120 {
            return Err(CoreError::Validation(
                "memory name cannot exceed 120 characters".into(),
            ));
        }
        self.content.validate()?;
        self.index.validate_and_normalize()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Memory {
    pub id: Uuid,
    pub name: String,
    pub content: MemoryContent,
    pub status: MemoryStatus,
    pub source_type: SourceType,
    pub source_agent: Option<String>,
    pub index: MemoryIndex,
    pub body_version: u32,
    pub embedding_status: EmbeddingStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Memory {
    #[must_use]
    pub fn embedding_text(&self) -> String {
        self.index
            .embedding_text(&self.name, &self.content.plain_text())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmbeddingStatus {
    Pending,
    Ready,
    Failed,
}

impl EmbeddingStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Ready => "ready",
            Self::Failed => "failed",
        }
    }
}

impl TryFrom<&str> for EmbeddingStatus {
    type Error = CoreError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "pending" => Ok(Self::Pending),
            "ready" => Ok(Self::Ready),
            "failed" => Ok(Self::Failed),
            _ => Err(CoreError::Validation(format!(
                "unknown embedding status: {value}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActorKind {
    User,
    Agent,
}

impl ActorKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Agent => "agent",
        }
    }
}

impl TryFrom<&str> for ActorKind {
    type Error = CoreError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "user" => Ok(Self::User),
            "agent" => Ok(Self::Agent),
            _ => Err(CoreError::Validation(format!(
                "unknown actor kind: {value}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Actor {
    pub id: Uuid,
    pub name: String,
    pub kind: ActorKind,
    pub can_read: bool,
    pub can_create: bool,
    pub can_confirm_user_changes: bool,
    pub can_manage: bool,
    pub revoked: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewActor {
    pub name: String,
    pub kind: ActorKind,
    #[serde(default = "default_true")]
    pub can_read: bool,
    #[serde(default)]
    pub can_create: bool,
    #[serde(default)]
    pub can_confirm_user_changes: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum MemoryChange {
    Create { draft: MemoryDraft },
    Update { id: Uuid, draft: MemoryDraft },
    SetStatus { id: Uuid, status: MemoryStatus },
    Delete { id: Uuid },
}

const fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssuedActor {
    pub actor: Actor,
    pub token: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_and_deduplicates_index_values() {
        let mut index = MemoryIndex {
            actions: vec![" Edit ".into(), "edit".into()],
            keywords: vec![" XMind ".into(), "xmind".into()],
            ..MemoryIndex::default()
        };
        index.validate_and_normalize().unwrap();
        assert_eq!(index.actions, ["edit"]);
        assert_eq!(index.keywords, ["xmind"]);
    }

    #[test]
    fn rejects_unknown_controlled_values() {
        let mut index = MemoryIndex {
            actions: vec!["launch_missiles".into()],
            ..MemoryIndex::default()
        };
        assert!(index.validate_and_normalize().is_err());
    }
}
