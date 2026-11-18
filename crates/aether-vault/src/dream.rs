use std::collections::HashSet;

use crate::parser::{
    detect_opening_fence, inspect_note_frontmatter, is_closing_fence, parse_atx_heading,
};
use crate::types::{Amu, AmuId, ScopeTag, VaultTier};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DreamCandidate {
    pub(crate) proposed_fact: String,
    pub(crate) suggested_tier: VaultTier,
    pub(crate) scope: ScopeTag,
    pub(crate) target_note: Option<String>,
    pub(crate) supersedes_amu_id: Option<AmuId>,
}

/// Returns `true` when `rel_path` is a Daily/Journal note or sets `aether_kind: fleeting`
/// while not marked `aether_tier: private`.
pub(crate) fn is_daily_or_fleeting_note(rel_path: &str, raw_md: &str) -> bool {
    let (hint, _) = inspect_note_frontmatter(rel_path, raw_md);
    if hint.is_private {
        return false;
    }
    let lower = rel_path.replace('\\', "/").to_ascii_lowercase();
    let in_daily_folder = lower.starts_with("daily/")
        || lower.starts_with("journal/")
        || lower.contains("/daily/")
        || lower.contains("/journal/");
    in_daily_folder || hint.is_fleeting
}

/// Extracts durable decisions, rules, preferences, and temporal contradictions from a Daily Note.
pub(crate) fn extract_dream_candidates(
    rel_path: &str,
    raw_md: &str,
    active_amus: &[Amu],
) -> Vec<DreamCandidate> {
    let (hint, body) = inspect_note_frontmatter(rel_path, raw_md);
    if hint.is_private {
        return Vec::new();
    }

    let mut fence: Option<(u8, usize)> = None;
    let mut current_heading = String::new();
    let mut seen_facts: HashSet<String> = HashSet::new();
    let mut out = Vec::new();

    for raw_line in body.lines() {
        let trimmed = raw_line.trim();
        if let Some((fchar, flen)) = fence {
            if is_closing_fence(trimmed, fchar, flen) {
                fence = None;
            }
            continue;
        }
        if let Some(opening) = detect_opening_fence(trimmed) {
            fence = Some(opening);
            continue;
        }
        if let Some((_, title)) = parse_atx_heading(trimmed) {
            current_heading = title.to_ascii_lowercase();
            continue;
        }
        if trimmed.is_empty() {
            continue;
        }

        if let Some((fact, tier)) = classify_candidate_line(trimmed, &current_heading) {
            if !seen_facts.insert(fact.to_ascii_lowercase()) {
                continue;
            }
            let target_note = normalize_wikilink_target(&fact);
            let supersedes_amu_id = find_superseded_amu(
                rel_path,
                &fact,
                target_note.as_deref(),
                &hint.scope,
                active_amus,
            );
            out.push(DreamCandidate {
                proposed_fact: fact,
                suggested_tier: tier,
                scope: hint.scope.clone(),
                target_note,
                supersedes_amu_id,
            });
        }
    }

    out
}

fn classify_candidate_line(trimmed: &str, current_heading: &str) -> Option<(String, VaultTier)> {
    // Skip unchecked brainstorm checkboxes unless they carry an explicit decision/rule tag.
    let is_unchecked_todo = trimmed.starts_with("- [ ]") || trimmed.starts_with("* [ ]");
    let stripped = trimmed
        .strip_prefix("- [x]")
        .or_else(|| trimmed.strip_prefix("- [X]"))
        .or_else(|| trimmed.strip_prefix("- [ ]"))
        .or_else(|| trimmed.strip_prefix("* [x]"))
        .or_else(|| trimmed.strip_prefix("* [X]"))
        .or_else(|| trimmed.strip_prefix("* [ ]"))
        .or_else(|| trimmed.strip_prefix("- "))
        .or_else(|| trimmed.strip_prefix("* "))
        .or_else(|| trimmed.strip_prefix("> "))
        .unwrap_or(trimmed)
        .trim();

    if stripped.is_empty() || stripped.starts_with("- [-]") {
        return None;
    }

    let lower = stripped.to_ascii_lowercase();
    let is_rule_or_pref = lower.starts_with("rule:")
        || lower.starts_with("[rule]")
        || lower.starts_with("preference:")
        || lower.starts_with("[preference]")
        || lower.starts_with("always ")
        || lower.starts_with("never ");
    let is_explicit_decision = lower.starts_with("decision:")
        || lower.starts_with("[decision]")
        || lower.starts_with("[!decision]")
        || lower.starts_with("decided to ")
        || lower.starts_with("we decided ")
        || lower.starts_with("switched to ")
        || lower.starts_with("migrated to ")
        || lower.starts_with("replaced ");

    let heading_is_durable = current_heading.contains("decision")
        || current_heading.contains("rule")
        || current_heading.contains("preference")
        || current_heading.contains("architecture");

    if is_unchecked_todo && !is_explicit_decision && !is_rule_or_pref {
        return None;
    }
    if !is_rule_or_pref && !is_explicit_decision && !heading_is_durable {
        return None;
    }

    let cleaned = strip_cue_prefix(stripped);
    if cleaned.len() < 8 {
        return None;
    }

    let tier = if is_rule_or_pref
        || current_heading.contains("preference")
        || current_heading.contains("rule")
    {
        VaultTier::Hot
    } else {
        VaultTier::Warm
    };

    Some((cleaned, tier))
}

fn strip_cue_prefix(raw: &str) -> String {
    let lower = raw.to_ascii_lowercase();
    for prefix in [
        "decision:",
        "[decision]",
        "[!decision]",
        "rule:",
        "[rule]",
        "preference:",
        "[preference]",
    ] {
        if lower.starts_with(prefix) {
            return raw[prefix.len()..].trim().to_string();
        }
    }
    raw.trim().to_string()
}

fn find_superseded_amu(
    source_rel_path: &str,
    candidate_fact: &str,
    target_note: Option<&str>,
    scope: &ScopeTag,
    active_amus: &[Amu],
) -> Option<AmuId> {
    let lower = candidate_fact.to_ascii_lowercase();
    let has_replacement_cue = [
        "switched",
        "migrated",
        "replaced",
        "instead of",
        "no longer",
        "deprecated",
        "superseded",
    ]
    .iter()
    .any(|cue| lower.contains(cue));

    if !has_replacement_cue {
        return None;
    }

    let cand_tokens = meaningful_tokens(candidate_fact);
    let target_stem = target_note.map(|t| {
        let low = t.replace('\\', "/").to_ascii_lowercase();
        low.strip_suffix(".md").unwrap_or(&low).to_string()
    });

    let mut best: Option<(usize, AmuId)> = None;
    for amu in active_amus {
        if amu
            .source_pointer
            .relative_path()
            .eq_ignore_ascii_case(source_rel_path)
        {
            continue;
        }
        if !amu
            .scopes
            .iter()
            .any(|s| s == scope || s.as_str() == "global")
        {
            continue;
        }
        let mut score = 0usize;
        if let Some(stem) = &target_stem {
            let amu_stem = amu.source_pointer.relative_path().to_ascii_lowercase();
            let amu_stem = amu_stem.strip_suffix(".md").unwrap_or(&amu_stem);
            if amu_stem == stem {
                score += 5;
            } else if amu.graph_edges.iter().any(|e| e == &format!("note:{stem}")) {
                score += 2;
            }
        }
        let amu_tokens = meaningful_tokens(&amu.distilled_fact);
        let overlap = cand_tokens.intersection(&amu_tokens).count();
        score += overlap;

        let qualifies = (score >= 3 && overlap >= 1) || overlap >= 2;
        if qualifies && best.is_none_or(|(best_score, _)| score > best_score) {
            best = Some((score, amu.amu_id));
        }
    }

    best.map(|(_, id)| id)
}

fn normalize_wikilink_target(raw: &str) -> Option<String> {
    let start = raw.find("[[")?;
    let after = &raw[start + 2..];
    let end = after.find("]]")?;
    let inner = &after[..end];
    let target = inner.split('|').next().unwrap_or("");
    let note = target
        .split('#')
        .next()
        .unwrap_or("")
        .trim()
        .replace('\\', "/");
    if note.is_empty() {
        None
    } else {
        Some(note)
    }
}

fn meaningful_tokens(text: &str) -> HashSet<String> {
    const STOPWORDS: &[&str] = &[
        "the",
        "and",
        "for",
        "with",
        "from",
        "into",
        "that",
        "this",
        "use",
        "using",
        "local",
        "primary",
        "used",
        "will",
        "should",
        "must",
        "decision",
        "rule",
        "preference",
        "switched",
        "migrated",
        "replaced",
        "instead",
        "always",
        "never",
    ];
    text.to_ascii_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() >= 3 && !STOPWORDS.contains(t))
        .map(ToOwned::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;
    use crate::types::{AmuKind, AmuStatus, ContentHash, SourcePointer};

    fn make_active_amu(pointer: &str, scope: &str, fact: &str) -> Amu {
        let ptr = SourcePointer::parse(pointer).unwrap();
        Amu {
            amu_id: AmuId::from_pointer(&ptr),
            tier: VaultTier::Warm,
            kind: AmuKind::Decision,
            status: AmuStatus::Active,
            heading_path: ptr.as_str().to_string(),
            distilled_fact: fact.to_string(),
            source_pointer: ptr,
            graph_edges: Vec::new(),
            valid_from: Utc::now(),
            superseded_by: None,
            scopes: vec![ScopeTag::new(scope).unwrap()],
            authority_weight: 1.0,
            content_hash: ContentHash::compute(fact.as_bytes()),
        }
    }

    #[test]
    fn extracts_decisions_rules_and_contradictions_while_ignoring_code_blocks() {
        let old_amu = make_active_amu(
            "Arch/Vector.md#Engine",
            "engineering",
            "Use Qdrant for local vector search.",
        );
        let daily_md = "\
---
aether_scope: engineering
---
# Daily Log
Just chatting with teammates about lunch.

## Decisions
- Decision: Switched local vector search from Qdrant to LanceDB in [[Arch/Vector]].
- Always run cargo clippy with -D warnings before push.

```markdown
- Decision: fake decision inside fenced code block
```
";
        assert!(is_daily_or_fleeting_note("Daily/2026-05-10.md", daily_md));
        let extracted = extract_dream_candidates(
            "Daily/2026-05-10.md",
            daily_md,
            std::slice::from_ref(&old_amu),
        );
        assert_eq!(extracted.len(), 2);
        assert!(extracted[0].proposed_fact.contains("LanceDB"));
        assert_eq!(extracted[0].target_note.as_deref(), Some("Arch/Vector"));
        assert_eq!(extracted[0].supersedes_amu_id, Some(old_amu.amu_id));
        assert_eq!(extracted[1].suggested_tier, VaultTier::Hot);
        assert!(!extracted
            .iter()
            .any(|c| c.proposed_fact.contains("fake decision")));
    }
}
