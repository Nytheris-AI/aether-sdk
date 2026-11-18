use std::collections::HashSet;

use crate::types::{
    estimate_tokens, Amu, AmuId, AmuStatus, ContentHash, RecallEntry, RecallResponse, TokenBudget,
    VaultTier, HOT_PROFILE_MAX_TOKENS,
};

const HOT_HEADER: &str = "[AETHER_HOT_PROFILE]\n";
const DYNAMIC_HEADER: &str = "[AETHER_RECALL_CONTEXT]\n";

/// Formats active `HOT` and dynamic `WARM`/`COLD` recall entries into a prefix-cache-friendly
/// response obeying strict token ceilings and zero volatile timestamps inside the static block.
pub(crate) fn format_recall_response(
    hot_amus: Vec<Amu>,
    ranked_entries: Vec<RecallEntry>,
    hot_budget: TokenBudget,
    max_tokens: TokenBudget,
    include_hot_profile: bool,
) -> RecallResponse {
    let mut selected_hot = Vec::new();
    let mut included_ids: HashSet<AmuId> = HashSet::new();
    let mut hot_prefix = String::new();

    if include_hot_profile {
        let mut sorted_hot: Vec<Amu> = hot_amus
            .into_iter()
            .filter(|a| a.tier == VaultTier::Hot && a.status == AmuStatus::Active)
            .collect();
        sorted_hot.sort_by_key(|a| a.amu_id);
        sorted_hot.dedup_by(|a, b| a.amu_id == b.amu_id);

        let effective_hot_budget = hot_budget
            .get()
            .min(HOT_PROFILE_MAX_TOKENS)
            .min(max_tokens.get());

        for amu in sorted_hot {
            let line = format!(
                "- [{}] ({}) {}\n",
                amu.amu_id, amu.source_pointer, amu.distilled_fact
            );
            let mut candidate = if hot_prefix.is_empty() {
                String::from(HOT_HEADER)
            } else {
                hot_prefix.clone()
            };
            candidate.push_str(&line);
            if estimate_tokens(&candidate) <= effective_hot_budget {
                hot_prefix = candidate;
                included_ids.insert(amu.amu_id);
                selected_hot.push(amu);
            }
        }
    }

    let prefix_hash = ContentHash::compute(hot_prefix.as_bytes());
    let mut dynamic_context = String::new();
    let mut warm_cold_amus = Vec::new();
    let mut entries = Vec::new();

    for entry in ranked_entries {
        if entry.amu.status != AmuStatus::Active || !included_ids.insert(entry.amu.amu_id) {
            continue;
        }
        let tier_label = entry.amu.tier.as_str().to_ascii_uppercase();
        let line = format!(
            "- [{}] [{}] ({}) {}\n",
            entry.amu.amu_id, tier_label, entry.amu.source_pointer, entry.amu.distilled_fact
        );
        let mut candidate_dyn = if dynamic_context.is_empty() {
            String::from(DYNAMIC_HEADER)
        } else {
            dynamic_context.clone()
        };
        candidate_dyn.push_str(&line);

        let combined = combine_blocks(&hot_prefix, &candidate_dyn);
        if estimate_tokens(&combined) <= max_tokens.get() {
            dynamic_context = candidate_dyn;
            warm_cold_amus.push(entry.amu.clone());
            entries.push(entry);
        }
    }

    let formatted_context = combine_blocks(&hot_prefix, &dynamic_context);
    let static_prefix_tokens = estimate_tokens(&hot_prefix);
    let dynamic_tokens = estimate_tokens(&dynamic_context);
    let tokens_used = estimate_tokens(&formatted_context);
    let static_prefix = hot_prefix.clone();

    RecallResponse {
        formatted_context,
        hot_prefix,
        static_prefix,
        dynamic_context,
        hot_amus: selected_hot,
        warm_cold_amus,
        entries,
        static_prefix_tokens,
        dynamic_tokens,
        tokens_used,
        prefix_hash,
    }
}

fn combine_blocks(hot_prefix: &str, dynamic_context: &str) -> String {
    match (hot_prefix.is_empty(), dynamic_context.is_empty()) {
        (true, true) => String::new(),
        (false, true) => hot_prefix.to_string(),
        (true, false) => dynamic_context.to_string(),
        (false, false) => format!("{hot_prefix}\n{dynamic_context}"),
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;
    use crate::types::{AmuKind, ScopeTag, SourcePointer};

    fn make_amu(
        path: &str,
        tier: VaultTier,
        status: AmuStatus,
        fact: &str,
        days_offset: i64,
    ) -> Amu {
        let pointer = SourcePointer::parse(path).unwrap();
        let amu_id = AmuId::from_pointer(&pointer);
        Amu {
            amu_id,
            tier,
            kind: AmuKind::Decision,
            status,
            heading_path: pointer.as_str().to_string(),
            distilled_fact: fact.to_string(),
            source_pointer: pointer,
            graph_edges: Vec::new(),
            valid_from: Utc::now() + chrono::Duration::days(days_offset),
            superseded_by: None,
            scopes: vec![ScopeTag::global()],
            authority_weight: 1.0,
            content_hash: ContentHash::compute(fact.as_bytes()),
        }
    }

    #[test]
    fn hot_prefix_sorts_canonically_by_amu_id_with_identical_hash_across_queries() {
        let h1 = make_amu(
            "Profile/A.md#One",
            VaultTier::Hot,
            AmuStatus::Active,
            "Rule Alpha.",
            1,
        );
        let h2 = make_amu(
            "Profile/B.md#Two",
            VaultTier::Hot,
            AmuStatus::Active,
            "Rule Beta.",
            99,
        );
        let w1 = make_amu(
            "Specs/W1.md#DB",
            VaultTier::Warm,
            AmuStatus::Active,
            "Query one fact.",
            0,
        );
        let w2 = make_amu(
            "Specs/W2.md#Cache",
            VaultTier::Warm,
            AmuStatus::Active,
            "Query two fact.",
            0,
        );

        let budget = TokenBudget::new(500).unwrap();
        let hot_cap = TokenBudget::new(HOT_PROFILE_MAX_TOKENS).unwrap();

        let resp_a = format_recall_response(
            vec![h1.clone(), h2.clone()],
            vec![RecallEntry {
                amu: w1,
                score: 0.9,
            }],
            hot_cap,
            budget,
            true,
        );
        let resp_b = format_recall_response(
            vec![h2.clone(), h1.clone()],
            vec![RecallEntry {
                amu: w2,
                score: 0.8,
            }],
            hot_cap,
            budget,
            true,
        );

        assert_eq!(resp_a.prefix_hash, resp_b.prefix_hash);
        assert_eq!(resp_a.hot_prefix, resp_b.hot_prefix);
        assert_eq!(resp_a.static_prefix, resp_a.hot_prefix);
        assert_eq!(
            resp_a.static_prefix_tokens,
            estimate_tokens(&resp_a.static_prefix)
        );
        assert_eq!(
            resp_a.dynamic_tokens,
            estimate_tokens(&resp_a.dynamic_context)
        );
        assert!(resp_a.hot_amus[0].amu_id < resp_a.hot_amus[1].amu_id);
        assert!(resp_a.formatted_context.starts_with(&resp_a.hot_prefix));
        assert!(resp_a.tokens_used <= 500);
    }

    #[test]
    fn strict_max_tokens_and_exclusion_of_inactive_or_duplicate_hot_entries() {
        let hot = make_amu(
            "Profile/Core.md#Core",
            VaultTier::Hot,
            AmuStatus::Active,
            "Core hot rule.",
            0,
        );
        let superseded = make_amu(
            "Specs/Old.md#Old",
            VaultTier::Warm,
            AmuStatus::Superseded,
            "Old rule.",
            0,
        );
        let warm_long = make_amu(
            "Specs/Long.md#Long",
            VaultTier::Warm,
            AmuStatus::Active,
            &"longword ".repeat(80),
            0,
        );
        let warm_short = make_amu(
            "Specs/Short.md#Short",
            VaultTier::Warm,
            AmuStatus::Active,
            "Short valid fact.",
            0,
        );

        let budget = TokenBudget::new(60).unwrap();
        let resp = format_recall_response(
            vec![hot.clone()],
            vec![
                RecallEntry {
                    amu: hot.clone(),
                    score: 1.0,
                },
                RecallEntry {
                    amu: superseded,
                    score: 0.95,
                },
                RecallEntry {
                    amu: warm_long,
                    score: 0.9,
                },
                RecallEntry {
                    amu: warm_short.clone(),
                    score: 0.8,
                },
            ],
            TokenBudget::new(HOT_PROFILE_MAX_TOKENS).unwrap(),
            budget,
            true,
        );

        assert!(resp.tokens_used <= 60);
        assert_eq!(resp.hot_amus.len(), 1);
        assert_eq!(resp.warm_cold_amus, vec![warm_short]);
        assert!(!resp.formatted_context.contains("Old rule."));
    }
}
