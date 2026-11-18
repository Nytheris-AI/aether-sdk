use std::collections::HashSet;

use crate::embedder::ComputeDevice;
use crate::types::RecallEntry;

/// Default top-K RRF candidate slice evaluated by [`CrossEncoderReranker`].
pub const DEFAULT_RERANK_TOP_K: usize = 30;

/// Second-stage reranker applied over top RRF recall candidates.
pub trait Reranker: Send + Sync {
    /// Stable identifier for diagnostics and telemetry.
    fn id(&self) -> &str;
    /// Maximum candidate prefix length reranked per recall query.
    fn top_k(&self) -> usize {
        DEFAULT_RERANK_TOP_K
    }
    /// Reranks `candidates` in place.
    fn rerank(&self, query: &str, candidates: &mut [RecallEntry]);
}

/// No-op reranker preserving raw RRF ordering.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopReranker;

impl Reranker for NoopReranker {
    fn id(&self) -> &str {
        "noop"
    }

    fn rerank(&self, _query: &str, _candidates: &mut [RecallEntry]) {}
}

/// Cross-interaction reranker scoring query-document term coverage, ordered phrase
/// adjacency, and heading-path alignment over the top-`K` RRF candidates.
#[derive(Debug, Clone)]
pub struct CrossEncoderReranker {
    id: String,
    top_k: usize,
    device: ComputeDevice,
}

impl CrossEncoderReranker {
    pub fn new(device: ComputeDevice) -> Self {
        Self {
            id: "cross-encoder-v1".to_string(),
            top_k: DEFAULT_RERANK_TOP_K,
            device,
        }
    }

    pub fn with_top_k(mut self, top_k: usize) -> Self {
        self.top_k = top_k.max(1);
        self
    }

    pub fn device(&self) -> ComputeDevice {
        self.device
    }
}

impl Default for CrossEncoderReranker {
    fn default() -> Self {
        Self::new(ComputeDevice::Cpu)
    }
}

impl Reranker for CrossEncoderReranker {
    fn id(&self) -> &str {
        &self.id
    }

    fn top_k(&self) -> usize {
        self.top_k
    }

    fn rerank(&self, query: &str, candidates: &mut [RecallEntry]) {
        let limit = candidates.len().min(self.top_k);
        if limit <= 1 {
            return;
        }
        let query_tokens = tokenize_lower(query);
        if query_tokens.is_empty() {
            return;
        }
        let unique_query: HashSet<&str> = query_tokens.iter().map(String::as_str).collect();
        let query_norm = query_tokens.join(" ");

        for entry in &mut candidates[..limit] {
            let heading_tokens = tokenize_lower(&entry.amu.heading_path);
            let fact_tokens = tokenize_lower(&entry.amu.distilled_fact);
            let mut combined_tokens = Vec::with_capacity(heading_tokens.len() + fact_tokens.len());
            combined_tokens.extend(heading_tokens.iter().cloned());
            combined_tokens.extend(fact_tokens.iter().cloned());

            let combined_set: HashSet<&str> = combined_tokens.iter().map(String::as_str).collect();
            let heading_set: HashSet<&str> = heading_tokens.iter().map(String::as_str).collect();

            let matched_unique = unique_query
                .iter()
                .filter(|t| combined_set.contains(*t))
                .count();
            let token_coverage = matched_unique as f32 / unique_query.len() as f32;

            let heading_matched = unique_query
                .iter()
                .filter(|t| heading_set.contains(*t))
                .count();
            let heading_coverage = heading_matched as f32 / unique_query.len() as f32;

            let bigram_ratio = ordered_bigram_ratio(&query_tokens, &combined_tokens);

            let phrase_bonus = if query_tokens.len() >= 2 {
                let heading_norm = heading_tokens.join(" ");
                let fact_norm = fact_tokens.join(" ");
                if heading_norm.contains(&query_norm) || fact_norm.contains(&query_norm) {
                    1.0
                } else {
                    0.0
                }
            } else {
                0.0
            };

            let cross_score = 0.40 * token_coverage
                + 0.30 * bigram_ratio
                + 0.15 * phrase_bonus
                + 0.15 * heading_coverage;
            entry.score += cross_score * entry.amu.authority_weight.max(0.1) * 0.10;
        }

        candidates[..limit].sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then_with(|| a.amu.amu_id.cmp(&b.amu.amu_id))
        });
    }
}

fn tokenize_lower(text: &str) -> Vec<String> {
    text.to_ascii_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn ordered_bigram_ratio(query_tokens: &[String], doc_tokens: &[String]) -> f32 {
    if query_tokens.len() < 2 || doc_tokens.len() < 2 {
        return 0.0;
    }
    let doc_bigrams: HashSet<(&str, &str)> = doc_tokens
        .windows(2)
        .map(|w| (w[0].as_str(), w[1].as_str()))
        .collect();
    let total = query_tokens.len() - 1;
    let matched = query_tokens
        .windows(2)
        .filter(|w| doc_bigrams.contains(&(w[0].as_str(), w[1].as_str())))
        .count();
    matched as f32 / total as f32
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use chrono::Utc;

    use super::*;
    use crate::types::{
        Amu, AmuId, AmuKind, AmuStatus, ContentHash, ScopeTag, SourcePointer, VaultTier,
    };

    fn make_entry(pointer: &str, heading: &str, fact: &str, score: f32) -> RecallEntry {
        let ptr = SourcePointer::parse(pointer).unwrap();
        RecallEntry {
            amu: Amu {
                amu_id: AmuId::from_pointer(&ptr),
                tier: VaultTier::Warm,
                kind: AmuKind::Spec,
                status: AmuStatus::Active,
                heading_path: heading.to_string(),
                distilled_fact: fact.to_string(),
                source_pointer: ptr,
                graph_edges: Vec::new(),
                valid_from: Utc::now(),
                superseded_by: None,
                scopes: vec![ScopeTag::global()],
                authority_weight: 1.0,
                content_hash: ContentHash::compute(fact.as_bytes()),
            },
            score,
        }
    }

    #[test]
    fn cross_encoder_promotes_exact_phrase_and_heading_match_under_50ms() {
        let mut candidates = Vec::new();
        // Top RRF entry only matches scattered keywords.
        candidates.push(make_entry(
            "Notes/Distractor.md#General",
            "General Notes",
            "Token mentioned once, then lots of noise before rotation is discussed elsewhere.",
            0.035,
        ));
        for idx in 0..28 {
            candidates.push(make_entry(
                &format!("Notes/Filler{idx}.md#Item"),
                "Filler Section",
                "Filler background engineering paragraph.",
                0.020 - (idx as f32 * 0.0001),
            ));
        }
        // Lower-ranked candidate has exact heading and phrase match.
        candidates.push(make_entry(
            "Specs/Auth.md#Token-Rotation",
            "Auth > Token Rotation",
            "Use token rotation grace period of 300 seconds.",
            0.015,
        ));

        let reranker = CrossEncoderReranker::default();
        let start = Instant::now();
        reranker.rerank("token rotation", &mut candidates);
        let elapsed = start.elapsed();

        assert!(
            elapsed.as_millis() < 50,
            "reranking 30 candidates took {}ms",
            elapsed.as_millis()
        );
        assert_eq!(
            candidates[0].amu.source_pointer.as_str(),
            "Specs/Auth.md#Token-Rotation"
        );
    }
}
