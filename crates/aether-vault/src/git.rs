use std::collections::BTreeMap;
use std::path::Path;
use std::process::{Command, Output};

use nytheris_aether_core::{redact_url_userinfo, AetherError, Result};
use serde::{Deserialize, Serialize};

use crate::inbox::{self, APPROVED_MIRROR_REL_PATH, INBOX_REL_PATH};
use crate::parser::{detect_opening_fence, is_closing_fence};
use crate::types::{
    AmuId, ApprovedMirrorEntry, CandidateId, InboxCheckbox, InboxEntry, SourcePointer, SyncReport,
    VaultPath,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitSyncOptions {
    pub remote: String,
    pub branch: Option<String>,
    pub push: bool,
    pub commit_message: Option<String>,
}

impl Default for GitSyncOptions {
    fn default() -> Self {
        Self::new()
    }
}

impl GitSyncOptions {
    pub fn new() -> Self {
        Self {
            remote: "origin".into(),
            branch: None,
            push: false,
            commit_message: None,
        }
    }

    pub fn with_remote(mut self, remote: impl Into<String>) -> Self {
        self.remote = remote.into();
        self
    }

    pub fn with_branch(mut self, branch: impl Into<String>) -> Self {
        self.branch = Some(branch.into());
        self
    }

    pub fn with_push(mut self, push: bool) -> Self {
        self.push = push;
        self
    }

    pub fn with_commit_message(mut self, msg: impl Into<String>) -> Self {
        self.commit_message = Some(msg.into());
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitSyncReport {
    pub remote: String,
    pub branch: String,
    pub head_commit: String,
    pub merged_cleanly: bool,
    pub aether_conflicts_resolved: usize,
    pub conflicted_files: Vec<VaultPath>,
    pub sync_report: SyncReport,
    pub pushed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum CandidateLatticeStatus {
    Pending = 0,
    Rejected = 1,
    Approved = 2,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LatticeCandidate {
    pub(crate) status: CandidateLatticeStatus,
    pub(crate) entry: InboxEntry,
    pub(crate) compiled_amu_id: Option<AmuId>,
}

pub(crate) fn has_unresolved_conflict_markers(markdown: &str) -> bool {
    let mut fence: Option<(u8, usize)> = None;
    let mut saw_open = false;
    let mut saw_mid = false;

    for raw_line in markdown.lines() {
        let trimmed = raw_line.trim_start();
        if let Some((fc, fl)) = fence {
            if is_closing_fence(trimmed, fc, fl) {
                fence = None;
            }
            continue;
        }
        if let Some(open) = detect_opening_fence(trimmed) {
            fence = Some(open);
            continue;
        }
        if !crate::inbox::is_conflict_marker_line(raw_line) {
            continue;
        }
        if raw_line.starts_with("<<<<<<<") {
            saw_open = true;
            saw_mid = false;
        } else if saw_open && raw_line.starts_with("=======") {
            saw_mid = true;
        } else if saw_open && saw_mid && raw_line.starts_with(">>>>>>>") {
            return true;
        }
    }

    false
}

pub(crate) fn merge_aether_lattice(
    db_pending: &[InboxEntry],
    db_approved: &[ApprovedMirrorEntry],
    db_rejected: &[InboxEntry],
    inbox_snapshots: &[&str],
    approved_snapshots: &[&str],
) -> BTreeMap<CandidateId, LatticeCandidate> {
    let mut merged: BTreeMap<CandidateId, LatticeCandidate> = BTreeMap::new();

    for entry in db_pending {
        join_candidate(
            &mut merged,
            CandidateLatticeStatus::Pending,
            entry.clone(),
            None,
        );
    }
    for entry in db_rejected {
        join_candidate(
            &mut merged,
            CandidateLatticeStatus::Rejected,
            entry.clone(),
            None,
        );
    }
    for app in db_approved {
        let entry = InboxEntry {
            checkbox: InboxCheckbox::Approved,
            tier: app.tier,
            scope: app.scope.clone(),
            proposed_fact: app.proposed_fact.clone(),
            candidate_id: app.candidate_id.clone(),
            target_link: app.target_link.clone(),
            proposed_by: app.proposed_by.clone(),
            proposed_date: app.proposed_date.clone(),
            supersedes_amu_id: app.supersedes_amu_id,
        };
        join_candidate(
            &mut merged,
            CandidateLatticeStatus::Approved,
            entry,
            Some(app.compiled_amu_id),
        );
    }

    for snap in inbox_snapshots {
        for entry in inbox::parse_inbox(snap) {
            let status = match entry.checkbox {
                InboxCheckbox::Pending => CandidateLatticeStatus::Pending,
                InboxCheckbox::Rejected => CandidateLatticeStatus::Rejected,
                InboxCheckbox::Approved => CandidateLatticeStatus::Approved,
            };
            join_candidate(&mut merged, status, entry, None);
        }
    }

    for snap in approved_snapshots {
        for app in inbox::parse_approved_mirror(snap) {
            let entry = InboxEntry {
                checkbox: InboxCheckbox::Approved,
                tier: app.tier,
                scope: app.scope,
                proposed_fact: app.proposed_fact,
                candidate_id: app.candidate_id,
                target_link: app.target_link,
                proposed_by: app.proposed_by,
                proposed_date: app.proposed_date,
                supersedes_amu_id: app.supersedes_amu_id,
            };
            join_candidate(
                &mut merged,
                CandidateLatticeStatus::Approved,
                entry,
                Some(app.compiled_amu_id),
            );
        }
    }

    merged
}

fn join_candidate(
    map: &mut BTreeMap<CandidateId, LatticeCandidate>,
    status: CandidateLatticeStatus,
    mut entry: InboxEntry,
    compiled_amu_id: Option<AmuId>,
) {
    entry.checkbox = match status {
        CandidateLatticeStatus::Pending => InboxCheckbox::Pending,
        CandidateLatticeStatus::Rejected => InboxCheckbox::Rejected,
        CandidateLatticeStatus::Approved => InboxCheckbox::Approved,
    };

    let compiled_amu_id = compiled_amu_id.or_else(|| {
        if status == CandidateLatticeStatus::Approved {
            let ptr_raw = format!("{APPROVED_MIRROR_REL_PATH}#{}", entry.candidate_id);
            SourcePointer::parse(&ptr_raw)
                .ok()
                .map(|p| AmuId::from_pointer(&p))
        } else {
            None
        }
    });

    match map.get_mut(&entry.candidate_id) {
        None => {
            map.insert(
                entry.candidate_id.clone(),
                LatticeCandidate {
                    status,
                    entry,
                    compiled_amu_id,
                },
            );
        }
        Some(existing) => {
            if status > existing.status {
                let merged_target = entry
                    .target_link
                    .take()
                    .or_else(|| existing.entry.target_link.take());
                let merged_sup = entry.supersedes_amu_id.or(existing.entry.supersedes_amu_id);
                let merged_date = if entry.proposed_date.trim().is_empty() {
                    existing.entry.proposed_date.clone()
                } else {
                    entry.proposed_date.clone()
                };
                entry.target_link = merged_target;
                entry.supersedes_amu_id = merged_sup;
                entry.proposed_date = merged_date;
                existing.status = status;
                existing.entry = entry;
                if compiled_amu_id.is_some() {
                    existing.compiled_amu_id = compiled_amu_id;
                }
            } else {
                if existing.entry.target_link.is_none() {
                    existing.entry.target_link = entry.target_link;
                }
                if existing.entry.supersedes_amu_id.is_none() {
                    existing.entry.supersedes_amu_id = entry.supersedes_amu_id;
                }
                if existing.entry.proposed_date.trim().is_empty()
                    && !entry.proposed_date.trim().is_empty()
                {
                    existing.entry.proposed_date = entry.proposed_date;
                }
                if existing.compiled_amu_id.is_none() {
                    existing.compiled_amu_id = compiled_amu_id;
                }
            }
        }
    }
}

pub(crate) struct GitMergeOutcome {
    pub(crate) branch: String,
    pub(crate) unmerged_paths: Vec<String>,
    pub(crate) inbox_stages: Vec<String>,
    pub(crate) approved_stages: Vec<String>,
    pub(crate) in_merge_state: bool,
}

pub(crate) fn ensure_aether_gitignored_blocking(vault_root: &Path) -> Result<bool> {
    let check = Command::new("git")
        .args(["check-ignore", "-q", ".aether/aether.sqlite"])
        .current_dir(vault_root)
        .output()
        .map_err(|err| permanent_git_err(format!("failed to invoke git check-ignore: {err}")))?;
    if check.status.success() {
        return Ok(false);
    }

    let gitignore_path = vault_root.join(".gitignore");
    let existing = std::fs::read_to_string(&gitignore_path).unwrap_or_default();
    if existing
        .lines()
        .any(|line| matches!(line.trim(), ".aether/" | ".aether" | ".aether/*"))
    {
        return Ok(false);
    }

    let mut updated = existing;
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    updated.push_str(".aether/\n");
    inbox::atomic_write_file(&gitignore_path, updated.as_bytes())?;
    Ok(true)
}

pub(crate) fn run_git_fetch_merge_blocking(
    vault_root: &Path,
    remote: &str,
    branch: Option<&str>,
) -> Result<GitMergeOutcome> {
    let remote_clean = remote.trim();
    if remote_clean.is_empty() {
        return Err(AetherError::InvalidInput(
            "git remote must not be empty".into(),
        ));
    }
    let safe_remote = redact_url_userinfo(remote_clean);

    let inside = run_git_cmd(vault_root, &["rev-parse", "--is-inside-work-tree"])?;
    if !inside.status.success() || String::from_utf8_lossy(&inside.stdout).trim() != "true" {
        return Err(AetherError::InvalidInput(format!(
            "vault root `{}` is not inside a git repository",
            vault_root.display()
        )));
    }

    let branch_name = match branch.map(str::trim).filter(|b| !b.is_empty()) {
        Some(b) => b.to_string(),
        None => {
            let out = run_git_checked(
                vault_root,
                &["rev-parse", "--abbrev-ref", "HEAD"],
                &safe_remote,
            )?;
            let resolved = out.trim().to_string();
            if resolved == "HEAD" || resolved.is_empty() {
                "main".to_string()
            } else {
                resolved
            }
        }
    };

    stage_and_commit_aether_files_if_dirty(vault_root)?;

    let fetch = run_git_cmd(vault_root, &["fetch", remote_clean, &branch_name])?;
    if !fetch.status.success() {
        let stderr = redact_url_userinfo(&String::from_utf8_lossy(&fetch.stderr));
        return Err(transient_git_err(format!(
            "git fetch `{safe_remote}` `{branch_name}` failed: {}",
            stderr.trim()
        )));
    }

    let tracking_ref = format!("{remote_clean}/{branch_name}");
    let remote_ref = format!("refs/remotes/{tracking_ref}");
    let merge_target = if run_git_cmd(
        vault_root,
        &["rev-parse", "--verify", "--quiet", &remote_ref],
    )?
    .status
    .success()
    {
        tracking_ref
    } else {
        "FETCH_HEAD".to_string()
    };

    let merge_out = run_git_cmd(vault_root, &["merge", "--no-edit", &merge_target])?;
    let unmerged_paths = list_unmerged_paths(vault_root)?;
    if !merge_out.status.success() && unmerged_paths.is_empty() {
        let stderr = redact_url_userinfo(&String::from_utf8_lossy(&merge_out.stderr));
        return Err(AetherError::Permanent {
            connector: "vault-git",
            message: format!("git merge `{merge_target}` failed: {}", stderr.trim()),
        });
    }

    let inbox_stages = collect_conflict_stages(vault_root, INBOX_REL_PATH);
    let approved_stages = collect_conflict_stages(vault_root, APPROVED_MIRROR_REL_PATH);
    let in_merge_state = run_git_cmd(
        vault_root,
        &["rev-parse", "--verify", "--quiet", "MERGE_HEAD"],
    )?
    .status
    .success();
    Ok(GitMergeOutcome {
        branch: branch_name,
        unmerged_paths,
        inbox_stages,
        approved_stages,
        in_merge_state,
    })
}

pub(crate) fn finalize_git_merge_and_push_blocking(
    vault_root: &Path,
    resolved_aether_paths: &[&str],
    commit_merge: bool,
    remote: &str,
    branch: &str,
    push: bool,
    commit_message: Option<&str>,
) -> Result<(String, bool)> {
    let safe_remote = redact_url_userinfo(remote);
    let existing_paths: Vec<&str> = resolved_aether_paths
        .iter()
        .copied()
        .filter(|rel| vault_root.join(rel).exists())
        .collect();
    if !existing_paths.is_empty() {
        let mut args = vec!["add", "--"];
        args.extend(existing_paths);
        run_git_checked(vault_root, &args, &safe_remote)?;
    }

    if commit_merge {
        let mut args = vec!["commit", "--no-edit"];
        if let Some(msg) = commit_message.map(str::trim).filter(|m| !m.is_empty()) {
            args.extend(["-m", msg]);
        }
        run_git_with_identity(vault_root, &args, &safe_remote)?;
    }

    let mut pushed = false;
    if push {
        let out = run_git_cmd(vault_root, &["push", remote, branch])?;
        if !out.status.success() {
            let stderr = redact_url_userinfo(&String::from_utf8_lossy(&out.stderr));
            return Err(transient_git_err(format!(
                "git push `{safe_remote}` `{branch}` failed: {}",
                stderr.trim()
            )));
        }
        pushed = true;
    }

    let head = run_git_checked(vault_root, &["rev-parse", "HEAD"], &safe_remote)?;
    Ok((head.trim().to_string(), pushed))
}

fn stage_and_commit_aether_files_if_dirty(vault_root: &Path) -> Result<()> {
    let candidates = [".gitignore", INBOX_REL_PATH, APPROVED_MIRROR_REL_PATH];
    let to_stage: Vec<&str> = candidates
        .into_iter()
        .filter(|rel| vault_root.join(rel).exists())
        .collect();
    if to_stage.is_empty() {
        return Ok(());
    }

    let mut status_args = vec!["status", "--porcelain", "--"];
    status_args.extend(to_stage.iter().copied());
    let status_out = run_git_checked(vault_root, &status_args, "local")?;
    if status_out.trim().is_empty() {
        return Ok(());
    }

    let mut add_args = vec!["add", "--"];
    add_args.extend(to_stage);
    run_git_checked(vault_root, &add_args, "local")?;

    let diff_cached = run_git_cmd(vault_root, &["diff", "--cached", "--quiet"])?;
    if !diff_cached.status.success() {
        run_git_with_identity(
            vault_root,
            &[
                "commit",
                "-m",
                "chore(aether): sync inbox and compiled memory",
            ],
            "local",
        )?;
    }
    Ok(())
}

fn list_unmerged_paths(vault_root: &Path) -> Result<Vec<String>> {
    let out = run_git_checked(
        vault_root,
        &["diff", "--name-only", "--diff-filter=U"],
        "local",
    )?;
    Ok(out
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

fn collect_conflict_stages(vault_root: &Path, rel_path: &str) -> Vec<String> {
    let mut stages = Vec::new();
    for stage in [2, 3] {
        let spec = format!(":{stage}:{rel_path}");
        if let Ok(out) = run_git_cmd(vault_root, &["show", &spec]) {
            if out.status.success() {
                stages.push(String::from_utf8_lossy(&out.stdout).into_owned());
            }
        }
    }
    stages
}

fn run_git_cmd(vault_root: &Path, args: &[&str]) -> Result<Output> {
    Command::new("git")
        .args(args)
        .current_dir(vault_root)
        .output()
        .map_err(|err| permanent_git_err(format!("failed to execute git {:?}: {err}", args)))
}

fn run_git_checked(vault_root: &Path, args: &[&str], safe_remote: &str) -> Result<String> {
    let out = run_git_cmd(vault_root, args)?;
    if !out.status.success() {
        let stderr = redact_url_userinfo(&String::from_utf8_lossy(&out.stderr));
        return Err(permanent_git_err(format!(
            "git {:?} on `{safe_remote}` failed: {}",
            args,
            stderr.trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn run_git_with_identity(vault_root: &Path, args: &[&str], safe_remote: &str) -> Result<String> {
    let has_name = run_git_cmd(vault_root, &["config", "--get", "user.name"])?
        .status
        .success();
    let has_email = run_git_cmd(vault_root, &["config", "--get", "user.email"])?
        .status
        .success();
    let mut full_args: Vec<&str> = Vec::new();
    if !has_name {
        full_args.extend(["-c", "user.name=Aether Sync"]);
    }
    if !has_email {
        full_args.extend(["-c", "user.email=aether-sync@local"]);
    }
    full_args.extend(args.iter().copied());
    run_git_checked(vault_root, &full_args, safe_remote)
}

fn permanent_git_err(message: impl Into<String>) -> AetherError {
    AetherError::Permanent {
        connector: "vault-git",
        message: message.into(),
    }
}

fn transient_git_err(message: impl Into<String>) -> AetherError {
    AetherError::Transient {
        connector: "vault-git",
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{InboxCheckbox, ScopeTag, SourcePointer, VaultTier};

    #[test]
    fn conflict_marker_detector_ignores_fenced_code_blocks_and_flags_real_conflicts() {
        let code_only = "\
# Git Docs
```markdown
<<<<<<< HEAD
example ours
=======
example theirs
>>>>>>> branch
```
";
        assert!(!has_unresolved_conflict_markers(code_only));

        let real_conflict = "\
# Spec
<<<<<<< HEAD
Use Redis.
=======
Use Valkey.
>>>>>>> origin/main
";
        assert!(has_unresolved_conflict_markers(real_conflict));
    }

    #[test]
    fn monotonic_lattice_converges_approved_gt_rejected_gt_pending() {
        let cid_app = CandidateId::parse("cand_111111aaaaaa").unwrap();
        let cid_rej = CandidateId::parse("cand_222222bbbbbb").unwrap();
        let cid_pend = CandidateId::parse("cand_333333cccccc").unwrap();

        let pending_app = InboxEntry {
            checkbox: InboxCheckbox::Pending,
            tier: VaultTier::Warm,
            scope: ScopeTag::new("engineering").unwrap(),
            proposed_fact: "Use WAL mode.".into(),
            candidate_id: cid_app.clone(),
            target_link: Some("Arch/DB".into()),
            proposed_by: "alice".into(),
            proposed_date: "2026-05-01".into(),
            supersedes_amu_id: None,
        };
        let rejected_entry = InboxEntry {
            checkbox: InboxCheckbox::Rejected,
            tier: VaultTier::Warm,
            scope: ScopeTag::new("engineering").unwrap(),
            proposed_fact: "Use MyISAM.".into(),
            candidate_id: cid_rej.clone(),
            target_link: None,
            proposed_by: "bob".into(),
            proposed_date: "2026-05-01".into(),
            supersedes_amu_id: None,
        };
        let pending_only = InboxEntry {
            checkbox: InboxCheckbox::Pending,
            tier: VaultTier::Hot,
            scope: ScopeTag::global(),
            proposed_fact: "Always run cargo fmt.".into(),
            candidate_id: cid_pend.clone(),
            target_link: None,
            proposed_by: "carol".into(),
            proposed_date: "2026-05-02".into(),
            supersedes_amu_id: None,
        };

        let inbox_conflict = format!(
            "# Aether Memory Inbox\n\n<<<<<<< HEAD\n- [ ] **[WARM | engineering]** Use WAL mode.\n  - `amu_candidate_id`: `{cid_app}`\n- [ ] **[WARM | engineering]** Use MyISAM.\n  - `amu_candidate_id`: `{cid_rej}`\n=======\n- [x] **[WARM | engineering]** Use WAL mode.\n  - `amu_candidate_id`: `{cid_app}`\n- [-] **[WARM | engineering]** Use MyISAM.\n  - `amu_candidate_id`: `{cid_rej}`\n- [ ] **[HOT | global]** Always run cargo fmt.\n  - `amu_candidate_id`: `{cid_pend}`\n>>>>>>> origin/main\n"
        );
        let ptr = SourcePointer::parse(format!("Aether/Compiled/Approved.md#{cid_app}")).unwrap();
        let approved_snapshot = format!(
            "# Aether Compiled Approved Memory\n\n## {cid_app}\n- `amu_id`: `{}`\n- `tier`: `warm`\n- `scope`: `engineering`\n- `target_link`: [[Arch/DB]]\n- `proposed_by`: `alice` on `2026-05-01`\n\nUse WAL mode.\n",
            AmuId::from_pointer(&ptr)
        );

        let merged = merge_aether_lattice(
            std::slice::from_ref(&pending_app),
            &[],
            std::slice::from_ref(&rejected_entry),
            &[inbox_conflict.as_str()],
            &[approved_snapshot.as_str()],
        );

        assert_eq!(merged[&cid_app].status, CandidateLatticeStatus::Approved);
        assert_eq!(merged[&cid_rej].status, CandidateLatticeStatus::Rejected);
        assert_eq!(merged[&cid_pend].status, CandidateLatticeStatus::Pending);
        let _ = pending_only;
    }
}
