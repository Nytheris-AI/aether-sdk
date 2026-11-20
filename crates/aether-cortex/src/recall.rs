//! Composable recall steps, scoring, scope filtering, and reciprocal rank fusion.

use std::collections::{HashMap, HashSet};

use muninn_aether_core::{
    DistanceMetric, EntityId, FilterExpr, FilterValue, GraphStore, MemoryId, MemoryItem,
    MemoryScope, ProjectId, RecordStore, Result, SessionId, Subgraph, TemporalFilter, VectorStore,
};
use tracing::Instrument;

use crate::indexer::index_metadata;
use crate::ppr::{graph_scores, PersonalizedPageRank};

/// Default smoothing constant `k` for Reciprocal Rank Fusion.
pub const RRF_K: f32 = 60.0;

/// Retrieved memory record paired with a ranking score.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub item: MemoryItem,
    pub score: f32,
}

/// Builds an index-side metadata filter covering non-session scopes (`project` and `user:<id>`).
pub fn scope_filter(scopes: &[MemoryScope]) -> Option<FilterExpr> {
    let mut seen = HashSet::new();
    let mut values = Vec::new();
    for scope in scopes {
        if let Some(key) = scope.index_key() {
            if seen.insert(key.clone()) {
                values.push(FilterValue::String(key));
            }
        }
    }
    if values.is_empty() {
        None
    } else {
        Some(FilterExpr::In {
            field: "_aether_scope".into(),
            values,
        })
    }
}

/// Searches the vector index and resolves live records from `records`, dropping deleted or expired hits.
pub async fn vector_search(
    vectors: &dyn VectorStore,
    records: &dyn RecordStore,
    ns: &ProjectId,
    query_vector: &[f32],
    top_k: usize,
    filter: Option<&FilterExpr>,
) -> Result<Vec<Candidate>> {
    let span = tracing::info_span!(
        "aether.cortex.vector_search",
        project_id = %ns,
        top_k,
    );
    async move {
        if top_k == 0 {
            return Ok(Vec::new());
        }
        let matches = vectors.search(ns, query_vector, top_k, filter).await?;
        if matches.is_empty() {
            return Ok(Vec::new());
        }
        let ids: Vec<MemoryId> = matches.iter().map(|m| m.id).collect();
        let resolved: HashMap<MemoryId, MemoryItem> = records
            .get_many(ns, &ids)
            .await?
            .into_iter()
            .map(|item| (item.id, item))
            .collect();

        let mut candidates: Vec<Candidate> = matches
            .into_iter()
            .filter_map(|m| {
                resolved.get(&m.id).cloned().map(|item| Candidate {
                    item,
                    score: m.score,
                })
            })
            .collect();
        sort_candidates_desc(&mut candidates);
        Ok(candidates)
    }
    .instrument(span)
    .await
}

/// Scores pending unindexed records in `records` against `query_vector` using `metric`.
pub async fn unindexed(
    records: &dyn RecordStore,
    ns: &ProjectId,
    query_vector: &[f32],
    metric: DistanceMetric,
    filter: Option<&FilterExpr>,
) -> Result<Vec<Candidate>> {
    let pending = records.pending_for(ns, 256).await?;
    Ok(score_records(pending, query_vector, metric, filter))
}

/// Scores live session-scoped items in `records` against `query_vector` using `metric`.
pub async fn session(
    records: &dyn RecordStore,
    ns: &ProjectId,
    session_id: &SessionId,
    query_vector: &[f32],
    metric: DistanceMetric,
    filter: Option<&FilterExpr>,
) -> Result<Vec<Candidate>> {
    let items = records.session_items(ns, session_id, 256).await?;
    Ok(score_records(items, query_vector, metric, filter))
}

fn score_records(
    items: Vec<MemoryItem>,
    query_vector: &[f32],
    metric: DistanceMetric,
    filter: Option<&FilterExpr>,
) -> Vec<Candidate> {
    let mut candidates: Vec<Candidate> = items
        .into_iter()
        .filter_map(|item| {
            if let Some(expr) = filter {
                if !expr.matches(&index_metadata(&item)) {
                    return None;
                }
            }
            let emb = item.embedding.as_ref()?;
            if emb.len() != query_vector.len() {
                return None;
            }
            let score = metric.score(query_vector, emb);
            Some(Candidate { item, score })
        })
        .collect();
    sort_candidates_desc(&mut candidates);
    candidates
}

/// Traverses the knowledge graph, scores entities with PPR, and resolves source memories via `records`.
#[allow(clippy::too_many_arguments)]
pub async fn graph_search(
    graph: &dyn GraphStore,
    records: &dyn RecordStore,
    ns: &ProjectId,
    seeds: &[EntityId],
    hops: u8,
    per_node_limit: usize,
    temporal: &TemporalFilter,
    ppr: &PersonalizedPageRank,
) -> Result<(Vec<Candidate>, Subgraph)> {
    let span = tracing::info_span!(
        "aether.cortex.graph_search",
        project_id = %ns,
        seed_count = seeds.len(),
        hops,
    );
    async move {
        if seeds.is_empty() {
            return Ok((Vec::new(), Subgraph::default()));
        }
        let subgraph = graph
            .neighborhood(ns, seeds, hops, per_node_limit, temporal)
            .await?;
        if subgraph.edges.is_empty() {
            return Ok((Vec::new(), subgraph));
        }

        let entity_ranks = ppr.rank(&subgraph, seeds);
        let mem_scores = graph_scores(&subgraph, &entity_ranks);
        let ids: Vec<MemoryId> = mem_scores.keys().copied().collect();
        let resolved = records.get_many(ns, &ids).await?;

        let mut candidates: Vec<Candidate> = resolved
            .into_iter()
            .filter_map(|item| {
                mem_scores
                    .get(&item.id)
                    .copied()
                    .map(|score| Candidate { item, score })
            })
            .collect();
        sort_candidates_desc(&mut candidates);
        Ok((candidates, subgraph))
    }
    .instrument(span)
    .await
}

/// Merges same-metric candidate lists, keeping the newest `version` (tie-break highest score) per `MemoryId`.
pub fn merge(lists: Vec<Vec<Candidate>>) -> Vec<Candidate> {
    let mut by_id: HashMap<MemoryId, Candidate> = HashMap::new();
    for list in lists {
        for cand in list {
            match by_id.get_mut(&cand.item.id) {
                Some(existing) => {
                    if cand.item.version > existing.item.version
                        || (cand.item.version == existing.item.version
                            && cand.score > existing.score)
                    {
                        *existing = cand;
                    }
                }
                None => {
                    by_id.insert(cand.item.id, cand);
                }
            }
        }
    }
    let mut merged: Vec<Candidate> = by_id.into_values().collect();
    sort_candidates_desc(&mut merged);
    merged
}

/// Fuses ranked candidate lists via Reciprocal Rank Fusion (`1.0 / (k + rank)`), keeping the newest version.
pub fn rrf(lists: &[Vec<Candidate>], k: f32) -> Vec<Candidate> {
    let mut items: HashMap<MemoryId, MemoryItem> = HashMap::new();
    let mut scores: HashMap<MemoryId, f32> = HashMap::new();

    for list in lists {
        for (idx, cand) in list.iter().enumerate() {
            let rank = (idx + 1) as f32;
            *scores.entry(cand.item.id).or_insert(0.0) += 1.0 / (k + rank);
            match items.get_mut(&cand.item.id) {
                Some(existing) if cand.item.version > existing.version => {
                    *existing = cand.item.clone();
                }
                Some(_) => {}
                None => {
                    items.insert(cand.item.id, cand.item.clone());
                }
            }
        }
    }

    let mut fused: Vec<Candidate> = items
        .into_iter()
        .map(|(id, item)| Candidate {
            item,
            score: scores.get(&id).copied().unwrap_or(0.0),
        })
        .collect();
    sort_candidates_desc(&mut fused);
    fused
}

/// Filters candidates to those whose `MemoryScope` is present in `scopes`.
pub fn in_scopes(candidates: Vec<Candidate>, scopes: &[MemoryScope]) -> Vec<Candidate> {
    candidates
        .into_iter()
        .filter(|c| scopes.contains(&c.item.scope))
        .collect()
}

fn sort_candidates_desc(candidates: &mut [Candidate]) {
    candidates.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.item.id.cmp(&b.item.id))
    });
}
