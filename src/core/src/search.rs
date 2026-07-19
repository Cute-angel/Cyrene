use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    domain::{Memory, MemoryKind, MemoryStatus},
    error::CoreResult,
    storage::Storage,
};

const ROUTE_LIMIT: usize = 30;
const RRF_K: f64 = 60.0;

pub type RankedCandidate = (Uuid, f64);
pub type RecallRoute = (RecallSource, f64, Vec<RankedCandidate>);

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct QueryDescription {
    pub stage: Option<String>,
    pub action: Option<String>,
    pub object: Option<String>,
    pub task_type: Option<String>,
    pub environment: Vec<String>,
    pub tools: Vec<String>,
    pub keywords: Vec<String>,
    pub explicit_constraints: Vec<String>,
}

impl QueryDescription {
    #[must_use]
    pub fn search_text(&self) -> String {
        self.stage
            .iter()
            .chain(self.action.iter())
            .chain(self.object.iter())
            .chain(self.task_type.iter())
            .chain(self.environment.iter())
            .chain(self.tools.iter())
            .chain(self.keywords.iter())
            .chain(self.explicit_constraints.iter())
            .map(String::as_str)
            .filter(|value| !value.trim().is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[must_use]
    pub fn facets(&self) -> Vec<(String, String)> {
        let mut facets = Vec::new();
        for (kind, value) in [
            ("action", self.action.as_ref()),
            ("object", self.object.as_ref()),
            ("task_type", self.task_type.as_ref()),
        ] {
            if let Some(value) = value {
                facets.push((kind.to_owned(), value.trim().to_lowercase()));
            }
        }
        facets.extend(
            self.environment
                .iter()
                .map(|value| ("environment".into(), value.trim().to_lowercase())),
        );
        facets.extend(
            self.tools
                .iter()
                .map(|value| ("tool".into(), value.trim().to_lowercase())),
        );
        facets.retain(|(_, value)| !value.is_empty());
        facets
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchFilters {
    pub status: Option<MemoryStatus>,
    pub kind: Option<MemoryKind>,
    pub source_agent: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManualSearchRequest {
    pub query: String,
    #[serde(default)]
    pub description: QueryDescription,
    #[serde(default)]
    pub filters: SearchFilters,
    #[serde(default = "default_manual_limit")]
    pub limit: usize,
    #[serde(default)]
    pub offset: usize,
}

const fn default_manual_limit() -> usize {
    50
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchCandidatesRequest {
    pub description: QueryDescription,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchHit {
    pub memory: Memory,
    pub score: f64,
    pub sources: Vec<RecallSource>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecallSource {
    Structured,
    FullText,
    Semantic,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResponse {
    pub hits: Vec<SearchHit>,
    pub degraded: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub degradation_reason: Option<String>,
}

pub fn fuse(
    storage: &Storage,
    routes: &[RecallRoute],
    filters: &SearchFilters,
    limit: usize,
    offset: usize,
) -> CoreResult<Vec<SearchHit>> {
    let mut scores = HashMap::<Uuid, f64>::new();
    let mut sources = HashMap::<Uuid, HashSet<RecallSource>>::new();
    for (source, weight, results) in routes {
        for (rank, (id, _raw_score)) in results.iter().take(ROUTE_LIMIT).enumerate() {
            *scores.entry(*id).or_default() += weight / (RRF_K + rank as f64 + 1.0);
            sources.entry(*id).or_default().insert(*source);
        }
    }
    let mut ranked = scores.into_iter().collect::<Vec<_>>();
    ranked.sort_by(|left, right| {
        right
            .1
            .total_cmp(&left.1)
            .then_with(|| left.0.cmp(&right.0))
    });
    let mut hits = Vec::new();
    for (id, score) in ranked {
        let memory = storage.get_memory(id)?;
        if filters.status.is_some_and(|status| status != memory.status)
            || filters
                .kind
                .is_some_and(|kind| kind != memory.content.kind())
            || filters
                .source_agent
                .as_deref()
                .is_some_and(|agent| memory.source_agent.as_deref() != Some(agent))
        {
            continue;
        }
        let mut hit_sources = sources
            .remove(&id)
            .unwrap_or_default()
            .into_iter()
            .collect::<Vec<_>>();
        hit_sources.sort_by_key(|source| match source {
            RecallSource::Structured => 0,
            RecallSource::FullText => 1,
            RecallSource::Semantic => 2,
        });
        hits.push(SearchHit {
            memory,
            score,
            sources: hit_sources,
        });
    }
    Ok(hits.into_iter().skip(offset).take(limit).collect())
}

#[must_use]
pub fn semantic_ranking(
    query: &[f32],
    candidates: impl IntoIterator<Item = (Uuid, Vec<f32>)>,
) -> Vec<(Uuid, f64)> {
    let mut ranked = candidates
        .into_iter()
        .filter_map(|(id, vector)| cosine_similarity(query, &vector).map(|score| (id, score)))
        .collect::<Vec<_>>();
    ranked.sort_by(|left, right| right.1.total_cmp(&left.1));
    ranked.truncate(ROUTE_LIMIT);
    ranked
}

fn cosine_similarity(left: &[f32], right: &[f32]) -> Option<f64> {
    if left.len() != right.len() || left.is_empty() {
        return None;
    }
    let mut dot = 0.0_f64;
    let mut left_norm = 0.0_f64;
    let mut right_norm = 0.0_f64;
    for (left, right) in left.iter().zip(right) {
        let left = f64::from(*left);
        let right = f64::from(*right);
        dot += left * right;
        left_norm += left * left;
        right_norm += right * right;
    }
    let denominator = left_norm.sqrt() * right_norm.sqrt();
    (denominator > f64::EPSILON).then_some(dot / denominator)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semantic_ranking_prefers_closest_vector() {
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();
        let ranked = semantic_ranking(
            &[1.0, 0.0],
            [(first, vec![0.9, 0.1]), (second, vec![0.0, 1.0])],
        );
        assert_eq!(ranked[0].0, first);
    }
}
