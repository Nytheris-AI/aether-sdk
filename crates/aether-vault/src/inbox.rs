use std::io::Write;
use std::path::Path;

use nytheris_aether_core::{AetherError, Result};
use uuid::Uuid;

use crate::types::{
    AmuId, CandidateId, ContentHash, InboxCheckbox, InboxEntry, ScopeTag, VaultTier,
};

pub(crate) const INBOX_REL_PATH: &str = "Aether/Inbox.md";

const INBOX_HEADER: &str = "# Aether Memory Inbox\n> Check `[x]` to approve and compile into active vault memory. Delete or mark `[-]` to reject.\n";

/// Atomically replaces `path` via a sibling temp file, `sync_all`, and `rename`.
pub(crate) fn atomic_write_file(path: &Path, bytes: &[u8]) -> Result<ContentHash> {
    let parent = path.parent().ok_or_else(|| {
        AetherError::InvalidInput(format!(
            "target path `{}` has no parent dir",
            path.display()
        ))
    })?;
    std::fs::create_dir_all(parent).map_err(|err| AetherError::Permanent {
        connector: "vault-fs",
        message: format!("failed to create dir `{}`: {err}", parent.display()),
    })?;

    let file_name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("file.md");
    let tmp_path = parent.join(format!(".tmp.{file_name}.{}", Uuid::new_v4().simple()));

    let write_res = (|| -> std::io::Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&tmp_path)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp_path, path)?;
        Ok(())
    })();

    if let Err(err) = write_res {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(AetherError::Permanent {
            connector: "vault-fs",
            message: format!("atomic write failed for `{}`: {err}", path.display()),
        });
    }

    Ok(ContentHash::compute(bytes))
}

/// Parses PRD Section 7.3 entries from `Aether/Inbox.md`.
pub(crate) fn parse_inbox(markdown: &str) -> Vec<InboxEntry> {
    let mut entries = Vec::new();
    let mut current: Option<InboxEntry> = None;

    for raw_line in markdown.lines() {
        let trimmed = raw_line.trim();
        if let Some(entry) = parse_task_header(trimmed) {
            if let Some(prev) = current.take() {
                entries.push(prev);
            }
            current = Some(entry);
            continue;
        }

        if let Some(entry) = current.as_mut() {
            if let Some(bullet) = trimmed.strip_prefix("- ") {
                apply_metadata_bullet(entry, bullet);
            }
        }
    }

    if let Some(last) = current.take() {
        entries.push(last);
    }

    entries
}

fn parse_task_header(trimmed: &str) -> Option<InboxEntry> {
    let (checkbox, rest) = if let Some(r) = trimmed.strip_prefix("- [ ] ") {
        (InboxCheckbox::Pending, r.trim())
    } else if let Some(r) = trimmed
        .strip_prefix("- [x] ")
        .or_else(|| trimmed.strip_prefix("- [X] "))
    {
        (InboxCheckbox::Approved, r.trim())
    } else {
        let r = trimmed.strip_prefix("- [-] ")?;
        (InboxCheckbox::Rejected, r.trim())
    };

    let (tier, scope, proposed_fact) = if let Some(after_open) = rest.strip_prefix("**[") {
        let end_bracket = after_open.find("]**")?;
        let header_meta = &after_open[..end_bracket];
        let fact = after_open[end_bracket + 3..].trim();
        if fact.is_empty() {
            return None;
        }
        let (tier_str, scope_str) = match header_meta.split_once('|') {
            Some((t, s)) => (t.trim(), s.trim()),
            None => (header_meta.trim(), "global"),
        };
        let tier = VaultTier::parse_str(tier_str).unwrap_or(VaultTier::Warm);
        let scope = ScopeTag::new(scope_str).unwrap_or_else(|_| ScopeTag::global());
        (tier, scope, fact.to_string())
    } else {
        if rest.is_empty() {
            return None;
        }
        (VaultTier::Warm, ScopeTag::global(), rest.to_string())
    };

    let candidate_id = CandidateId::from_proposal(&proposed_fact, None, tier);
    Some(InboxEntry {
        checkbox,
        tier,
        scope,
        proposed_fact,
        candidate_id,
        target_link: None,
        proposed_by: "mcp-agent".into(),
        proposed_date: String::new(),
        supersedes_amu_id: None,
    })
}

fn apply_metadata_bullet(entry: &mut InboxEntry, bullet: &str) {
    let Some((raw_key, raw_val)) = bullet.split_once(':') else {
        return;
    };
    let key = unquote_backticks(raw_key).to_ascii_lowercase();
    let val = raw_val.trim();

    match key.as_str() {
        "amu_candidate_id" | "candidate_id" => {
            if let Ok(id) = CandidateId::parse(&unquote_backticks(val)) {
                entry.candidate_id = id;
            }
        }
        "target_link" | "target_note" => {
            let clean = val
                .trim()
                .trim_start_matches("[[")
                .trim_end_matches("]]")
                .trim();
            if !clean.is_empty() {
                entry.target_link = Some(clean.to_string());
            }
        }
        "proposed_by" => {
            if let Some((who, date)) = val.split_once(" on ") {
                let agent = unquote_backticks(who);
                let dt = unquote_backticks(date);
                if !agent.is_empty() {
                    entry.proposed_by = agent;
                }
                if !dt.is_empty() {
                    entry.proposed_date = dt;
                }
            } else {
                let agent = unquote_backticks(val);
                if !agent.is_empty() {
                    entry.proposed_by = agent;
                }
            }
        }
        "supersedes" | "supersedes_amu_id" => {
            if let Ok(id) = AmuId::parse(&unquote_backticks(val)) {
                entry.supersedes_amu_id = Some(id);
            }
        }
        _ => {}
    }
}

fn unquote_backticks(raw: &str) -> String {
    raw.trim().trim_matches('`').trim().to_string()
}

/// Renders `Aether/Inbox.md` in the PRD Section 7.3 format.
pub(crate) fn render_inbox(entries: &[InboxEntry]) -> String {
    let mut out = String::from(INBOX_HEADER);
    for entry in entries {
        out.push('\n');
        let mark = match entry.checkbox {
            InboxCheckbox::Pending => ' ',
            InboxCheckbox::Approved => 'x',
            InboxCheckbox::Rejected => '-',
        };
        let tier_upper = entry.tier.as_str().to_ascii_uppercase();
        out.push_str(&format!(
            "- [{mark}] **[{tier_upper} | {}]** {}\n",
            entry.scope,
            entry.proposed_fact.trim()
        ));
        out.push_str(&format!(
            "  - `amu_candidate_id`: `{}`\n",
            entry.candidate_id
        ));
        if let Some(target) = &entry.target_link {
            let trimmed = target
                .trim()
                .trim_start_matches("[[")
                .trim_end_matches("]]");
            if !trimmed.is_empty() {
                out.push_str(&format!("  - `target_link`: [[{trimmed}]]\n"));
            }
        }
        let date = if entry.proposed_date.trim().is_empty() {
            "unknown"
        } else {
            entry.proposed_date.trim()
        };
        out.push_str(&format!(
            "  - `proposed_by`: `{}` on `{date}`\n",
            entry.proposed_by.trim()
        ));
        if let Some(sup) = entry.supersedes_amu_id {
            out.push_str(&format!("  - `supersedes`: `{sup}`\n"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_and_parses_section_7_3_inbox_roundtrip() {
        let supersede = AmuId::parse("amu_0123456789abcdef0123456789abcdef").unwrap();
        let entry = InboxEntry {
            checkbox: InboxCheckbox::Pending,
            tier: VaultTier::Warm,
            scope: ScopeTag::new("engineering").unwrap(),
            proposed_fact:
                "Switched primary local vector store to LanceDB for zero-copy arrow reads.".into(),
            candidate_id: CandidateId::parse("cand_9f82a1").unwrap(),
            target_link: Some("Architecture/Storage-Engine".into()),
            proposed_by: "cursor-agent".into(),
            proposed_date: "2026-04-14".into(),
            supersedes_amu_id: Some(supersede),
        };

        let rendered = render_inbox(std::slice::from_ref(&entry));
        assert!(rendered.contains("# Aether Memory Inbox"));
        assert!(rendered.contains(
            "- [ ] **[WARM | engineering]** Switched primary local vector store to LanceDB for zero-copy arrow reads."
        ));
        assert!(rendered.contains("- `amu_candidate_id`: `cand_9f82a1`"));
        assert!(rendered.contains("- `target_link`: [[Architecture/Storage-Engine]]"));
        assert!(rendered.contains("- `proposed_by`: `cursor-agent` on `2026-04-14`"));

        let parsed = parse_inbox(&rendered);
        assert_eq!(parsed, vec![entry]);
    }

    #[test]
    fn parses_pending_approved_and_rejected_checkboxes() {
        let md = "\
# Aether Memory Inbox
> Check `[x]` to approve and compile into active vault memory. Delete or mark `[-]` to reject.

- [x] **[HOT | rust]** Always use TransactionBehavior::Immediate for SQLite writes.
  - `amu_candidate_id`: `cand_aaaaaa`
  - `target_link`: [[Storage/SQLite]]
  - `proposed_by`: `claude` on `2026-05-01`
- [-] **[COLD | global]** Deprecated note about old config.
  - `amu_candidate_id`: `cand_bbbbbb`
  - `proposed_by`: `cursor` on `2026-05-02`
- [ ] **[WARM | ops]** Keep WAL mode enabled.
  - `amu_candidate_id`: `cand_cccccc`
";
        let entries = parse_inbox(md);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].checkbox, InboxCheckbox::Approved);
        assert_eq!(entries[0].tier, VaultTier::Hot);
        assert_eq!(entries[0].scope.as_str(), "rust");
        assert_eq!(entries[0].target_link.as_deref(), Some("Storage/SQLite"));
        assert_eq!(entries[1].checkbox, InboxCheckbox::Rejected);
        assert_eq!(entries[1].tier, VaultTier::Cold);
        assert_eq!(entries[2].checkbox, InboxCheckbox::Pending);
    }

    #[test]
    fn atomic_write_file_writes_and_hashes_cleanly_without_leftover_tmp() {
        let dir = std::env::temp_dir().join(format!("aether_inbox_test_{}", Uuid::new_v4()));
        let target = dir.join("Aether").join("Inbox.md");
        let payload = b"# Aether Memory Inbox\ntest content\n";

        let hash = atomic_write_file(&target, payload).unwrap();
        assert_eq!(hash, ContentHash::compute(payload));
        assert_eq!(std::fs::read(&target).unwrap(), payload);

        let leftovers: Vec<_> = std::fs::read_dir(target.parent().unwrap())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with(".tmp."))
            .collect();
        assert!(leftovers.is_empty());

        let _ = std::fs::remove_dir_all(dir);
    }
}
