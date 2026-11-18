use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chrono::Utc;
use nytheris_aether_core::{redact_url_userinfo, AetherError, Result};
use nytheris_aether_cortex::{
    graph_scores,
    recall::{self, Candidate, RRF_K},
    PersonalizedPageRank,
};
use tokio::sync::{watch, Mutex as AsyncMutex};
use tokio::task::JoinHandle;
use walkdir::WalkDir;

use crate::auth::{
    AgentId, AgentRole, ApiKeySecret, AuthError, IssueApiKeyRequest, IssuedApiKey, KeyId,
    ScopePolicy, ScopedApiKey,
};
use crate::dream;
use crate::embedder::{embed_amu_text, Embedder, HashedNgramEmbedder};
use crate::git::{self, GitSyncOptions, GitSyncReport};
use crate::inbox::{self, APPROVED_MIRROR_REL_PATH, INBOX_REL_PATH};
use crate::parser::{self, IgnoreRules, ParsedNote, ParsedSection};
use crate::prefix;
use crate::reranker::{CrossEncoderReranker, Reranker};
use crate::store::{to_memory_item, VaultStore};
use crate::types::{
    Amu, AmuId, AmuKind, AmuStatus, CandidateId, ContentHash, DreamReport, InboxCheckbox,
    InboxEntry, InboxReconcileReport, RecallEntry, RecallRequest, RecallResponse, SourcePointer,
    StageMemoryRequest, SyncReport, VaultConfig, VaultMeta, VaultPath, VaultStatusReport,
    VAULT_SCHEMA_VERSION,
};

/// Single-owner orchestrator for Obsidian vault compilation, hybrid recall,
/// atomic Inbox staging, and live source provenance checks.
pub struct VaultEngine {
    vault_root: PathBuf,
    config: VaultConfig,
    store: VaultStore,
    embedder: Arc<dyn Embedder>,
    reranker: Arc<dyn Reranker>,
    write_gate: Arc<AsyncMutex<()>>,
    shutdown_tx: watch::Sender<bool>,
    watcher_handle: Mutex<Option<JoinHandle<()>>>,
    reindex_handle: Mutex<Option<JoinHandle<Result<usize>>>>,
}

impl VaultEngine {
    /// Opens the SQLite WAL index, performs an initial sync, and starts the optional poller.
    pub async fn open(config: VaultConfig) -> Result<Self> {
        let vault_root = std::fs::canonicalize(&config.vault_root).map_err(|err| {
            AetherError::InvalidInput(format!(
                "vault_root `{}` inaccessible: {err}",
                config.vault_root.display()
            ))
        })?;
        let store = VaultStore::open(config.db_target.clone(), config.project_id.clone()).await?;
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        let embedder: Arc<dyn Embedder> = config
            .embedder_override
            .clone()
            .unwrap_or_else(|| Arc::new(HashedNgramEmbedder::default()));
        let reranker: Arc<dyn Reranker> = config
            .reranker_override
            .clone()
            .unwrap_or_else(|| Arc::new(CrossEncoderReranker::default()));
        tracing::debug!(
            embedder_id = embedder.id(),
            embed_dims = embedder.dimensions(),
            reranker_id = reranker.id(),
            "initialized vault engine"
        );
        let engine = Self {
            vault_root,
            config,
            store,
            embedder,
            reranker,
            write_gate: Arc::new(AsyncMutex::new(())),
            shutdown_tx,
            watcher_handle: Mutex::new(None),
            reindex_handle: Mutex::new(None),
        };

        let _ = engine.reconcile_embedder_meta().await?;
        engine.sync_vault().await?;
        engine.maybe_spawn_watcher(shutdown_rx)?;
        Ok(engine)
    }

    pub async fn vault_meta(&self) -> Result<Option<VaultMeta>> {
        self.store.get_vault_meta().await
    }

    pub async fn wait_for_reindex(&self) -> Result<usize> {
        let handle = self
            .reindex_handle
            .lock()
            .map_err(|_| AetherError::Permanent {
                connector: "vault-engine",
                message: "reindex mutex poisoned".into(),
            })?
            .take();
        match handle {
            Some(h) => h.await.map_err(|err| AetherError::Permanent {
                connector: "vault-engine",
                message: format!("reindex task join error: {err}"),
            })?,
            None => Ok(0),
        }
    }

    async fn reconcile_embedder_meta(&self) -> Result<usize> {
        let existing = self.store.get_vault_meta().await?;
        let active_count = self.store.count_active_amus().await?;
        let target_id = self.embedder.id();
        let target_dim = self.embedder.dimensions();

        let needs_reembed = match existing {
            None => {
                if active_count == 0 {
                    false
                } else {
                    self.store.active_embedding_dim().await? != Some(target_dim)
                }
            }
            Some(ref meta) => meta.embedder_id != target_id || meta.embedding_dim != target_dim,
        };

        if !needs_reembed {
            let needs_meta_write = existing.as_ref().is_none_or(|meta| {
                meta.schema_version != VAULT_SCHEMA_VERSION
                    || meta.embedder_id != target_id
                    || meta.embedding_dim != target_dim
            });
            if needs_meta_write {
                self.store
                    .set_vault_meta(&VaultMeta {
                        schema_version: VAULT_SCHEMA_VERSION,
                        embedder_id: target_id.to_string(),
                        embedding_dim: target_dim,
                        updated_at: Utc::now(),
                    })
                    .await?;
            }
            return Ok(0);
        }

        if active_count <= self.config.sync_reindex_threshold {
            return self
                .store
                .reembed_active_sync(Arc::clone(&self.embedder), VAULT_SCHEMA_VERSION)
                .await;
        }

        let store = self.store.clone();
        let embedder = Arc::clone(&self.embedder);
        let handle = tokio::spawn(async move {
            store
                .reembed_active_shadow_swap(embedder, VAULT_SCHEMA_VERSION)
                .await
        });
        let mut guard = self
            .reindex_handle
            .lock()
            .map_err(|_| AetherError::Permanent {
                connector: "vault-engine",
                message: "reindex mutex poisoned".into(),
            })?;
        if let Some(prev) = guard.take() {
            prev.abort();
        }
        *guard = Some(handle);
        Ok(0)
    }

    fn maybe_spawn_watcher(&self, mut shutdown_rx: watch::Receiver<bool>) -> Result<()> {
        let Some(interval) = self.config.watch_interval else {
            return Ok(());
        };
        let vault_root = self.vault_root.clone();
        let store = self.store.clone();
        let embedder = Arc::clone(&self.embedder);
        let write_gate = Arc::clone(&self.write_gate);
        let project_id = self.config.project_id.clone();

        let handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = shutdown_rx.changed() => break,
                    _ = tokio::time::sleep(interval) => {
                        let _guard = write_gate.lock().await;
                        if let Err(err) = sync_vault_inner(&vault_root, &store, embedder.as_ref(), &project_id).await {
                            tracing::warn!(error = %err, "background vault sync failed");
                        }
                    }
                }
            }
        });

        let mut guard = self
            .watcher_handle
            .lock()
            .map_err(|_| AetherError::Permanent {
                connector: "vault-engine",
                message: "watcher mutex poisoned".into(),
            })?;
        *guard = Some(handle);
        Ok(())
    }

    /// Scans the vault, reconciles `Aether/Inbox.md`, and updates changed Markdown files.
    pub async fn sync_vault(&self) -> Result<SyncReport> {
        let reindexed = self.reconcile_embedder_meta().await?;
        let _guard = self.write_gate.lock().await;
        let mut report = sync_vault_inner(
            &self.vault_root,
            &self.store,
            self.embedder.as_ref(),
            &self.config.project_id,
        )
        .await?;
        report.reindexed_amus = reindexed;
        Ok(report)
    }

    /// Processes a single filesystem path event, skipping self-originated writes via SHA-256 ledger.
    pub async fn handle_fs_event(&self, path: &Path) -> Result<bool> {
        let _guard = self.write_gate.lock().await;
        let rel_path = match self.relative_vault_path(path) {
            Ok(rel) => rel,
            Err(_) => return Ok(false),
        };

        if rel_path.eq_ignore_ascii_case(APPROVED_MIRROR_REL_PATH) {
            if let Ok(bytes) = tokio::fs::read(path).await {
                let hash = ContentHash::compute(&bytes);
                let _ = self
                    .store
                    .consume_self_write(APPROVED_MIRROR_REL_PATH, &hash)
                    .await?;
            }
            return Ok(false);
        }

        if rel_path.eq_ignore_ascii_case(INBOX_REL_PATH) {
            let Ok(bytes) = tokio::fs::read(path).await else {
                return Ok(false);
            };
            let hash = ContentHash::compute(&bytes);
            if self.store.consume_self_write(INBOX_REL_PATH, &hash).await? {
                return Ok(false);
            }
            let manifest = self.store.file_manifest().await?;
            if manifest.get(INBOX_REL_PATH) == Some(&hash) {
                return Ok(false);
            }
            let report =
                reconcile_inbox_inner(&self.vault_root, &self.store, self.embedder.as_ref())
                    .await?;
            return Ok(report.compiled > 0 || report.rejected > 0);
        }

        let rules = IgnoreRules::load(&self.vault_root);
        if rules.is_ignored(&rel_path) || !rel_path.to_ascii_lowercase().ends_with(".md") {
            return Ok(false);
        }

        let Ok(bytes) = tokio::fs::read(path).await else {
            return self.store.remove_file(&rel_path, None).await;
        };
        let hash = ContentHash::compute(&bytes);
        if self.store.consume_self_write(&rel_path, &hash).await? {
            return Ok(false);
        }
        let manifest = self.store.file_manifest().await?;
        if manifest.get(&rel_path) == Some(&hash) {
            return Ok(false);
        }

        let vault_path = VaultPath::parse(&rel_path)?;
        let raw_md = String::from_utf8(bytes).map_err(|err| {
            AetherError::InvalidInput(format!("note `{rel_path}` is not valid UTF-8: {err}"))
        })?;
        if git::has_unresolved_conflict_markers(&raw_md) {
            tracing::warn!(rel_path = %rel_path, "skipping note with unresolved git conflict markers");
            return Ok(false);
        }
        match parser::parse_note(&rel_path, &raw_md, Utc::now(), self.embedder.as_ref())? {
            ParsedNote::Private => {
                self.store.remove_file(&rel_path, Some(&hash)).await?;
                Ok(true)
            }
            ParsedNote::Public(sections) => {
                self.store
                    .commit_file_sections(&vault_path, &hash, &sections)
                    .await?;
                Ok(true)
            }
        }
    }

    fn relative_vault_path(&self, path: &Path) -> Result<String> {
        let candidate = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.vault_root.join(path)
        };
        let canonical = if candidate.exists() {
            std::fs::canonicalize(&candidate).map_err(|err| {
                AetherError::InvalidInput(format!("path `{}` error: {err}", path.display()))
            })?
        } else {
            candidate
        };
        let rel = canonical.strip_prefix(&self.vault_root).map_err(|_| {
            AetherError::InvalidInput(format!("path `{}` escapes vault root", path.display()))
        })?;
        Ok(rel
            .to_string_lossy()
            .replace('\\', "/")
            .trim_start_matches('/')
            .to_string())
    }

    /// Stages a proposed memory non-destructively into `Aether/Inbox.md`.
    pub async fn stage_memory(&self, req: StageMemoryRequest) -> Result<CandidateId> {
        if req.proposed_fact.trim().is_empty() {
            return Err(AetherError::InvalidInput(
                "proposed_fact must not be empty".into(),
            ));
        }
        let _guard = self.write_gate.lock().await;

        let target_link = match req.target_note {
            Some(raw) => {
                let cleaned = raw
                    .trim()
                    .trim_start_matches("[[")
                    .trim_end_matches("]]")
                    .trim()
                    .replace('\\', "/");
                if cleaned.is_empty() {
                    None
                } else {
                    let probe = if cleaned.to_ascii_lowercase().ends_with(".md") {
                        cleaned.clone()
                    } else {
                        format!("{cleaned}.md")
                    };
                    let _ = VaultPath::parse(&probe)?;
                    let rules = IgnoreRules::load(&self.vault_root);
                    if rules.is_ignored(&probe) {
                        return Err(AetherError::InvalidInput(format!(
                            "target_note `{cleaned}` is in an ignored vault path"
                        )));
                    }
                    Some(cleaned)
                }
            }
            None => None,
        };

        let _ =
            reconcile_inbox_inner(&self.vault_root, &self.store, self.embedder.as_ref()).await?;

        let candidate_id = CandidateId::from_proposal(
            &req.proposed_fact,
            target_link.as_deref(),
            req.suggested_tier,
        );
        let entry = InboxEntry {
            checkbox: InboxCheckbox::Pending,
            tier: req.suggested_tier,
            scope: req.scope,
            proposed_fact: req.proposed_fact.trim().to_string(),
            candidate_id: candidate_id.clone(),
            target_link,
            proposed_by: req.proposed_by,
            proposed_date: req.proposed_date,
            supersedes_amu_id: req.supersedes_amu_id,
        };

        self.store.stage_candidate(&entry).await?;
        let pending = self.store.list_pending_candidates().await?;
        write_inbox_atomically(&self.vault_root, &self.store, &pending).await?;
        Ok(candidate_id)
    }

    /// Compiles checked `[x]` entries and drops `[-]` entries in `Aether/Inbox.md`.
    pub async fn reconcile_inbox(&self) -> Result<InboxReconcileReport> {
        let _guard = self.write_gate.lock().await;
        reconcile_inbox_inner(&self.vault_root, &self.store, self.embedder.as_ref()).await
    }

    /// Runs hybrid FTS5, cosine vector, and `PersonalizedPageRank` recall fused via RRF.
    pub async fn recall(&self, req: RecallRequest) -> Result<RecallResponse> {
        let policy = match &req.scope {
            Some(s) => ScopePolicy::Allowed(vec![s.clone()]),
            None => ScopePolicy::Unrestricted,
        };
        self.recall_with_policy(&req, &policy).await
    }

    async fn recall_with_policy(
        &self,
        req: &RecallRequest,
        scope_policy: &ScopePolicy,
    ) -> Result<RecallResponse> {
        let query_trimmed = req.query.trim();
        if query_trimmed.is_empty() && !req.include_hot_profile {
            return Err(AetherError::InvalidInput(
                "recall query must not be empty when include_hot_profile is false".into(),
            ));
        }

        let query_vec = if query_trimmed.is_empty() {
            Vec::new()
        } else {
            self.embedder.embed(query_trimmed)
        };

        let bundle = self
            .store
            .recall_bundle(query_trimmed, &query_vec, scope_policy, 32)
            .await?;

        if query_trimmed.is_empty() {
            return Ok(prefix::format_recall_response(
                bundle.hot_amus,
                Vec::new(),
                self.config.hot_budget,
                req.max_tokens,
                req.include_hot_profile,
            ));
        }

        let ppr = PersonalizedPageRank::default();
        let entity_ranks = ppr.rank(&bundle.subgraph, &bundle.seed_entities);
        let mem_graph_scores = graph_scores(&bundle.subgraph, &entity_ranks);

        let mut graph_candidates: Vec<Candidate> = mem_graph_scores
            .into_iter()
            .filter_map(|(mem_id, score)| {
                bundle.amus_by_memory_id.get(&mem_id).map(|amu| Candidate {
                    item: to_memory_item(amu, &self.config.project_id),
                    score,
                })
            })
            .collect();
        graph_candidates.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then_with(|| a.item.id.cmp(&b.item.id))
        });

        let mut lists = Vec::new();
        if !bundle.fts_candidates.is_empty() {
            lists.push(bundle.fts_candidates);
        }
        if !bundle.vector_candidates.is_empty() {
            lists.push(bundle.vector_candidates);
        }
        if !graph_candidates.is_empty() {
            lists.push(graph_candidates);
        }

        let fused = recall::rrf(&lists, RRF_K);
        let mut ranked_entries: Vec<RecallEntry> = fused
            .into_iter()
            .filter_map(|cand| {
                bundle
                    .amus_by_memory_id
                    .get(&cand.item.id)
                    .map(|amu| RecallEntry {
                        amu: amu.clone(),
                        score: cand.score * amu.authority_weight,
                    })
            })
            .collect();
        ranked_entries.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then_with(|| a.amu.amu_id.cmp(&b.amu.amu_id))
        });
        self.reranker.rerank(query_trimmed, &mut ranked_entries);

        Ok(prefix::format_recall_response(
            bundle.hot_amus,
            ranked_entries,
            self.config.hot_budget,
            req.max_tokens,
            req.include_hot_profile,
        ))
    }

    /// Scans Daily/Journal/fleeting notes not yet recorded in `dream_watermarks`,
    /// extracts durable facts and temporal contradictions, and stages them in `Aether/Inbox.md`.
    pub async fn run_dream_pass(&self) -> Result<DreamReport> {
        let _guard = self.write_gate.lock().await;
        let _ =
            reconcile_inbox_inner(&self.vault_root, &self.store, self.embedder.as_ref()).await?;

        let rules = IgnoreRules::load(&self.vault_root);
        let scanned_files = scan_vault_markdown_files(&self.vault_root, rules).await?;
        let watermarks = self.store.dream_watermarks().await?;
        let active_amus = self.store.list_active_amus().await?;

        let mut report = DreamReport::default();
        let today = Utc::now().format("%Y-%m-%d").to_string();

        for (rel_posix, bytes) in scanned_files {
            let Ok(raw_md) = String::from_utf8(bytes.clone()) else {
                continue;
            };
            if !dream::is_daily_or_fleeting_note(&rel_posix, &raw_md) {
                continue;
            }
            let hash = ContentHash::compute(&bytes);
            if watermarks.get(&rel_posix) == Some(&hash) {
                report.daily_notes_skipped_unchanged += 1;
                continue;
            }

            report.daily_notes_scanned += 1;
            let candidates = dream::extract_dream_candidates(&rel_posix, &raw_md, &active_amus);
            let mut staged_for_note = 0usize;
            for cand in candidates {
                let candidate_id = CandidateId::from_proposal(
                    &cand.proposed_fact,
                    cand.target_note.as_deref(),
                    cand.suggested_tier,
                );
                if cand.supersedes_amu_id.is_some() {
                    report.contradictions_flagged += 1;
                }
                let entry = InboxEntry {
                    checkbox: InboxCheckbox::Pending,
                    tier: cand.suggested_tier,
                    scope: cand.scope,
                    proposed_fact: cand.proposed_fact,
                    candidate_id: candidate_id.clone(),
                    target_link: cand.target_note,
                    proposed_by: "dream-worker".into(),
                    proposed_date: today.clone(),
                    supersedes_amu_id: cand.supersedes_amu_id,
                };
                self.store.stage_candidate(&entry).await?;
                report.candidates_staged += 1;
                staged_for_note += 1;
                report.staged_candidate_ids.push(candidate_id);
            }
            self.store
                .record_dream_watermark(&rel_posix, &hash, staged_for_note)
                .await?;
        }

        if report.candidates_staged > 0 {
            let pending = self.store.list_pending_candidates().await?;
            write_inbox_atomically(&self.vault_root, &self.store, &pending).await?;
        }

        Ok(report)
    }

    /// Returns a live status snapshot for the vault engine, active embedder, HOT budget, and Inbox.
    pub async fn status(&self) -> Result<VaultStatusReport> {
        let active_amus = self.store.count_active_amus().await?;
        let pending = self.store.list_pending_candidates().await?;
        let approved = self.store.list_approved_candidates().await?;
        let meta = self.store.get_vault_meta().await?;
        let mut hot_req = RecallRequest::new("");
        hot_req.include_hot_profile = true;
        let hot_resp = self.recall(hot_req).await?;

        Ok(VaultStatusReport {
            status: "ok".to_string(),
            schema_version: meta
                .as_ref()
                .map_or(VAULT_SCHEMA_VERSION, |m| m.schema_version),
            embedder_id: self.embedder.id().to_string(),
            embedding_dim: self.embedder.dimensions(),
            active_amus,
            pending_inbox_count: pending.len(),
            approved_inbox_count: approved.len(),
            hot_tokens_used: hot_resp.static_prefix_tokens,
            hot_token_budget: self.config.hot_budget.get(),
            prefix_hash: hot_resp.prefix_hash,
        })
    }

    /// Lists currently pending candidates staged in `Aether/Inbox.md`.
    pub async fn pending_inbox(&self) -> Result<Vec<InboxEntry>> {
        self.store.list_pending_candidates().await
    }

    /// Approves (`[x]`) or rejects (`[-]`) a pending Inbox candidate by ID and immediately
    /// updates SQLite, `Aether/Compiled/Approved.md`, and `Aether/Inbox.md`.
    pub async fn decide_inbox_candidate(
        &self,
        candidate_id: &CandidateId,
        approve: bool,
    ) -> Result<InboxReconcileReport> {
        let _guard = self.write_gate.lock().await;
        let _ =
            reconcile_inbox_inner(&self.vault_root, &self.store, self.embedder.as_ref()).await?;

        let pending = self.store.list_pending_candidates().await?;
        let mut found = false;
        let mut updated_entries = Vec::with_capacity(pending.len());
        let mut compiled_amus = Vec::new();
        let mut remaining_pending = Vec::new();

        for mut entry in pending {
            if &entry.candidate_id == candidate_id {
                found = true;
                entry.checkbox = if approve {
                    InboxCheckbox::Approved
                } else {
                    InboxCheckbox::Rejected
                };
                if approve {
                    let (amu, emb) = build_compiled_amu(&entry, self.embedder.as_ref())?;
                    compiled_amus.push((entry.clone(), amu, emb));
                }
                updated_entries.push(entry);
            } else {
                remaining_pending.push(entry.clone());
                updated_entries.push(entry);
            }
        }

        if !found {
            return Err(AetherError::NotFound(format!(
                "pending candidate `{candidate_id}` not found"
            )));
        }

        let (compiled, rejected) = self
            .store
            .apply_inbox_decisions(&updated_entries, &compiled_amus)
            .await?;

        if compiled > 0 {
            let approved = self.store.list_approved_candidates().await?;
            if !approved.is_empty() {
                write_approved_mirror_atomically(&self.vault_root, &self.store, &approved).await?;
            }
        }

        write_inbox_atomically(&self.vault_root, &self.store, &remaining_pending).await?;

        Ok(InboxReconcileReport {
            compiled,
            rejected,
            remaining_pending: remaining_pending.len(),
        })
    }

    /// Returns the exact verbatim Markdown section for `raw_pointer`, enforcing traversal and privacy guards.
    pub async fn inspect_source(&self, raw_pointer: &str) -> Result<String> {
        self.inspect_source_inner(None, raw_pointer).await
    }

    async fn inspect_source_inner(
        &self,
        agent: Option<&ScopedApiKey>,
        raw_pointer: &str,
    ) -> Result<String> {
        let pointer = SourcePointer::parse(raw_pointer)?;
        let rules = IgnoreRules::load(&self.vault_root);
        if rules.is_ignored_for_inspect(pointer.relative_path()) {
            return Err(AetherError::InvalidInput(format!(
                "path `{}` is excluded by vault ignore rules",
                pointer.relative_path()
            )));
        }

        let joined = pointer.vault_path().join_on(&self.vault_root);
        let canonical = std::fs::canonicalize(&joined).map_err(|err| {
            AetherError::NotFound(format!(
                "source `{}` not found: {err}",
                pointer.relative_path()
            ))
        })?;
        if !canonical.starts_with(&self.vault_root) {
            return Err(AetherError::InvalidInput(format!(
                "source `{}` resolves outside vault root",
                pointer.relative_path()
            )));
        }

        let raw_md = tokio::fs::read_to_string(&canonical).await.map_err(|err| {
            AetherError::NotFound(format!(
                "failed to read `{}`: {err}",
                pointer.relative_path()
            ))
        })?;

        if let Some(scoped_agent) = agent {
            if pointer
                .relative_path()
                .eq_ignore_ascii_case(APPROVED_MIRROR_REL_PATH)
            {
                let entries = inbox::parse_approved_mirror(&raw_md);
                match pointer.heading_anchor() {
                    Some(anchor) => {
                        if let Some(entry) = entries
                            .iter()
                            .find(|e| e.candidate_id.as_str().eq_ignore_ascii_case(anchor))
                        {
                            if !scoped_agent
                                .can_read_resource_scopes(std::slice::from_ref(&entry.scope))
                            {
                                return Err(AuthError::ScopeDenied {
                                    agent_id: scoped_agent.agent_id.clone(),
                                    requested: entry.scope.clone(),
                                }
                                .into());
                            }
                        }
                    }
                    None => {
                        for entry in &entries {
                            if !scoped_agent
                                .can_read_resource_scopes(std::slice::from_ref(&entry.scope))
                            {
                                return Err(AuthError::ScopeDenied {
                                    agent_id: scoped_agent.agent_id.clone(),
                                    requested: entry.scope.clone(),
                                }
                                .into());
                            }
                        }
                    }
                }
            } else {
                if parser::is_private_note(&raw_md) {
                    return Err(AetherError::InvalidInput(
                        "cannot inspect source of a private note".into(),
                    ));
                }
                let scopes = parser::inspect_note_scopes(pointer.relative_path(), &raw_md);
                if !scoped_agent.can_read_resource_scopes(&scopes) {
                    return Err(AuthError::ScopeDenied {
                        agent_id: scoped_agent.agent_id.clone(),
                        requested: scopes[0].clone(),
                    }
                    .into());
                }
            }
        }

        parser::extract_verbatim_section(&raw_md, &pointer)
    }

    /// Issues a scoped API key persisted in SQLite (`agent_api_keys`) as SHA-256 only.
    pub async fn issue_api_key(&self, req: IssueApiKeyRequest) -> Result<IssuedApiKey> {
        let secret = ApiKeySecret::generate();
        let key_hash = secret.digest();
        let key = ScopedApiKey::new(
            KeyId::generate(),
            req.agent_id,
            req.role,
            req.scopes,
            Utc::now(),
            req.expires_at,
            None,
        )?;
        self.store.insert_api_key(&key, &key_hash).await?;
        Ok(IssuedApiKey { key, secret })
    }

    /// Ensures a bootstrap Admin key matching `secret` exists in SQLite (idempotent across restarts).
    pub async fn ensure_bootstrap_admin_key(
        &self,
        agent_id: AgentId,
        secret: &ApiKeySecret,
    ) -> Result<ScopedApiKey> {
        self.store
            .ensure_bootstrap_admin_key(&agent_id, &secret.digest())
            .await
    }

    /// Authenticates `secret` in constant time and verifies active status.
    pub async fn authenticate_api_key(
        &self,
        secret: &ApiKeySecret,
    ) -> std::result::Result<ScopedApiKey, AuthError> {
        self.store
            .authenticate_api_key(&secret.digest(), Utc::now())
            .await
    }

    /// Lists all issued API keys without exposing secret hashes.
    pub async fn list_api_keys(&self) -> Result<Vec<ScopedApiKey>> {
        self.store.list_api_keys().await
    }

    /// Revokes an existing API key by `KeyId`.
    pub async fn revoke_api_key(&self, key_id: &KeyId) -> Result<bool> {
        self.store.revoke_api_key(key_id, Utc::now()).await
    }

    /// Executes scoped recall intersecting `agent.scopes()` with `req.scope`.
    pub async fn recall_as(
        &self,
        agent: &ScopedApiKey,
        req: RecallRequest,
    ) -> Result<RecallResponse> {
        agent.require_role(AgentRole::Reader)?;
        let policy = if agent.has_wildcard_scope() && req.scope.is_none() {
            ScopePolicy::Unrestricted
        } else {
            let effective = agent.intersect_scopes(req.scope.as_ref())?;
            ScopePolicy::Allowed(effective)
        };
        self.recall_with_policy(&req, &policy).await
    }

    /// Executes scoped memory staging, auto-stamping `proposed_by` with `agent.agent_id`.
    pub async fn stage_memory_as(
        &self,
        agent: &ScopedApiKey,
        mut req: StageMemoryRequest,
    ) -> Result<CandidateId> {
        agent.require_role(AgentRole::Contributor)?;
        agent.authorize_write_scope(&req.scope)?;
        req.proposed_by = agent.agent_id.as_str().to_string();
        self.stage_memory(req).await
    }

    /// Executes scoped source inspection, denying cross-scope vault notes and `Approved.md` entries.
    pub async fn inspect_source_as(
        &self,
        agent: &ScopedApiKey,
        raw_pointer: &str,
    ) -> Result<String> {
        agent.require_role(AgentRole::Reader)?;
        self.inspect_source_inner(Some(agent), raw_pointer).await
    }

    /// Lists pending Inbox candidates readable by `agent`.
    pub async fn pending_inbox_as(&self, agent: &ScopedApiKey) -> Result<Vec<InboxEntry>> {
        agent.require_role(AgentRole::Reader)?;
        let pending = self.pending_inbox().await?;
        Ok(pending
            .into_iter()
            .filter(|entry| agent.can_read_resource_scopes(std::slice::from_ref(&entry.scope)))
            .collect())
    }

    /// Synchronizes a Git-backed team vault, resolving Aether Inbox/Approved conflicts via
    /// monotonic lattice merge and preserving prior active AMUs for conflicted human notes.
    pub async fn git_sync(&self, opts: GitSyncOptions) -> Result<GitSyncReport> {
        let _guard = self.write_gate.lock().await;
        let _ =
            reconcile_inbox_inner(&self.vault_root, &self.store, self.embedder.as_ref()).await?;

        let vault_root_ignore = self.vault_root.clone();
        tokio::task::spawn_blocking(move || {
            git::ensure_aether_gitignored_blocking(&vault_root_ignore)
        })
        .await
        .map_err(|err| AetherError::Permanent {
            connector: "vault-git",
            message: format!("gitignore task join error: {err}"),
        })??;

        let vault_root_fetch = self.vault_root.clone();
        let remote_fetch = opts.remote.clone();
        let branch_fetch = opts.branch.clone();
        let outcome = tokio::task::spawn_blocking(move || {
            git::run_git_fetch_merge_blocking(
                &vault_root_fetch,
                &remote_fetch,
                branch_fetch.as_deref(),
            )
        })
        .await
        .map_err(|err| AetherError::Permanent {
            connector: "vault-git",
            message: format!("git fetch/merge task join error: {err}"),
        })??;

        let aether_conflicts_resolved = reconcile_aether_lattice_inner(
            &self.vault_root,
            &self.store,
            self.embedder.as_ref(),
            &outcome,
        )
        .await?;

        let mut remaining_unmerged = Vec::new();
        let mut conflicted_files = Vec::new();
        for rel in &outcome.unmerged_paths {
            if rel.eq_ignore_ascii_case(INBOX_REL_PATH)
                || rel.eq_ignore_ascii_case(APPROVED_MIRROR_REL_PATH)
            {
                continue;
            }
            remaining_unmerged.push(rel.clone());
            if let Ok(vp) = VaultPath::parse(rel) {
                conflicted_files.push(vp);
            }
        }

        let commit_merge = outcome.in_merge_state && remaining_unmerged.is_empty();
        let do_push = opts.push && remaining_unmerged.is_empty();
        let vault_root_fin = self.vault_root.clone();
        let remote_fin = opts.remote.clone();
        let branch_fin = outcome.branch.clone();
        let msg_fin = opts.commit_message.clone();
        let (head_commit, pushed) = tokio::task::spawn_blocking(move || {
            git::finalize_git_merge_and_push_blocking(
                &vault_root_fin,
                &[INBOX_REL_PATH, APPROVED_MIRROR_REL_PATH],
                commit_merge,
                &remote_fin,
                &branch_fin,
                do_push,
                msg_fin.as_deref(),
            )
        })
        .await
        .map_err(|err| AetherError::Permanent {
            connector: "vault-git",
            message: format!("git finalize task join error: {err}"),
        })??;

        let sync_report = sync_vault_inner(
            &self.vault_root,
            &self.store,
            self.embedder.as_ref(),
            &self.config.project_id,
        )
        .await?;

        Ok(GitSyncReport {
            remote: redact_url_userinfo(&opts.remote),
            branch: outcome.branch,
            head_commit,
            merged_cleanly: remaining_unmerged.is_empty() && conflicted_files.is_empty(),
            aether_conflicts_resolved,
            conflicted_files,
            sync_report,
            pushed,
        })
    }

    /// Stops any active background polling loop and closes the SQLite connection.
    pub async fn shutdown(&self) -> Result<()> {
        let _ = self.shutdown_tx.send(true);
        let handle = self
            .watcher_handle
            .lock()
            .map_err(|_| AetherError::Permanent {
                connector: "vault-engine",
                message: "watcher mutex poisoned".into(),
            })?
            .take();
        if let Some(h) = handle {
            let _ = h.await;
        }
        let _ = self.wait_for_reindex().await?;
        self.store.close().await
    }
}

async fn sync_vault_inner(
    vault_root: &Path,
    store: &VaultStore,
    embedder: &dyn Embedder,
    _project_id: &nytheris_aether_core::ProjectId,
) -> Result<SyncReport> {
    let mut report = SyncReport::default();
    let manifest = store.file_manifest().await?;
    let inbox_has_conflicts = tokio::fs::read_to_string(vault_root.join(INBOX_REL_PATH))
        .await
        .ok()
        .is_some_and(|s| git::has_unresolved_conflict_markers(&s));
    let approved_path = vault_root.join(APPROVED_MIRROR_REL_PATH);
    let approved_changed_externally = if let Ok(bytes) = tokio::fs::read(&approved_path).await {
        let hash = ContentHash::compute(&bytes);
        let text = String::from_utf8_lossy(&bytes);
        git::has_unresolved_conflict_markers(&text)
            || (!store
                .consume_self_write(APPROVED_MIRROR_REL_PATH, &hash)
                .await?
                && manifest.get(APPROVED_MIRROR_REL_PATH) != Some(&hash))
    } else {
        false
    };
    if inbox_has_conflicts || approved_changed_externally {
        let empty_outcome = git::GitMergeOutcome {
            branch: String::new(),
            unmerged_paths: Vec::new(),
            inbox_stages: Vec::new(),
            approved_stages: Vec::new(),
            in_merge_state: false,
        };
        let _ = reconcile_aether_lattice_inner(vault_root, store, embedder, &empty_outcome).await?;
    }

    let inbox_rep = reconcile_inbox_inner(vault_root, store, embedder).await?;
    report.inbox_approved = inbox_rep.compiled;
    report.inbox_rejected = inbox_rep.rejected;

    let rules = IgnoreRules::load(vault_root);
    let manifest = store.file_manifest().await?;
    let scanned_files = scan_vault_markdown_files(vault_root, rules).await?;

    let mut seen_paths = HashSet::new();
    let mut pending_updates: Vec<(VaultPath, ContentHash, String)> = Vec::new();
    let mut note_stem_to_entity: HashMap<String, String> = HashMap::new();
    let mut ambiguous_basenames: HashSet<String> = HashSet::new();

    for (rel_posix, _) in &scanned_files {
        let lower_stem = rel_posix
            .to_ascii_lowercase()
            .strip_suffix(".md")
            .unwrap_or(&rel_posix.to_ascii_lowercase())
            .to_string();
        let full_entity = format!("note:{lower_stem}");
        note_stem_to_entity.insert(lower_stem.clone(), full_entity.clone());

        let base = lower_stem
            .rsplit('/')
            .next()
            .unwrap_or(&lower_stem)
            .to_string();
        if !ambiguous_basenames.contains(&base) {
            if let Some(prev) = note_stem_to_entity.get(&base) {
                if prev != &full_entity {
                    note_stem_to_entity.remove(&base);
                    ambiguous_basenames.insert(base);
                }
            } else {
                note_stem_to_entity.insert(base, full_entity);
            }
        }
    }

    for (rel_posix, bytes) in scanned_files {
        report.files_scanned += 1;
        seen_paths.insert(rel_posix.clone());
        let hash = ContentHash::compute(&bytes);

        if store.consume_self_write(&rel_posix, &hash).await? {
            report.files_skipped_self_write += 1;
            continue;
        }
        if manifest.get(&rel_posix) == Some(&hash) {
            report.files_skipped_unchanged += 1;
            continue;
        }

        let vault_path = VaultPath::parse(&rel_posix)?;
        let raw_md = match String::from_utf8(bytes) {
            Ok(s) => s,
            Err(_) => continue,
        };
        if git::has_unresolved_conflict_markers(&raw_md) {
            tracing::warn!(rel_path = %rel_posix, "skipping note with unresolved git conflict markers");
            continue;
        }
        pending_updates.push((vault_path, hash, raw_md));
    }

    for (vault_path, hash, raw_md) in pending_updates {
        match parser::parse_note(vault_path.as_str(), &raw_md, Utc::now(), embedder)? {
            ParsedNote::Private => {
                store.remove_file(vault_path.as_str(), Some(&hash)).await?;
                report.private_skipped += 1;
            }
            ParsedNote::Public(mut sections) => {
                resolve_wikilink_targets(&mut sections, &note_stem_to_entity);
                let count = store
                    .commit_file_sections(&vault_path, &hash, &sections)
                    .await?;
                report.files_updated += 1;
                report.amus_indexed += count;
            }
        }
    }

    for old_path in manifest.keys() {
        if old_path.eq_ignore_ascii_case(INBOX_REL_PATH)
            || old_path.eq_ignore_ascii_case(APPROVED_MIRROR_REL_PATH)
        {
            continue;
        }
        if !seen_paths.contains(old_path) && store.remove_file(old_path, None).await? {
            report.files_removed += 1;
        }
    }

    Ok(report)
}

async fn scan_vault_markdown_files(
    vault_root: &Path,
    rules: IgnoreRules,
) -> Result<Vec<(String, Vec<u8>)>> {
    let vault_root_owned = vault_root.to_path_buf();
    tokio::task::spawn_blocking(move || -> Vec<(String, Vec<u8>)> {
        let mut files = Vec::new();
        for entry in WalkDir::new(&vault_root_owned)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if !entry.file_type().is_file() {
                continue;
            }
            let Ok(canon) = std::fs::canonicalize(entry.path()) else {
                continue;
            };
            if !canon.starts_with(&vault_root_owned) {
                continue;
            }
            let Ok(rel) = entry.path().strip_prefix(&vault_root_owned) else {
                continue;
            };
            let rel_posix = rel.to_string_lossy().replace('\\', "/");
            if rules.is_ignored(&rel_posix) || !rel_posix.to_ascii_lowercase().ends_with(".md") {
                continue;
            }
            if let Ok(bytes) = std::fs::read(&canon) {
                files.push((rel_posix, bytes));
            }
        }
        files
    })
    .await
    .map_err(|err| AetherError::Permanent {
        connector: "vault-fs",
        message: format!("vault scan task error: {err}"),
    })
}

fn resolve_wikilink_targets(sections: &mut [ParsedSection], stem_map: &HashMap<String, String>) {
    for sec in sections {
        for edge in &mut sec.amu.graph_edges {
            if let Some(target_stem) = edge.strip_prefix("note:") {
                if let Some(resolved) = stem_map.get(target_stem) {
                    *edge = resolved.clone();
                }
            }
        }
    }
}

async fn reconcile_aether_lattice_inner(
    vault_root: &Path,
    store: &VaultStore,
    embedder: &dyn Embedder,
    outcome: &git::GitMergeOutcome,
) -> Result<usize> {
    let inbox_path = vault_root.join(INBOX_REL_PATH);
    let mirror_path = vault_root.join(APPROVED_MIRROR_REL_PATH);
    let disk_inbox = tokio::fs::read_to_string(&inbox_path).await.ok();
    let disk_approved = tokio::fs::read_to_string(&mirror_path).await.ok();

    let mut conflicts_resolved = 0usize;
    if outcome
        .unmerged_paths
        .iter()
        .any(|p| p.eq_ignore_ascii_case(INBOX_REL_PATH))
        || disk_inbox
            .as_deref()
            .is_some_and(git::has_unresolved_conflict_markers)
    {
        conflicts_resolved += 1;
    }
    if outcome
        .unmerged_paths
        .iter()
        .any(|p| p.eq_ignore_ascii_case(APPROVED_MIRROR_REL_PATH))
        || disk_approved
            .as_deref()
            .is_some_and(git::has_unresolved_conflict_markers)
    {
        conflicts_resolved += 1;
    }

    let db_pending = store.list_pending_candidates().await?;
    let db_approved = store.list_approved_candidates().await?;
    let db_rejected = store.list_rejected_candidates().await?;

    let mut inbox_snapshots: Vec<&str> = Vec::new();
    if let Some(ref s) = disk_inbox {
        inbox_snapshots.push(s.as_str());
    }
    for s in &outcome.inbox_stages {
        inbox_snapshots.push(s.as_str());
    }

    let mut approved_snapshots: Vec<&str> = Vec::new();
    if let Some(ref s) = disk_approved {
        approved_snapshots.push(s.as_str());
    }
    for s in &outcome.approved_stages {
        approved_snapshots.push(s.as_str());
    }

    let merged = git::merge_aether_lattice(
        &db_pending,
        &db_approved,
        &db_rejected,
        &inbox_snapshots,
        &approved_snapshots,
    );
    if merged.is_empty() {
        return Ok(conflicts_resolved);
    }

    let mut pending = Vec::new();
    let mut approved_compiled = Vec::new();
    let mut rejected = Vec::new();
    for item in merged.into_values() {
        match item.status {
            git::CandidateLatticeStatus::Pending => pending.push(item.entry),
            git::CandidateLatticeStatus::Rejected => rejected.push(item.entry),
            git::CandidateLatticeStatus::Approved => {
                let (amu, emb) = build_compiled_amu(&item.entry, embedder)?;
                approved_compiled.push((item.entry, amu, emb));
            }
        }
    }

    store
        .apply_lattice_state(&pending, &approved_compiled, &rejected)
        .await?;

    let mut all_approved = store.list_approved_candidates().await?;
    all_approved.sort_by(|a, b| a.candidate_id.cmp(&b.candidate_id));
    if !all_approved.is_empty() || mirror_path.exists() {
        write_approved_mirror_atomically(vault_root, store, &all_approved).await?;
    }

    let mut all_pending = store.list_pending_candidates().await?;
    all_pending.sort_by(|a, b| a.candidate_id.cmp(&b.candidate_id));
    if !all_pending.is_empty() || inbox_path.exists() {
        write_inbox_atomically(vault_root, store, &all_pending).await?;
    }

    Ok(conflicts_resolved)
}

async fn reconcile_inbox_inner(
    vault_root: &Path,
    store: &VaultStore,
    embedder: &dyn Embedder,
) -> Result<InboxReconcileReport> {
    let inbox_path = vault_root.join(INBOX_REL_PATH);
    let pending_in_db = store.list_pending_candidates().await?;

    if !inbox_path.exists() {
        if pending_in_db.is_empty() {
            return Ok(InboxReconcileReport::default());
        }
        write_inbox_atomically(vault_root, store, &pending_in_db).await?;
        return Ok(InboxReconcileReport {
            compiled: 0,
            rejected: 0,
            remaining_pending: pending_in_db.len(),
        });
    }

    let bytes = tokio::fs::read(&inbox_path)
        .await
        .map_err(|err| AetherError::Permanent {
            connector: "vault-fs",
            message: format!("failed to read `{}`: {err}", inbox_path.display()),
        })?;
    let hash = ContentHash::compute(&bytes);
    if store.consume_self_write(INBOX_REL_PATH, &hash).await? {
        return Ok(InboxReconcileReport {
            compiled: 0,
            rejected: 0,
            remaining_pending: pending_in_db.len(),
        });
    }

    let raw_inbox = String::from_utf8_lossy(&bytes);
    let parsed_entries = inbox::parse_inbox(&raw_inbox);

    let mut compiled_amus = Vec::new();
    let mut disk_ids = HashSet::new();
    let mut remaining_pending = Vec::new();

    for entry in &parsed_entries {
        disk_ids.insert(entry.candidate_id.as_str().to_string());
        match entry.checkbox {
            InboxCheckbox::Pending => remaining_pending.push(entry.clone()),
            InboxCheckbox::Approved => {
                let (amu, emb) = build_compiled_amu(entry, embedder)?;
                compiled_amus.push((entry.clone(), amu, emb));
            }
            InboxCheckbox::Rejected => {}
        }
    }

    for db_pending in pending_in_db {
        if !disk_ids.contains(db_pending.candidate_id.as_str()) {
            remaining_pending.push(db_pending);
        }
    }

    let (compiled, rejected) = store
        .apply_inbox_decisions(&parsed_entries, &compiled_amus)
        .await?;

    let mirror_path = vault_root.join(APPROVED_MIRROR_REL_PATH);
    if compiled > 0 || !mirror_path.exists() {
        let approved = store.list_approved_candidates().await?;
        if !approved.is_empty() {
            write_approved_mirror_atomically(vault_root, store, &approved).await?;
        }
    }

    if compiled > 0 || rejected > 0 || remaining_pending.len() != parsed_entries.len() {
        write_inbox_atomically(vault_root, store, &remaining_pending).await?;
    } else {
        store.update_file_watermark(INBOX_REL_PATH, &hash).await?;
    }

    Ok(InboxReconcileReport {
        compiled,
        rejected,
        remaining_pending: remaining_pending.len(),
    })
}

fn build_compiled_amu(entry: &InboxEntry, embedder: &dyn Embedder) -> Result<(Amu, Vec<f32>)> {
    let pointer_raw = format!("{APPROVED_MIRROR_REL_PATH}#{}", entry.candidate_id);
    let source_pointer = SourcePointer::parse(&pointer_raw)?;
    let amu_id = AmuId::from_pointer(&source_pointer);
    let heading_path = source_pointer.as_str().to_string();
    let distilled_fact = entry.proposed_fact.trim().to_string();
    let embedding = embedder.embed(&embed_amu_text(&heading_path, &distilled_fact));

    let mut graph_edges = Vec::new();
    if let Some(link) = &entry.target_link {
        let stem = link
            .trim()
            .trim_start_matches("[[")
            .trim_end_matches("]]")
            .replace('\\', "/")
            .to_ascii_lowercase();
        let stem = stem.strip_suffix(".md").unwrap_or(&stem);
        if !stem.is_empty() {
            graph_edges.push(format!("note:{stem}"));
        }
    }

    let amu = Amu {
        amu_id,
        tier: entry.tier,
        kind: AmuKind::Decision,
        status: AmuStatus::Active,
        heading_path,
        distilled_fact: distilled_fact.clone(),
        source_pointer,
        graph_edges,
        valid_from: Utc::now(),
        superseded_by: None,
        scopes: vec![entry.scope.clone()],
        authority_weight: 1.0,
        content_hash: ContentHash::compute(distilled_fact.as_bytes()),
    };
    Ok((amu, embedding))
}

async fn write_approved_mirror_atomically(
    vault_root: &Path,
    store: &VaultStore,
    entries: &[crate::types::ApprovedMirrorEntry],
) -> Result<()> {
    let mirror_path = vault_root.join(APPROVED_MIRROR_REL_PATH);
    let rendered = inbox::render_approved_mirror(entries);
    let hash = ContentHash::compute(rendered.as_bytes());
    store
        .record_self_write(APPROVED_MIRROR_REL_PATH, &hash)
        .await?;
    inbox::atomic_write_file(&mirror_path, rendered.as_bytes())?;
    Ok(())
}

async fn write_inbox_atomically(
    vault_root: &Path,
    store: &VaultStore,
    entries: &[InboxEntry],
) -> Result<()> {
    let inbox_path = vault_root.join(INBOX_REL_PATH);
    let rendered = inbox::render_inbox(entries);
    let hash = ContentHash::compute(rendered.as_bytes());
    store.record_self_write(INBOX_REL_PATH, &hash).await?;
    inbox::atomic_write_file(&inbox_path, rendered.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use uuid::Uuid;

    use super::*;
    use crate::types::{ScopeTag, TokenBudget, VaultTier};

    struct TempVault {
        root: PathBuf,
    }

    impl TempVault {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("aether_vault_test_{}", Uuid::new_v4()));
            fs::create_dir_all(&root).unwrap();
            Self { root }
        }

        fn write(&self, rel: &str, content: &str) {
            let full = self.root.join(rel);
            if let Some(parent) = full.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(full, content).unwrap();
        }
    }

    impl Drop for TempVault {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[tokio::test]
    async fn sync_excludes_private_and_ignored_notes_and_inspect_guards_source() {
        let vault = TempVault::new();
        vault.write(".aetherignore", "SecretFolder/\n");
        vault.write("SecretFolder/Keys.md", "# Keys\nSECRET_API_KEY=12345\n");
        vault.write(
            "Personal/Diary.md",
            "---\naether_tier: private\n---\n# Thoughts\nPrivate diary entry.\n",
        );
        vault.write(
            "Arch/DB.md",
            "---\naether_tier: warm\naether_scope:\n  - engineering\n---\n# Schema\nUse `--port 5432` and `PRAGMA journal_mode = WAL;`.\n",
        );

        let engine = VaultEngine::open(VaultConfig::in_memory(&vault.root))
            .await
            .unwrap();

        let verbatim = engine.inspect_source("Arch/DB.md#Schema").await.unwrap();
        assert!(verbatim.contains("--port 5432"));
        assert!(verbatim.contains("PRAGMA journal_mode = WAL;"));

        assert!(matches!(
            engine.inspect_source("../../etc/passwd").await,
            Err(AetherError::InvalidInput(_))
        ));
        assert!(matches!(
            engine.inspect_source("Personal/Diary.md#Thoughts").await,
            Err(AetherError::InvalidInput(_))
        ));
        assert!(matches!(
            engine.inspect_source("SecretFolder/Keys.md#Keys").await,
            Err(AetherError::InvalidInput(_))
        ));

        let mut req = RecallRequest::new("SECRET_API_KEY diary");
        req.scope = Some(ScopeTag::new("engineering").unwrap());
        let res = engine.recall(req).await.unwrap();
        assert!(!res.formatted_context.contains("SECRET_API_KEY"));
        assert!(!res.formatted_context.contains("Private diary"));

        engine.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn stage_memory_prevents_self_write_watcher_loops_and_reconciles_approvals() {
        let vault = TempVault::new();
        vault.write(
            "Arch/Vector.md",
            "---\naether_tier: warm\naether_scope:\n  - engineering\n---\n# Engine\nOriginally used Qdrant for local vector search.\n",
        );

        let engine = VaultEngine::open(VaultConfig::in_memory(&vault.root))
            .await
            .unwrap();

        let old_ptr = SourcePointer::parse("Arch/Vector.md#Engine").unwrap();
        let old_amu = AmuId::from_pointer(&old_ptr);

        let cand_id = engine
            .stage_memory(
                StageMemoryRequest::new(
                    "Switched primary local vector store to LanceDB for zero-copy arrow reads.",
                    VaultTier::Warm,
                )
                .with_scope("engineering")
                .unwrap()
                .with_target_note("Architecture/Storage-Engine")
                .with_supersedes(old_amu),
            )
            .await
            .unwrap();

        let inbox_path = vault.root.join("Aether/Inbox.md");
        assert!(inbox_path.exists());

        assert!(!engine.handle_fs_event(&inbox_path).await.unwrap());

        let inbox_text = fs::read_to_string(&inbox_path).unwrap();
        let approved_text = inbox_text.replace("- [ ] ", "- [x] ");
        fs::write(&inbox_path, approved_text).unwrap();

        assert!(engine.handle_fs_event(&inbox_path).await.unwrap());
        assert!(!engine.handle_fs_event(&inbox_path).await.unwrap());

        let mut req = RecallRequest::new("primary local vector store LanceDB Qdrant");
        req.scope = Some(ScopeTag::new("engineering").unwrap());
        let res = engine.recall(req).await.unwrap();
        assert!(res.formatted_context.contains("LanceDB"));
        assert!(!res.formatted_context.contains("Originally used Qdrant"));
        let _ = cand_id;

        engine.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn hybrid_recall_enforces_scope_hot_prefix_hash_and_graph_ppr() {
        let vault = TempVault::new();
        vault.write(
            "Profile/Identity.md",
            "---\naether_tier: hot\naether_scope:\n  - global\naether_kind: profile\n---\n# Identity\nAlways respond concisely in Rust.\n",
        );
        vault.write(
            "Specs/Hub.md",
            "---\naether_tier: warm\naether_scope:\n  - backend\n---\n# Hub\nSee [[Specs/Spoke]] for PostgreSQL connection pooling.\n",
        );
        vault.write(
            "Specs/Spoke.md",
            "---\naether_tier: warm\naether_scope:\n  - backend\n---\n# Spoke\nPgBouncer transaction pooling uses max 64 connections.\n",
        );
        vault.write(
            "Frontend/Theme.md",
            "---\naether_tier: warm\naether_scope:\n  - frontend\n---\n# Theme\nUse Tailwind CSS tokens and connection pooling UI colors.\n",
        );

        let engine = VaultEngine::open(VaultConfig::in_memory(&vault.root))
            .await
            .unwrap();

        let mut cold_start = RecallRequest::new("");
        cold_start.include_hot_profile = true;
        let cold_res = engine.recall(cold_start).await.unwrap();
        assert!(cold_res
            .hot_prefix
            .contains("Always respond concisely in Rust."));
        assert!(cold_res.dynamic_context.is_empty());

        let mut q1 = RecallRequest::new("PgBouncer connection pooling");
        q1.scope = Some(ScopeTag::new("backend").unwrap());
        q1.max_tokens = TokenBudget::new(300).unwrap();
        let r1 = engine.recall(q1).await.unwrap();

        let mut q2 = RecallRequest::new("Rust PostgreSQL");
        q2.scope = Some(ScopeTag::new("backend").unwrap());
        q2.max_tokens = TokenBudget::new(300).unwrap();
        let r2 = engine.recall(q2).await.unwrap();

        assert_eq!(r1.prefix_hash, r2.prefix_hash);
        assert!(r1.tokens_used <= 300);
        assert!(r1.formatted_context.contains("PgBouncer"));
        assert!(!r1.formatted_context.contains("Tailwind CSS"));

        engine.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn inbox_rejection_and_note_deletion_remove_or_skip_memories() {
        let vault = TempVault::new();
        vault.write(
            "Notes/Temp.md",
            "---\naether_tier: warm\naether_scope:\n  - ops\n---\n# Temp\nEphemeral operational note about Redis cluster.\n",
        );

        let engine = VaultEngine::open(VaultConfig::in_memory(&vault.root))
            .await
            .unwrap();

        let _cand = engine
            .stage_memory(
                StageMemoryRequest::new("Rejected memory proposal.", VaultTier::Warm)
                    .with_scope("ops")
                    .unwrap(),
            )
            .await
            .unwrap();

        let inbox_path = vault.root.join("Aether/Inbox.md");
        let text = fs::read_to_string(&inbox_path).unwrap();
        fs::write(&inbox_path, text.replace("- [ ] ", "- [-] ")).unwrap();

        let rep = engine.reconcile_inbox().await.unwrap();
        assert_eq!(rep.compiled, 0);
        assert_eq!(rep.rejected, 1);
        assert_eq!(rep.remaining_pending, 0);

        fs::remove_file(vault.root.join("Notes/Temp.md")).unwrap();
        let sync_rep = engine.sync_vault().await.unwrap();
        assert_eq!(sync_rep.files_removed, 1);

        let mut req = RecallRequest::new("Redis cluster Rejected memory");
        req.scope = Some(ScopeTag::new("ops").unwrap());
        let res = engine.recall(req).await.unwrap();
        assert!(!res.formatted_context.contains("Ephemeral operational"));
        assert!(!res.formatted_context.contains("Rejected memory proposal"));

        engine.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn vault_meta_sync_reindexes_when_embedder_dimension_changes() {
        let vault = TempVault::new();
        vault.write(
            "Specs/Vector.md",
            "---\naether_tier: warm\naether_scope:\n  - engineering\n---\n# Indexing\nStore embeddings in SQLite WAL.\n",
        );

        let engine256 = VaultEngine::open(VaultConfig::new(&vault.root))
            .await
            .unwrap();
        let meta256 = engine256.vault_meta().await.unwrap().unwrap();
        assert_eq!(meta256.schema_version, VAULT_SCHEMA_VERSION);
        assert_eq!(meta256.embedder_id, "hashed-ngram-256-v1");
        assert_eq!(meta256.embedding_dim, 256);
        engine256.shutdown().await.unwrap();

        let embedder384: Arc<dyn Embedder> = Arc::new(HashedNgramEmbedder::with_dimensions(384));
        let engine384 = VaultEngine::open(VaultConfig::new(&vault.root).with_embedder(embedder384))
            .await
            .unwrap();
        let meta384 = engine384.vault_meta().await.unwrap().unwrap();
        assert_eq!(meta384.embedder_id, "hashed-ngram-384-v1");
        assert_eq!(meta384.embedding_dim, 384);
        assert_eq!(
            engine384.store.active_embedding_dim().await.unwrap(),
            Some(384)
        );

        let mut req = RecallRequest::new("embeddings SQLite WAL");
        req.scope = Some(ScopeTag::new("engineering").unwrap());
        let recalled = engine384.recall(req).await.unwrap();
        assert!(recalled
            .formatted_context
            .contains("Store embeddings in SQLite WAL."));

        engine384.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn vault_meta_shadow_swap_reindexes_above_threshold_while_keeping_recall_online() {
        let vault = TempVault::new();
        vault.write(
            "Specs/One.md",
            "---\naether_tier: warm\n---\n# Alpha\nKafka streaming pipeline specification.\n",
        );
        vault.write(
            "Specs/Two.md",
            "---\naether_tier: warm\n---\n# Beta\nPostgres WAL checkpoint tuning.\n",
        );
        vault.write(
            "Specs/Three.md",
            "---\naether_tier: warm\n---\n# Gamma\nRedis sentinel failover timing.\n",
        );

        let engine256 = VaultEngine::open(VaultConfig::new(&vault.root))
            .await
            .unwrap();
        engine256.shutdown().await.unwrap();

        let embedder384: Arc<dyn Embedder> = Arc::new(HashedNgramEmbedder::with_dimensions(384));
        let engine384 = VaultEngine::open(
            VaultConfig::new(&vault.root)
                .with_sync_reindex_threshold(1)
                .with_embedder(embedder384),
        )
        .await
        .unwrap();

        // Recall must stay online via FTS5 + graph RRF even before waiting for background swap.
        let online_recall = engine384
            .recall(RecallRequest::new("Kafka streaming pipeline"))
            .await
            .unwrap();
        assert!(online_recall
            .formatted_context
            .contains("Kafka streaming pipeline"));

        let swapped = engine384.wait_for_reindex().await.unwrap();
        assert_eq!(swapped, 3);
        let meta384 = engine384.vault_meta().await.unwrap().unwrap();
        assert_eq!(meta384.embedding_dim, 384);
        assert_eq!(engine384.store.shadow_table_row_count().await.unwrap(), 0);
        assert_eq!(
            engine384.store.active_embedding_dim().await.unwrap(),
            Some(384)
        );

        engine384.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn dense_subword_embedder_auto_reindexes_and_reranker_scores_accurately() {
        use crate::embedder::DenseSubwordEmbedder;

        let vault = TempVault::new();
        vault.write(
            "Specs/General.md",
            "---\naether_tier: warm\naether_scope:\n  - engineering\n---\n# General\nPrefix notes and cache comments.\n",
        );
        vault.write(
            "Specs/Cache.md",
            "---\naether_tier: warm\naether_scope:\n  - engineering\n---\n# Prefix Cache\nPrefix cache canonicalization removes volatile timestamps.\n",
        );

        let engine256 = VaultEngine::open(VaultConfig::new(&vault.root))
            .await
            .unwrap();
        engine256.shutdown().await.unwrap();

        let dense: Arc<dyn Embedder> = Arc::new(DenseSubwordEmbedder::default());
        let engine384 = VaultEngine::open(VaultConfig::new(&vault.root).with_embedder(dense))
            .await
            .unwrap();
        let meta = engine384.vault_meta().await.unwrap().unwrap();
        assert_eq!(meta.embedder_id, "dense-subword-384-v1");
        assert_eq!(meta.embedding_dim, 384);

        let mut req = RecallRequest::new("prefix cache");
        req.scope = Some(ScopeTag::new("engineering").unwrap());
        let resp = engine384.recall(req).await.unwrap();
        assert_eq!(
            resp.warm_cold_amus[0].source_pointer.as_str(),
            "Specs/Cache.md#Prefix-Cache"
        );

        engine384.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn dream_pass_extracts_contradictions_tracks_watermarks_and_supersedes_on_approval() {
        let vault = TempVault::new();
        vault.write(
            "Arch/Vector.md",
            "---\naether_tier: warm\naether_scope:\n  - engineering\n---\n# Engine\nUse Qdrant for local vector search.\n",
        );
        vault.write(
            "Daily/2026-05-10.md",
            "---\naether_scope: engineering\n---\n# Log\n## Decisions\n- Decision: Switched local vector search from Qdrant to LanceDB in [[Arch/Vector]].\n",
        );

        let engine = VaultEngine::open(VaultConfig::in_memory(&vault.root))
            .await
            .unwrap();

        let pass1 = engine.run_dream_pass().await.unwrap();
        assert_eq!(pass1.daily_notes_scanned, 1);
        assert_eq!(pass1.daily_notes_skipped_unchanged, 0);
        assert_eq!(pass1.candidates_staged, 1);
        assert_eq!(pass1.contradictions_flagged, 1);

        let inbox_path = vault.root.join("Aether/Inbox.md");
        let inbox_text = fs::read_to_string(&inbox_path).unwrap();
        assert!(inbox_text.contains("LanceDB"));
        assert!(inbox_text.contains("supersedes"));

        // Second pass skips unchanged daily note via dream_watermarks.
        let pass2 = engine.run_dream_pass().await.unwrap();
        assert_eq!(pass2.daily_notes_scanned, 0);
        assert_eq!(pass2.daily_notes_skipped_unchanged, 1);
        assert_eq!(pass2.candidates_staged, 0);

        // Approve the Dream-staged candidate -> supersedes old Qdrant AMU and writes Approved.md.
        fs::write(&inbox_path, inbox_text.replace("- [ ] ", "- [x] ")).unwrap();
        let rep = engine.reconcile_inbox().await.unwrap();
        assert_eq!(rep.compiled, 1);
        assert!(vault.root.join("Aether/Compiled/Approved.md").exists());

        let mut req = RecallRequest::new("local vector search Qdrant LanceDB");
        req.scope = Some(ScopeTag::new("engineering").unwrap());
        let res = engine.recall(req).await.unwrap();
        assert!(res.formatted_context.contains("LanceDB"));
        assert!(!res
            .formatted_context
            .contains("Use Qdrant for local vector search."));

        engine.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn scoped_api_keys_enforce_recall_stage_inspect_and_revocation() {
        let vault = TempVault::new();
        vault.write(
            "Engineering/DB.md",
            "---\naether_tier: warm\naether_scope:\n  - engineering\n---\n# DB\nUse WAL mode in SQLite.\n",
        );
        vault.write(
            "Ops/Runbook.md",
            "---\naether_tier: warm\naether_scope:\n  - ops\n---\n# Failover\nSentinel quorum is 2.\n",
        );
        vault.write(
            "HR/Compensation.md",
            "---\naether_tier: warm\naether_scope:\n  - hr\n---\n# Bands\nExecutive compensation bands.\n",
        );

        let engine = VaultEngine::open(VaultConfig::in_memory(&vault.root))
            .await
            .unwrap();

        let issued_eng_ops = engine
            .issue_api_key(
                IssueApiKeyRequest::new(
                    AgentId::new("ci-platform").unwrap(),
                    AgentRole::Contributor,
                    vec![
                        ScopeTag::new("engineering").unwrap(),
                        ScopeTag::new("ops").unwrap(),
                    ],
                )
                .unwrap(),
            )
            .await
            .unwrap();

        let parsed_secret = ApiKeySecret::parse(issued_eng_ops.secret.expose_secret()).unwrap();
        let authed = engine.authenticate_api_key(&parsed_secret).await.unwrap();
        assert_eq!(authed.key_id, issued_eng_ops.key.key_id);

        // Multi-scope recall without explicit `req.scope` fetches both `engineering` and `ops`,
        // excluding `hr` before top-K truncation.
        let multi_res = engine
            .recall_as(&authed, RecallRequest::new("SQLite Sentinel Executive"))
            .await
            .unwrap();
        assert!(multi_res.formatted_context.contains("WAL mode"));
        assert!(multi_res.formatted_context.contains("Sentinel quorum"));
        assert!(!multi_res
            .formatted_context
            .contains("Executive compensation"));

        // Requesting unauthorized `hr` scope fails.
        let mut bad_req = RecallRequest::new("Executive");
        bad_req.scope = Some(ScopeTag::new("hr").unwrap());
        assert!(engine.recall_as(&authed, bad_req).await.is_err());

        // Scoped stage auto-stamps `proposed_by` and prevents staging into `hr`.
        assert!(engine
            .stage_memory_as(
                &authed,
                StageMemoryRequest::new("Unauthorized HR claim.", VaultTier::Warm)
                    .with_scope("hr")
                    .unwrap(),
            )
            .await
            .is_err());

        let eng_cand = engine
            .stage_memory_as(
                &authed,
                StageMemoryRequest::new("Prefer connection pooling.", VaultTier::Warm)
                    .with_scope("engineering")
                    .unwrap(),
            )
            .await
            .unwrap();
        let pending = engine.pending_inbox_as(&authed).await.unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].proposed_by, "ci-platform");

        // Approve a secret HR candidate directly and ensure `ci-platform` cannot inspect it in Approved.md.
        let hr_cand = engine
            .stage_memory(
                StageMemoryRequest::new("HR bonus target is 15 percent.", VaultTier::Warm)
                    .with_scope("hr")
                    .unwrap(),
            )
            .await
            .unwrap();
        engine
            .decide_inbox_candidate(&eng_cand, true)
            .await
            .unwrap();
        engine.decide_inbox_candidate(&hr_cand, true).await.unwrap();

        assert!(engine
            .inspect_source_as(&authed, "Engineering/DB.md#DB")
            .await
            .is_ok());
        assert!(engine
            .inspect_source_as(&authed, &format!("Aether/Compiled/Approved.md#{eng_cand}"))
            .await
            .is_ok());
        assert!(engine
            .inspect_source_as(&authed, "HR/Compensation.md#Bands")
            .await
            .is_err());
        assert!(engine
            .inspect_source_as(&authed, &format!("Aether/Compiled/Approved.md#{hr_cand}"))
            .await
            .is_err());

        // Reader role cannot stage memory.
        let reader = engine
            .issue_api_key(
                IssueApiKeyRequest::new(
                    AgentId::new("readonly-bot").unwrap(),
                    AgentRole::Reader,
                    vec![ScopeTag::new("engineering").unwrap()],
                )
                .unwrap(),
            )
            .await
            .unwrap();
        assert!(engine
            .stage_memory_as(
                &reader.key,
                StageMemoryRequest::new("Should be rejected.", VaultTier::Warm)
                    .with_scope("engineering")
                    .unwrap(),
            )
            .await
            .is_err());

        // Revocation takes effect immediately on next authentication.
        assert!(engine
            .revoke_api_key(&issued_eng_ops.key.key_id)
            .await
            .unwrap());
        assert!(matches!(
            engine.authenticate_api_key(&parsed_secret).await,
            Err(AuthError::RevokedToken(_))
        ));

        // Bootstrap admin key creation is idempotent across calls.
        let admin_secret = ApiKeySecret::generate();
        let admin_a = engine
            .ensure_bootstrap_admin_key(AgentId::new("bootstrap-admin").unwrap(), &admin_secret)
            .await
            .unwrap();
        let admin_b = engine
            .ensure_bootstrap_admin_key(AgentId::new("bootstrap-admin").unwrap(), &admin_secret)
            .await
            .unwrap();
        assert_eq!(admin_a.key_id, admin_b.key_id);
        assert_eq!(engine.list_api_keys().await.unwrap().len(), 3);

        engine.shutdown().await.unwrap();
    }

    fn run_git_ok(dir: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "Test User")
            .env("GIT_AUTHOR_EMAIL", "test@aether.local")
            .env("GIT_COMMITTER_NAME", "Test User")
            .env("GIT_COMMITTER_EMAIL", "test@aether.local")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {:?} failed in {}: {}",
            args,
            dir.display(),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    #[tokio::test]
    async fn git_sync_resolves_aether_lattice_conflicts_and_preserves_conflicted_note_amus() {
        let base_dir = std::env::temp_dir().join(format!("aether_git_team_{}", Uuid::new_v4()));
        let bare_dir = base_dir.join("origin.git");
        let alice_dir = base_dir.join("alice");
        let bob_dir = base_dir.join("bob");
        fs::create_dir_all(&bare_dir).unwrap();
        fs::create_dir_all(&alice_dir).unwrap();

        run_git_ok(&bare_dir, &["init", "--bare", "-b", "main"]);
        run_git_ok(&alice_dir, &["init", "-b", "main"]);
        fs::write(
            alice_dir.join("Arch.md"),
            "---\naether_tier: warm\naether_scope:\n  - engineering\n---\n# Engine\nOriginal clean architecture rule.\n",
        )
        .unwrap();
        run_git_ok(&alice_dir, &["add", "."]);
        run_git_ok(&alice_dir, &["commit", "-m", "initial vault"]);
        run_git_ok(
            &alice_dir,
            &["remote", "add", "origin", bare_dir.to_str().unwrap()],
        );
        run_git_ok(&alice_dir, &["push", "-u", "origin", "main"]);
        run_git_ok(
            &base_dir,
            &[
                "clone",
                bare_dir.to_str().unwrap(),
                bob_dir.to_str().unwrap(),
            ],
        );

        let alice_engine = VaultEngine::open(VaultConfig::new(&alice_dir))
            .await
            .unwrap();
        let bob_engine = VaultEngine::open(VaultConfig::new(&bob_dir)).await.unwrap();

        // Both stage one shared candidate; Alice approves it and modifies Arch.md.
        let shared_req = StageMemoryRequest::new("Shared WAL rule.", VaultTier::Warm)
            .with_scope("engineering")
            .unwrap();
        let shared_cid = alice_engine.stage_memory(shared_req.clone()).await.unwrap();
        let bob_shared_cid = bob_engine.stage_memory(shared_req).await.unwrap();
        assert_eq!(shared_cid, bob_shared_cid);

        alice_engine
            .decide_inbox_candidate(&shared_cid, true)
            .await
            .unwrap();
        let alice_only_cid = alice_engine
            .stage_memory(
                StageMemoryRequest::new("Alice pending rule.", VaultTier::Warm)
                    .with_scope("engineering")
                    .unwrap(),
            )
            .await
            .unwrap();
        fs::write(
            alice_dir.join("Arch.md"),
            "---\naether_tier: warm\naether_scope:\n  - engineering\n---\n# Engine\nAlice updated architecture rule.\n",
        )
        .unwrap();
        run_git_ok(&alice_dir, &["add", "Arch.md", "Aether/"]);
        run_git_ok(&alice_dir, &["commit", "-m", "alice updates"]);
        run_git_ok(&alice_dir, &["push", "origin", "main"]);

        // Bob stages another candidate and edits Arch.md differently, creating conflicts on both.
        let bob_only_cid = bob_engine
            .stage_memory(
                StageMemoryRequest::new("Bob pending rule.", VaultTier::Warm)
                    .with_scope("engineering")
                    .unwrap(),
            )
            .await
            .unwrap();
        fs::write(
            bob_dir.join("Arch.md"),
            "---\naether_tier: warm\naether_scope:\n  - engineering\n---\n# Engine\nBob updated architecture rule.\n",
        )
        .unwrap();
        run_git_ok(&bob_dir, &["add", "Arch.md", "Aether/"]);
        run_git_ok(&bob_dir, &["commit", "-m", "bob updates"]);

        let report = bob_engine
            .git_sync(GitSyncOptions::new().with_push(false))
            .await
            .unwrap();

        assert!(!report.merged_cleanly);
        assert!(report.aether_conflicts_resolved >= 1);
        assert_eq!(
            report.conflicted_files,
            vec![VaultPath::parse("Arch.md").unwrap()]
        );
        assert!(!report.pushed);

        // .gitignore must exclude .aether/ and never stage sqlite files.
        let gitignore = fs::read_to_string(bob_dir.join(".gitignore")).unwrap();
        assert!(gitignore.contains(".aether/"));

        // Approved shared_cid must win the monotonic lattice and both pending items remain.
        let approved_md = fs::read_to_string(bob_dir.join("Aether/Compiled/Approved.md")).unwrap();
        assert!(approved_md.contains(shared_cid.as_str()));
        let inbox_md = fs::read_to_string(bob_dir.join("Aether/Inbox.md")).unwrap();
        assert!(!inbox_md.contains(shared_cid.as_str()));
        assert!(inbox_md.contains(alice_only_cid.as_str()));
        assert!(inbox_md.contains(bob_only_cid.as_str()));

        // SQLite retains Bob's last valid AMU for conflicted Arch.md (does not drop or index `<<<<<<<`).
        let recalled = bob_engine
            .recall(RecallRequest::new(
                "Original clean architecture rule Shared WAL",
            ))
            .await
            .unwrap();
        assert!(recalled.formatted_context.contains("Shared WAL rule."));
        assert!(recalled
            .formatted_context
            .contains("Original clean architecture rule."));
        assert!(!recalled.formatted_context.contains("<<<<<<<"));

        alice_engine.shutdown().await.unwrap();
        bob_engine.shutdown().await.unwrap();
        let _ = fs::remove_dir_all(&base_dir);
    }
}
