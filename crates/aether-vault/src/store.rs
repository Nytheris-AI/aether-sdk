use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use nytheris_aether_core::{
    AetherError, DistanceMetric, Entity, EntityId, GraphEdge, MemoryId, MemoryItem, MemoryScope,
    MemoryTier, ProjectId, Result, Subgraph,
};
use nytheris_aether_cortex::recall::Candidate;
use rusqlite::{
    params, Connection, ErrorCode, OptionalExtension, Transaction, TransactionBehavior,
};

use crate::auth::{
    constant_time_eq, AgentId, AgentRole, AuthError, KeyId, ScopePolicy, ScopedApiKey,
};
use crate::embedder::{embed_amu_text, Embedder};
use crate::kv::{KvBlockHash, KvBlockMode, KvCacheBlock, KvStorageTier, ModelId};
use crate::parser::ParsedSection;
use crate::types::{
    Amu, AmuId, AmuKind, AmuStatus, ApprovedMirrorEntry, CandidateId, ContentHash, DbTarget,
    InboxCheckbox, InboxEntry, ScopeTag, SourcePointer, VaultMeta, VaultPath, VaultTier,
};

const CONNECTOR_NAME: &str = "sqlite-vault";

const SCHEMA_SQL: &str = "
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA busy_timeout = 5000;
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS vault_files (
    rel_path       TEXT PRIMARY KEY,
    sha256         TEXT NOT NULL,
    indexed_at_us  INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS amus (
    amu_id            TEXT PRIMARY KEY,
    memory_id         TEXT NOT NULL UNIQUE,
    rel_path          TEXT NOT NULL,
    tier              TEXT NOT NULL,
    kind              TEXT NOT NULL,
    status            TEXT NOT NULL,
    heading_path      TEXT NOT NULL,
    distilled_fact    TEXT NOT NULL,
    source_pointer    TEXT NOT NULL UNIQUE,
    graph_edges_json  TEXT NOT NULL,
    valid_from        TEXT NOT NULL,
    superseded_by     TEXT,
    scopes_json       TEXT NOT NULL,
    authority_weight  REAL NOT NULL,
    content_hash      TEXT NOT NULL,
    embedding         BLOB NOT NULL,
    version           INTEGER NOT NULL DEFAULT 1
);
CREATE INDEX IF NOT EXISTS idx_amus_rel_path ON amus (rel_path);
CREATE INDEX IF NOT EXISTS idx_amus_tier_status ON amus (tier, status);

CREATE VIRTUAL TABLE IF NOT EXISTS amus_fts USING fts5(
    amu_id UNINDEXED,
    heading_path,
    distilled_fact,
    graph_edges_text,
    tokenize = 'porter unicode61'
);

CREATE TABLE IF NOT EXISTS amu_edges (
    source_memory_id  TEXT NOT NULL,
    src_entity        TEXT NOT NULL,
    dst_entity        TEXT NOT NULL,
    relation_type     TEXT NOT NULL,
    valid_at          TEXT NOT NULL,
    invalid_at        TEXT,
    PRIMARY KEY (source_memory_id, src_entity, dst_entity, relation_type)
);
CREATE INDEX IF NOT EXISTS idx_edges_src ON amu_edges (src_entity, invalid_at);
CREATE INDEX IF NOT EXISTS idx_edges_dst ON amu_edges (dst_entity, invalid_at);

CREATE TABLE IF NOT EXISTS self_write_ledger (
    rel_path       TEXT NOT NULL,
    sha256         TEXT NOT NULL,
    written_at_us  INTEGER NOT NULL,
    PRIMARY KEY (rel_path, sha256)
);

CREATE TABLE IF NOT EXISTS staged_candidates (
    candidate_id       TEXT PRIMARY KEY,
    tier               TEXT NOT NULL,
    scope              TEXT NOT NULL,
    proposed_fact      TEXT NOT NULL,
    target_link        TEXT,
    proposed_by        TEXT NOT NULL,
    proposed_date      TEXT NOT NULL,
    supersedes_amu_id  TEXT,
    status             TEXT NOT NULL,
    compiled_amu_id    TEXT
);

CREATE TABLE IF NOT EXISTS vault_meta (
    singleton_id    INTEGER PRIMARY KEY CHECK (singleton_id = 1),
    schema_version  INTEGER NOT NULL,
    embedder_id     TEXT NOT NULL,
    embedding_dim   INTEGER NOT NULL,
    updated_at      TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS amus_shadow_embeddings (
    amu_id     TEXT PRIMARY KEY,
    embedding  BLOB NOT NULL
);

CREATE TABLE IF NOT EXISTS dream_watermarks (
    rel_path          TEXT PRIMARY KEY,
    last_sha256       TEXT NOT NULL,
    processed_at_us   INTEGER NOT NULL,
    candidates_staged INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS agent_api_keys (
    key_id       TEXT PRIMARY KEY,
    agent_id     TEXT NOT NULL,
    key_hash     TEXT NOT NULL UNIQUE,
    role         TEXT NOT NULL,
    scopes_json  TEXT NOT NULL,
    created_at   TEXT NOT NULL,
    expires_at   TEXT,
    revoked_at   TEXT
);
CREATE INDEX IF NOT EXISTS idx_agent_api_keys_agent ON agent_api_keys (agent_id);

CREATE TABLE IF NOT EXISTS kv_cache_blocks (
    block_hash    TEXT NOT NULL,
    model_id      TEXT NOT NULL,
    mode          TEXT NOT NULL,
    tier          TEXT NOT NULL,
    parent_hash   TEXT,
    amu_id        TEXT,
    prefix_hash   TEXT,
    scope         TEXT NOT NULL,
    token_count   INTEGER NOT NULL,
    byte_size     INTEGER NOT NULL,
    warmed_at     TEXT NOT NULL,
    last_hit_at   TEXT NOT NULL,
    PRIMARY KEY (model_id, block_hash)
);
CREATE INDEX IF NOT EXISTS idx_kv_blocks_amu ON kv_cache_blocks (amu_id);
CREATE INDEX IF NOT EXISTS idx_kv_blocks_prefix ON kv_cache_blocks (prefix_hash);
";

#[derive(Clone)]
pub(crate) struct VaultStore {
    conn: Arc<Mutex<Option<Connection>>>,
    project_id: ProjectId,
}

pub(crate) struct RecallBundle {
    pub(crate) hot_amus: Vec<Amu>,
    pub(crate) fts_candidates: Vec<Candidate>,
    pub(crate) vector_candidates: Vec<Candidate>,
    pub(crate) subgraph: Subgraph,
    pub(crate) seed_entities: Vec<EntityId>,
    pub(crate) amus_by_memory_id: HashMap<MemoryId, Amu>,
}

impl VaultStore {
    pub(crate) async fn open(target: DbTarget, project_id: ProjectId) -> Result<Self> {
        let conn = tokio::task::spawn_blocking(move || -> Result<Connection> {
            let conn = match target {
                DbTarget::InMemory => Connection::open_in_memory().map_err(map_sqlite_err)?,
                DbTarget::File(path) => {
                    if let Some(parent) = path.parent() {
                        std::fs::create_dir_all(parent).map_err(|err| {
                            permanent_err(format!(
                                "failed to create db dir `{}`: {err}",
                                parent.display()
                            ))
                        })?;
                    }
                    Connection::open(path).map_err(map_sqlite_err)?
                }
            };
            conn.execute_batch(SCHEMA_SQL).map_err(map_sqlite_err)?;
            Ok(conn)
        })
        .await
        .map_err(|err| permanent_err(format!("sqlite task join error: {err}")))??;

        Ok(Self {
            conn: Arc::new(Mutex::new(Some(conn))),
            project_id,
        })
    }

    pub(crate) async fn close(&self) -> Result<()> {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let mut guard = conn
                .lock()
                .map_err(|_| permanent_err("sqlite mutex poisoned"))?;
            let _ = guard.take();
            Ok(())
        })
        .await
        .map_err(|err| permanent_err(format!("sqlite task join error: {err}")))?
    }

    async fn with_conn<F, R>(&self, op: F) -> Result<R>
    where
        F: FnOnce(&mut Connection) -> Result<R> + Send + 'static,
        R: Send + 'static,
    {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let mut guard = conn
                .lock()
                .map_err(|_| permanent_err("sqlite mutex poisoned"))?;
            let conn = guard
                .as_mut()
                .ok_or_else(|| permanent_err("sqlite connection is closed"))?;
            op(conn)
        })
        .await
        .map_err(|err| permanent_err(format!("sqlite task join error: {err}")))?
    }

    pub(crate) async fn file_manifest(&self) -> Result<HashMap<String, ContentHash>> {
        self.with_conn(|conn| {
            let mut stmt = conn
                .prepare("SELECT rel_path, sha256 FROM vault_files")
                .map_err(map_sqlite_err)?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(map_sqlite_err)?;
            let mut manifest = HashMap::new();
            for row in rows {
                let (path, sha) = row.map_err(map_sqlite_err)?;
                if let Ok(hash) = ContentHash::parse(&sha) {
                    manifest.insert(path, hash);
                }
            }
            Ok(manifest)
        })
        .await
    }

    pub(crate) async fn record_self_write(&self, rel_path: &str, hash: &ContentHash) -> Result<()> {
        let rel = rel_path.to_string();
        let sha = hash.as_str().to_string();
        let now = now_epoch_us();
        self.with_conn(move |conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(map_sqlite_err)?;
            tx.execute(
                "INSERT OR REPLACE INTO self_write_ledger (rel_path, sha256, written_at_us)
                 VALUES (?1, ?2, ?3)",
                params![rel, sha, now],
            )
            .map_err(map_sqlite_err)?;
            tx.execute(
                "INSERT OR REPLACE INTO vault_files (rel_path, sha256, indexed_at_us)
                 VALUES (?1, ?2, ?3)",
                params![rel, sha, now],
            )
            .map_err(map_sqlite_err)?;
            tx.commit().map_err(map_sqlite_err)?;
            Ok(())
        })
        .await
    }

    pub(crate) async fn consume_self_write(
        &self,
        rel_path: &str,
        hash: &ContentHash,
    ) -> Result<bool> {
        let rel = rel_path.to_string();
        let sha = hash.as_str().to_string();
        self.with_conn(move |conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(map_sqlite_err)?;
            let deleted = tx
                .execute(
                    "DELETE FROM self_write_ledger WHERE rel_path = ?1 AND sha256 = ?2",
                    params![rel, sha],
                )
                .map_err(map_sqlite_err)?;
            tx.commit().map_err(map_sqlite_err)?;
            Ok(deleted > 0)
        })
        .await
    }

    pub(crate) async fn update_file_watermark(
        &self,
        rel_path: &str,
        hash: &ContentHash,
    ) -> Result<()> {
        let rel = rel_path.to_string();
        let sha = hash.as_str().to_string();
        let now = now_epoch_us();
        self.with_conn(move |conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(map_sqlite_err)?;
            tx.execute(
                "INSERT OR REPLACE INTO vault_files (rel_path, sha256, indexed_at_us)
                 VALUES (?1, ?2, ?3)",
                params![rel, sha, now],
            )
            .map_err(map_sqlite_err)?;
            tx.commit().map_err(map_sqlite_err)?;
            Ok(())
        })
        .await
    }

    pub(crate) async fn commit_file_sections(
        &self,
        rel_path: &VaultPath,
        file_hash: &ContentHash,
        sections: &[ParsedSection],
    ) -> Result<(usize, Vec<KvBlockHash>)> {
        let rel = rel_path.as_str().to_string();
        let sha = file_hash.as_str().to_string();
        let now = now_epoch_us();
        let owned_sections = sections.to_vec();

        self.with_conn(move |conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(map_sqlite_err)?;

            let (_, mut evicted_kv) =
                delete_amus_for_rel_path(&tx, &rel).map_err(map_sqlite_err)?;
            let mut has_new_hot = false;
            for sec in &owned_sections {
                if sec.amu.tier == VaultTier::Hot {
                    has_new_hot = true;
                }
                insert_amu_tx(&tx, &rel, &sec.amu, &sec.embedding).map_err(map_sqlite_err)?;
            }
            if has_new_hot {
                evicted_kv.extend(delete_kv_paged_prefix_tx(&tx).map_err(map_sqlite_err)?);
            }

            tx.execute(
                "INSERT OR REPLACE INTO vault_files (rel_path, sha256, indexed_at_us)
                 VALUES (?1, ?2, ?3)",
                params![rel, sha, now],
            )
            .map_err(map_sqlite_err)?;

            tx.commit().map_err(map_sqlite_err)?;
            Ok((owned_sections.len(), evicted_kv))
        })
        .await
    }

    pub(crate) async fn remove_file(
        &self,
        rel_path: &str,
        new_watermark: Option<&ContentHash>,
    ) -> Result<(bool, Vec<KvBlockHash>)> {
        let rel = rel_path.to_string();
        let wm = new_watermark.map(|h| h.as_str().to_string());
        let now = now_epoch_us();

        self.with_conn(move |conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(map_sqlite_err)?;

            let (deleted_amus, evicted_kv) =
                delete_amus_for_rel_path(&tx, &rel).map_err(map_sqlite_err)?;
            let deleted_file = tx
                .execute("DELETE FROM vault_files WHERE rel_path = ?1", params![rel])
                .map_err(map_sqlite_err)?;

            if let Some(sha) = wm {
                tx.execute(
                    "INSERT OR REPLACE INTO vault_files (rel_path, sha256, indexed_at_us)
                     VALUES (?1, ?2, ?3)",
                    params![rel, sha, now],
                )
                .map_err(map_sqlite_err)?;
            }

            tx.commit().map_err(map_sqlite_err)?;
            Ok((deleted_amus > 0 || deleted_file > 0, evicted_kv))
        })
        .await
    }

    pub(crate) async fn stage_candidate(&self, entry: &InboxEntry) -> Result<()> {
        let entry = entry.clone();
        self.with_conn(move |conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(map_sqlite_err)?;
            tx.execute(
                "INSERT OR IGNORE INTO staged_candidates (
                     candidate_id, tier, scope, proposed_fact, target_link,
                     proposed_by, proposed_date, supersedes_amu_id, status, compiled_amu_id
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'pending', NULL)",
                params![
                    entry.candidate_id.as_str(),
                    entry.tier.as_str(),
                    entry.scope.as_str(),
                    entry.proposed_fact,
                    entry.target_link,
                    entry.proposed_by,
                    entry.proposed_date,
                    entry.supersedes_amu_id.map(|id| id.to_string()),
                ],
            )
            .map_err(map_sqlite_err)?;
            tx.commit().map_err(map_sqlite_err)?;
            Ok(())
        })
        .await
    }

    pub(crate) async fn list_pending_candidates(&self) -> Result<Vec<InboxEntry>> {
        self.list_candidates_by_status("pending", InboxCheckbox::Pending)
            .await
    }

    pub(crate) async fn list_rejected_candidates(&self) -> Result<Vec<InboxEntry>> {
        self.list_candidates_by_status("rejected", InboxCheckbox::Rejected)
            .await
    }

    async fn list_candidates_by_status(
        &self,
        status: &'static str,
        checkbox: InboxCheckbox,
    ) -> Result<Vec<InboxEntry>> {
        self.with_conn(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT candidate_id, tier, scope, proposed_fact, target_link,
                            proposed_by, proposed_date, supersedes_amu_id
                     FROM staged_candidates
                     WHERE status = ?1
                     ORDER BY rowid ASC",
                )
                .map_err(map_sqlite_err)?;
            let rows = stmt
                .query_map(params![status], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, Option<String>>(7)?,
                    ))
                })
                .map_err(map_sqlite_err)?;

            let mut out = Vec::new();
            for row in rows {
                let (cid, tier, scope, fact, target, by, date, sup) =
                    row.map_err(map_sqlite_err)?;
                let Ok(candidate_id) = CandidateId::parse(&cid) else {
                    continue;
                };
                out.push(InboxEntry {
                    checkbox,
                    tier: VaultTier::parse_str(&tier).unwrap_or_default(),
                    scope: ScopeTag::new(scope).unwrap_or_default(),
                    proposed_fact: fact,
                    candidate_id,
                    target_link: target,
                    proposed_by: by,
                    proposed_date: date,
                    supersedes_amu_id: sup.and_then(|s| AmuId::parse(&s).ok()),
                });
            }
            Ok(out)
        })
        .await
    }

    pub(crate) async fn apply_lattice_state(
        &self,
        pending: &[InboxEntry],
        approved: &[(InboxEntry, Amu, Vec<f32>)],
        rejected: &[InboxEntry],
    ) -> Result<Vec<KvBlockHash>> {
        let pending = pending.to_vec();
        let approved = approved.to_vec();
        let rejected = rejected.to_vec();

        self.with_conn(move |conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(map_sqlite_err)?;
            let mut evicted_kv = Vec::new();

            for (entry, amu, emb) in &approved {
                let cid = entry.candidate_id.as_str();
                let amu_id_str = amu.amu_id.to_string();
                let existing_compiled: Option<(String, Option<String>)> = tx
                    .query_row(
                        "SELECT status, compiled_amu_id FROM staged_candidates WHERE candidate_id = ?1",
                        params![cid],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()
                    .map_err(map_sqlite_err)?;
                let already_compiled = existing_compiled
                    .as_ref()
                    .is_some_and(|(st, aid)| st == "approved" && aid.as_deref() == Some(amu_id_str.as_str()));

                if !already_compiled {
                    let rel_path = amu.source_pointer.relative_path().to_string();
                    evicted_kv.extend(
                        delete_kv_blocks_for_amu_tx(&tx, &amu_id_str).map_err(map_sqlite_err)?,
                    );
                    if amu.tier == VaultTier::Hot {
                        evicted_kv.extend(delete_kv_paged_prefix_tx(&tx).map_err(map_sqlite_err)?);
                    }
                    tx.execute(
                        "DELETE FROM amus_fts WHERE amu_id = ?1",
                        params![amu_id_str],
                    )
                    .map_err(map_sqlite_err)?;
                    tx.execute(
                        "DELETE FROM amus WHERE amu_id = ?1",
                        params![amu_id_str],
                    )
                    .map_err(map_sqlite_err)?;
                    insert_amu_tx(&tx, &rel_path, amu, emb).map_err(map_sqlite_err)?;
                    if let Some(old_id) = entry.supersedes_amu_id {
                        evicted_kv.extend(
                            supersede_amu_tx(&tx, old_id, amu.amu_id, amu.valid_from)
                                .map_err(map_sqlite_err)?,
                        );
                    }
                }

                tx.execute(
                    "INSERT INTO staged_candidates (
                         candidate_id, tier, scope, proposed_fact, target_link,
                         proposed_by, proposed_date, supersedes_amu_id, status, compiled_amu_id
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'approved', ?9)
                     ON CONFLICT(candidate_id) DO UPDATE SET
                         tier = excluded.tier,
                         scope = excluded.scope,
                         proposed_fact = excluded.proposed_fact,
                         target_link = COALESCE(excluded.target_link, staged_candidates.target_link),
                         proposed_by = excluded.proposed_by,
                         proposed_date = excluded.proposed_date,
                         supersedes_amu_id = COALESCE(excluded.supersedes_amu_id, staged_candidates.supersedes_amu_id),
                         status = 'approved',
                         compiled_amu_id = excluded.compiled_amu_id",
                    params![
                        cid,
                        entry.tier.as_str(),
                        entry.scope.as_str(),
                        entry.proposed_fact,
                        entry.target_link,
                        entry.proposed_by,
                        entry.proposed_date,
                        entry.supersedes_amu_id.map(|id| id.to_string()),
                        amu_id_str,
                    ],
                )
                .map_err(map_sqlite_err)?;
            }

            for entry in &rejected {
                tx.execute(
                    "INSERT INTO staged_candidates (
                         candidate_id, tier, scope, proposed_fact, target_link,
                         proposed_by, proposed_date, supersedes_amu_id, status, compiled_amu_id
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'rejected', NULL)
                     ON CONFLICT(candidate_id) DO UPDATE SET
                         status = CASE
                             WHEN staged_candidates.status = 'approved' THEN 'approved'
                             ELSE 'rejected'
                         END",
                    params![
                        entry.candidate_id.as_str(),
                        entry.tier.as_str(),
                        entry.scope.as_str(),
                        entry.proposed_fact,
                        entry.target_link,
                        entry.proposed_by,
                        entry.proposed_date,
                        entry.supersedes_amu_id.map(|id| id.to_string()),
                    ],
                )
                .map_err(map_sqlite_err)?;
            }

            for entry in &pending {
                tx.execute(
                    "INSERT OR IGNORE INTO staged_candidates (
                         candidate_id, tier, scope, proposed_fact, target_link,
                         proposed_by, proposed_date, supersedes_amu_id, status, compiled_amu_id
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'pending', NULL)",
                    params![
                        entry.candidate_id.as_str(),
                        entry.tier.as_str(),
                        entry.scope.as_str(),
                        entry.proposed_fact,
                        entry.target_link,
                        entry.proposed_by,
                        entry.proposed_date,
                        entry.supersedes_amu_id.map(|id| id.to_string()),
                    ],
                )
                .map_err(map_sqlite_err)?;
            }

            tx.commit().map_err(map_sqlite_err)?;
            Ok(evicted_kv)
        })
        .await
    }

    pub(crate) async fn list_approved_candidates(&self) -> Result<Vec<ApprovedMirrorEntry>> {
        self.with_conn(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT candidate_id, compiled_amu_id, tier, scope, proposed_fact, target_link,
                            proposed_by, proposed_date, supersedes_amu_id
                     FROM staged_candidates
                     WHERE status = 'approved' AND compiled_amu_id IS NOT NULL
                     ORDER BY rowid ASC",
                )
                .map_err(map_sqlite_err)?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, String>(7)?,
                        row.get::<_, Option<String>>(8)?,
                    ))
                })
                .map_err(map_sqlite_err)?;

            let mut out = Vec::new();
            for row in rows {
                let (cid, aid, tier, scope, fact, target, by, date, sup) =
                    row.map_err(map_sqlite_err)?;
                let Ok(candidate_id) = CandidateId::parse(&cid) else {
                    continue;
                };
                let Ok(compiled_amu_id) = AmuId::parse(&aid) else {
                    continue;
                };
                out.push(ApprovedMirrorEntry {
                    candidate_id,
                    compiled_amu_id,
                    tier: VaultTier::parse_str(&tier).unwrap_or_default(),
                    scope: ScopeTag::new(scope).unwrap_or_default(),
                    proposed_fact: fact,
                    target_link: target,
                    proposed_by: by,
                    proposed_date: date,
                    supersedes_amu_id: sup.and_then(|s| AmuId::parse(&s).ok()),
                });
            }
            Ok(out)
        })
        .await
    }

    pub(crate) async fn get_vault_meta(&self) -> Result<Option<VaultMeta>> {
        self.with_conn(|conn| {
            let row: Option<(u32, String, i64, String)> = conn
                .query_row(
                    "SELECT schema_version, embedder_id, embedding_dim, updated_at
                     FROM vault_meta WHERE singleton_id = 1",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .optional()
                .map_err(map_sqlite_err)?;
            let Some((schema_version, embedder_id, dim_i64, updated_raw)) = row else {
                return Ok(None);
            };
            let embedding_dim = usize::try_from(dim_i64).unwrap_or(0);
            let updated_at = DateTime::parse_from_rfc3339(&updated_raw)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now());
            Ok(Some(VaultMeta {
                schema_version,
                embedder_id,
                embedding_dim,
                updated_at,
            }))
        })
        .await
    }

    pub(crate) async fn set_vault_meta(&self, meta: &VaultMeta) -> Result<()> {
        let meta = meta.clone();
        self.with_conn(move |conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(map_sqlite_err)?;
            upsert_vault_meta_tx(&tx, &meta).map_err(map_sqlite_err)?;
            tx.commit().map_err(map_sqlite_err)?;
            Ok(())
        })
        .await
    }

    pub(crate) async fn count_active_amus(&self) -> Result<usize> {
        self.with_conn(|conn| {
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM amus WHERE status = 'active'",
                    [],
                    |r| r.get(0),
                )
                .map_err(map_sqlite_err)?;
            Ok(usize::try_from(count).unwrap_or(0))
        })
        .await
    }

    pub(crate) async fn active_embedding_dim(&self) -> Result<Option<usize>> {
        self.with_conn(|conn| {
            let len_bytes: Option<i64> = conn
                .query_row(
                    "SELECT length(embedding) FROM amus WHERE status = 'active' LIMIT 1",
                    [],
                    |r| r.get(0),
                )
                .optional()
                .map_err(map_sqlite_err)?;
            Ok(len_bytes
                .and_then(|b| usize::try_from(b).ok())
                .map(|b| b / 4))
        })
        .await
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) async fn shadow_table_row_count(&self) -> Result<usize> {
        self.with_conn(|conn| {
            let count: i64 = conn
                .query_row("SELECT COUNT(*) FROM amus_shadow_embeddings", [], |r| {
                    r.get(0)
                })
                .map_err(map_sqlite_err)?;
            Ok(usize::try_from(count).unwrap_or(0))
        })
        .await
    }

    pub(crate) async fn reembed_active_sync(
        &self,
        embedder: Arc<dyn Embedder>,
        schema_version: u32,
    ) -> Result<usize> {
        self.with_conn(move |conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(map_sqlite_err)?;
            let rows = load_active_embed_inputs(&tx).map_err(map_sqlite_err)?;
            let count = rows.len();
            for (amu_id, heading_path, distilled_fact) in rows {
                let emb = embedder.embed(&embed_amu_text(&heading_path, &distilled_fact));
                let blob = encode_embedding(&emb);
                tx.execute(
                    "UPDATE amus SET embedding = ?1 WHERE amu_id = ?2",
                    params![blob, amu_id],
                )
                .map_err(map_sqlite_err)?;
            }
            tx.execute("DELETE FROM amus_shadow_embeddings", [])
                .map_err(map_sqlite_err)?;
            let meta = VaultMeta {
                schema_version,
                embedder_id: embedder.id().to_string(),
                embedding_dim: embedder.dimensions(),
                updated_at: Utc::now(),
            };
            upsert_vault_meta_tx(&tx, &meta).map_err(map_sqlite_err)?;
            tx.commit().map_err(map_sqlite_err)?;
            Ok(count)
        })
        .await
    }

    pub(crate) async fn reembed_active_shadow_swap(
        &self,
        embedder: Arc<dyn Embedder>,
        schema_version: u32,
    ) -> Result<usize> {
        let rows = self
            .with_conn(|conn| {
                let tx = conn
                    .transaction_with_behavior(TransactionBehavior::Deferred)
                    .map_err(map_sqlite_err)?;
                load_active_embed_inputs(&tx).map_err(map_sqlite_err)
            })
            .await?;

        let embedder_for_compute = Arc::clone(&embedder);
        let computed: Vec<(String, Vec<u8>)> = tokio::task::spawn_blocking(move || {
            rows.into_iter()
                .map(|(amu_id, heading_path, distilled_fact)| {
                    let vec =
                        embedder_for_compute.embed(&embed_amu_text(&heading_path, &distilled_fact));
                    (amu_id, encode_embedding(&vec))
                })
                .collect()
        })
        .await
        .map_err(|err| permanent_err(format!("shadow reembed compute join error: {err}")))?;

        let total = computed.len();
        self.with_conn(|conn| {
            conn.execute("DELETE FROM amus_shadow_embeddings", [])
                .map_err(map_sqlite_err)?;
            Ok(())
        })
        .await?;

        for chunk in computed.chunks(256) {
            let chunk_owned = chunk.to_vec();
            self.with_conn(move |conn| {
                let tx = conn
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .map_err(map_sqlite_err)?;
                for (amu_id, blob) in chunk_owned {
                    tx.execute(
                        "INSERT OR REPLACE INTO amus_shadow_embeddings (amu_id, embedding)
                         VALUES (?1, ?2)",
                        params![amu_id, blob],
                    )
                    .map_err(map_sqlite_err)?;
                }
                tx.commit().map_err(map_sqlite_err)?;
                Ok(())
            })
            .await?;
            tokio::task::yield_now().await;
        }

        let meta = VaultMeta {
            schema_version,
            embedder_id: embedder.id().to_string(),
            embedding_dim: embedder.dimensions(),
            updated_at: Utc::now(),
        };
        self.with_conn(move |conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(map_sqlite_err)?;
            tx.execute(
                "UPDATE amus
                 SET embedding = (
                     SELECT s.embedding FROM amus_shadow_embeddings s WHERE s.amu_id = amus.amu_id
                 )
                 WHERE amu_id IN (SELECT amu_id FROM amus_shadow_embeddings)",
                [],
            )
            .map_err(map_sqlite_err)?;
            tx.execute("DELETE FROM amus_shadow_embeddings", [])
                .map_err(map_sqlite_err)?;
            upsert_vault_meta_tx(&tx, &meta).map_err(map_sqlite_err)?;
            tx.commit().map_err(map_sqlite_err)?;
            Ok(total)
        })
        .await
    }

    pub(crate) async fn apply_inbox_decisions(
        &self,
        entries: &[InboxEntry],
        compiled_amus: &[(InboxEntry, Amu, Vec<f32>)],
    ) -> Result<(usize, usize, Vec<KvBlockHash>)> {
        let entries = entries.to_vec();
        let compiled = compiled_amus.to_vec();

        self.with_conn(move |conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(map_sqlite_err)?;

            let mut approved_count = 0usize;
            let mut rejected_count = 0usize;
            let mut evicted_kv = Vec::new();

            let compiled_map: HashMap<String, (Amu, Vec<f32>)> = compiled
                .into_iter()
                .map(|(entry, amu, emb)| (entry.candidate_id.as_str().to_string(), (amu, emb)))
                .collect();

            for entry in &entries {
                let cid = entry.candidate_id.as_str();
                let existing: Option<(String, Option<String>)> = tx
                    .query_row(
                        "SELECT status, supersedes_amu_id FROM staged_candidates WHERE candidate_id = ?1",
                        params![cid],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .optional()
                    .map_err(map_sqlite_err)?;

                match entry.checkbox {
                    InboxCheckbox::Pending => {}
                    InboxCheckbox::Approved => {
                        if existing.as_ref().is_some_and(|(st, _)| st == "approved") {
                            continue;
                        }
                        if let Some((amu, emb)) = compiled_map.get(cid) {
                            let amu_id_str = amu.amu_id.to_string();
                            let rel_path = amu.source_pointer.relative_path().to_string();

                            evicted_kv.extend(
                                delete_kv_blocks_for_amu_tx(&tx, &amu_id_str)
                                    .map_err(map_sqlite_err)?,
                            );
                            if amu.tier == VaultTier::Hot {
                                evicted_kv.extend(
                                    delete_kv_paged_prefix_tx(&tx).map_err(map_sqlite_err)?,
                                );
                            }

                            tx.execute(
                                "DELETE FROM amus_fts WHERE amu_id = ?1",
                                params![amu_id_str],
                            )
                            .map_err(map_sqlite_err)?;
                            tx.execute(
                                "DELETE FROM amus WHERE amu_id = ?1",
                                params![amu_id_str],
                            )
                            .map_err(map_sqlite_err)?;
                            insert_amu_tx(&tx, &rel_path, amu, emb).map_err(map_sqlite_err)?;

                            let effective_sup = entry.supersedes_amu_id.or_else(|| {
                                existing
                                    .as_ref()
                                    .and_then(|(_, s)| s.as_deref())
                                    .and_then(|s| AmuId::parse(s).ok())
                            });

                            if let Some(old_id) = effective_sup {
                                evicted_kv.extend(
                                    supersede_amu_tx(&tx, old_id, amu.amu_id, amu.valid_from)
                                        .map_err(map_sqlite_err)?,
                                );
                            }

                            tx.execute(
                                "INSERT INTO staged_candidates (
                                     candidate_id, tier, scope, proposed_fact, target_link,
                                     proposed_by, proposed_date, supersedes_amu_id, status, compiled_amu_id
                                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'approved', ?9)
                                 ON CONFLICT(candidate_id) DO UPDATE SET
                                     status = 'approved',
                                     compiled_amu_id = excluded.compiled_amu_id",
                                params![
                                    cid,
                                    entry.tier.as_str(),
                                    entry.scope.as_str(),
                                    entry.proposed_fact,
                                    entry.target_link,
                                    entry.proposed_by,
                                    entry.proposed_date,
                                    effective_sup.map(|id| id.to_string()),
                                    amu_id_str,
                                ],
                            )
                            .map_err(map_sqlite_err)?;

                            approved_count += 1;
                        }
                    }
                    InboxCheckbox::Rejected => {
                        if existing.as_ref().is_some_and(|(st, _)| st == "rejected") {
                            continue;
                        }
                        tx.execute(
                            "INSERT INTO staged_candidates (
                                 candidate_id, tier, scope, proposed_fact, target_link,
                                 proposed_by, proposed_date, supersedes_amu_id, status, compiled_amu_id
                             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'rejected', NULL)
                             ON CONFLICT(candidate_id) DO UPDATE SET status = 'rejected'",
                            params![
                                cid,
                                entry.tier.as_str(),
                                entry.scope.as_str(),
                                entry.proposed_fact,
                                entry.target_link,
                                entry.proposed_by,
                                entry.proposed_date,
                                entry.supersedes_amu_id.map(|id| id.to_string()),
                            ],
                        )
                        .map_err(map_sqlite_err)?;
                        rejected_count += 1;
                    }
                }
            }

            tx.commit().map_err(map_sqlite_err)?;
            Ok((approved_count, rejected_count, evicted_kv))
        })
        .await
    }

    pub(crate) async fn recall_bundle(
        &self,
        query_text: &str,
        query_vec: &[f32],
        scope_policy: &ScopePolicy,
        top_k: usize,
    ) -> Result<RecallBundle> {
        let query_text = query_text.to_string();
        let query_vec = query_vec.to_vec();
        let scope_policy = scope_policy.clone();
        let project_id = self.project_id.clone();

        self.with_conn(move |conn| {
            let active_rows = load_active_amus(conn).map_err(map_sqlite_err)?;
            let mut amus_by_id: HashMap<String, Amu> = HashMap::new();
            let mut amus_by_memory_id: HashMap<MemoryId, Amu> = HashMap::new();
            let mut hot_amus = Vec::new();
            let mut vector_scored: Vec<(MemoryId, f32)> = Vec::new();

            for (amu, emb) in active_rows {
                if !scope_policy_matches(&amu.scopes, &scope_policy) {
                    continue;
                }
                let mem_id = amu.amu_id.to_memory_id();
                if amu.tier == VaultTier::Hot {
                    hot_amus.push(amu.clone());
                } else if !query_vec.is_empty() && emb.len() == query_vec.len() {
                    let sim = DistanceMetric::Cosine.score(&query_vec, &emb);
                    vector_scored.push((mem_id, sim));
                }
                amus_by_id.insert(amu.amu_id.to_string(), amu.clone());
                amus_by_memory_id.insert(mem_id, amu);
            }

            if query_text.trim().is_empty() {
                return Ok(RecallBundle {
                    hot_amus,
                    fts_candidates: Vec::new(),
                    vector_candidates: Vec::new(),
                    subgraph: Subgraph::default(),
                    seed_entities: Vec::new(),
                    amus_by_memory_id,
                });
            }

            vector_scored.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            vector_scored.truncate(top_k);

            let vector_candidates: Vec<Candidate> = vector_scored
                .iter()
                .filter_map(|(mid, score)| {
                    amus_by_memory_id.get(mid).map(|amu| Candidate {
                        item: to_memory_item(amu, &project_id),
                        score: *score,
                    })
                })
                .collect();

            let mut fts_candidates = Vec::new();
            if let Some(fts_query) = build_fts5_query(&query_text) {
                let mut stmt = conn
                    .prepare(
                        "SELECT amus_fts.amu_id, -bm25(amus_fts) AS rank_score
                         FROM amus_fts
                         JOIN amus ON amus.amu_id = amus_fts.amu_id
                         WHERE amus.status = 'active'
                           AND amus.tier != 'hot'
                           AND amus_fts MATCH ?1
                         ORDER BY bm25(amus_fts) ASC",
                    )
                    .map_err(map_sqlite_err)?;
                let rows = stmt
                    .query_map(params![fts_query], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?))
                    })
                    .map_err(map_sqlite_err)?;

                for row in rows {
                    let (aid, score) = row.map_err(map_sqlite_err)?;
                    if let Some(amu) = amus_by_id.get(&aid) {
                        fts_candidates.push(Candidate {
                            item: to_memory_item(amu, &project_id),
                            score: score as f32,
                        });
                        if fts_candidates.len() >= top_k {
                            break;
                        }
                    }
                }
            }

            let active_mem_ids: HashSet<String> =
                amus_by_memory_id.keys().map(ToString::to_string).collect();
            let (seed_entities, subgraph) = build_recall_subgraph(
                conn,
                &query_text,
                &fts_candidates,
                &vector_candidates,
                &amus_by_memory_id,
                &active_mem_ids,
            )
            .map_err(map_sqlite_err)?;

            Ok(RecallBundle {
                hot_amus,
                fts_candidates,
                vector_candidates,
                subgraph,
                seed_entities,
                amus_by_memory_id,
            })
        })
        .await
    }

    pub(crate) async fn dream_watermarks(&self) -> Result<HashMap<String, ContentHash>> {
        self.with_conn(|conn| {
            let mut stmt = conn
                .prepare("SELECT rel_path, last_sha256 FROM dream_watermarks")
                .map_err(map_sqlite_err)?;
            let rows = stmt
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
                .map_err(map_sqlite_err)?;
            let mut out = HashMap::new();
            for row in rows {
                let (rel, sha) = row.map_err(map_sqlite_err)?;
                if let Ok(hash) = ContentHash::parse(&sha) {
                    out.insert(rel, hash);
                }
            }
            Ok(out)
        })
        .await
    }

    pub(crate) async fn record_dream_watermark(
        &self,
        rel_path: &str,
        hash: &ContentHash,
        candidates_staged: usize,
    ) -> Result<()> {
        let rel = rel_path.to_string();
        let sha = hash.as_str().to_string();
        let now = now_epoch_us();
        let staged_i64 = i64::try_from(candidates_staged).unwrap_or(i64::MAX);
        self.with_conn(move |conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(map_sqlite_err)?;
            tx.execute(
                "INSERT OR REPLACE INTO dream_watermarks (
                     rel_path, last_sha256, processed_at_us, candidates_staged
                 ) VALUES (?1, ?2, ?3, ?4)",
                params![rel, sha, now, staged_i64],
            )
            .map_err(map_sqlite_err)?;
            tx.commit().map_err(map_sqlite_err)?;
            Ok(())
        })
        .await
    }

    pub(crate) async fn list_active_amus(&self) -> Result<Vec<Amu>> {
        self.with_conn(|conn| {
            let active = load_active_amus(conn).map_err(map_sqlite_err)?;
            Ok(active.into_iter().map(|(amu, _)| amu).collect())
        })
        .await
    }

    pub(crate) async fn insert_api_key(
        &self,
        key: &ScopedApiKey,
        key_hash: &ContentHash,
    ) -> Result<()> {
        let key = key.clone();
        let hash = key_hash.as_str().to_string();
        self.with_conn(move |conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(map_sqlite_err)?;
            insert_api_key_tx(&tx, &key, &hash).map_err(map_sqlite_err)?;
            tx.commit().map_err(map_sqlite_err)?;
            Ok(())
        })
        .await
    }

    pub(crate) async fn ensure_bootstrap_admin_key(
        &self,
        agent_id: &AgentId,
        key_hash: &ContentHash,
    ) -> Result<ScopedApiKey> {
        let agent_id = agent_id.clone();
        let hash = key_hash.as_str().to_string();
        self.with_conn(move |conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(map_sqlite_err)?;

            let mut stmt = tx
                .prepare(
                    "SELECT key_id, agent_id, key_hash, role, scopes_json, created_at, expires_at, revoked_at
                     FROM agent_api_keys",
                )
                .map_err(map_sqlite_err)?;
            let rows = stmt
                .query_map([], read_api_key_row)
                .map_err(map_sqlite_err)?;
            let mut matched: Option<ScopedApiKey> = None;
            for row in rows {
                let (key, stored_hash) = row.map_err(map_sqlite_err)?;
                if constant_time_eq(stored_hash.as_bytes(), hash.as_bytes()) {
                    matched = Some(key);
                }
            }
            drop(stmt);

            if let Some(existing) = matched {
                tx.commit().map_err(map_sqlite_err)?;
                if existing.revoked_at.is_some() {
                    return Err(AetherError::InvalidInput(format!(
                        "bootstrap admin key `{}` is revoked",
                        existing.key_id
                    )));
                }
                return Ok(existing);
            }

            let key = ScopedApiKey::new(
                KeyId::generate(),
                agent_id,
                AgentRole::Admin,
                vec![ScopeTag::new("*")?],
                Utc::now(),
                None,
                None,
            )?;
            insert_api_key_tx(&tx, &key, &hash).map_err(map_sqlite_err)?;
            tx.commit().map_err(map_sqlite_err)?;
            Ok(key)
        })
        .await
    }

    pub(crate) async fn authenticate_api_key(
        &self,
        key_hash: &ContentHash,
        now: DateTime<Utc>,
    ) -> std::result::Result<ScopedApiKey, AuthError> {
        let hash = key_hash.as_str().to_string();
        let lookup = self
            .with_conn(move |conn| {
                let mut stmt = conn
                    .prepare(
                        "SELECT key_id, agent_id, key_hash, role, scopes_json, created_at, expires_at, revoked_at
                         FROM agent_api_keys",
                    )
                    .map_err(map_sqlite_err)?;
                let rows = stmt
                    .query_map([], read_api_key_row)
                    .map_err(map_sqlite_err)?;
                let mut matched: Option<ScopedApiKey> = None;
                for row in rows {
                    let (key, stored_hash) = row.map_err(map_sqlite_err)?;
                    if constant_time_eq(stored_hash.as_bytes(), hash.as_bytes()) {
                        matched = Some(key);
                    }
                }
                Ok(matched)
            })
            .await;

        let Ok(Some(key)) = lookup else {
            return Err(AuthError::InvalidToken);
        };
        key.is_active_at(now)?;
        Ok(key)
    }

    pub(crate) async fn list_api_keys(&self) -> Result<Vec<ScopedApiKey>> {
        self.with_conn(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT key_id, agent_id, key_hash, role, scopes_json, created_at, expires_at, revoked_at
                     FROM agent_api_keys
                     ORDER BY created_at ASC, key_id ASC",
                )
                .map_err(map_sqlite_err)?;
            let rows = stmt
                .query_map([], read_api_key_row)
                .map_err(map_sqlite_err)?;
            let mut out = Vec::new();
            for row in rows {
                let (key, _) = row.map_err(map_sqlite_err)?;
                out.push(key);
            }
            Ok(out)
        })
        .await
    }

    pub(crate) async fn revoke_api_key(&self, key_id: &KeyId, at: DateTime<Utc>) -> Result<bool> {
        let kid = key_id.as_str().to_string();
        let at_str = at.to_rfc3339();
        self.with_conn(move |conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(map_sqlite_err)?;
            let updated = tx
                .execute(
                    "UPDATE agent_api_keys SET revoked_at = ?1 WHERE key_id = ?2 AND revoked_at IS NULL",
                    params![at_str, kid],
                )
                .map_err(map_sqlite_err)?;
            tx.commit().map_err(map_sqlite_err)?;
            Ok(updated > 0)
        })
        .await
    }

    pub(crate) async fn load_all_kv_blocks(&self) -> Result<Vec<KvCacheBlock>> {
        self.query_kv_blocks(None).await
    }

    pub(crate) async fn query_kv_blocks(
        &self,
        model_id: Option<&ModelId>,
    ) -> Result<Vec<KvCacheBlock>> {
        let model_filter = model_id.map(|m| m.as_str().to_string());
        self.with_conn(move |conn| {
            let sql = match &model_filter {
                Some(_) => {
                    "SELECT block_hash, model_id, mode, tier, parent_hash, amu_id, prefix_hash,
                            scope, token_count, byte_size, warmed_at, last_hit_at
                     FROM kv_cache_blocks
                     WHERE model_id = ?1
                     ORDER BY warmed_at ASC, block_hash ASC"
                }
                None => {
                    "SELECT block_hash, model_id, mode, tier, parent_hash, amu_id, prefix_hash,
                            scope, token_count, byte_size, warmed_at, last_hit_at
                     FROM kv_cache_blocks
                     ORDER BY warmed_at ASC, block_hash ASC"
                }
            };
            let mut stmt = conn.prepare(sql).map_err(map_sqlite_err)?;
            let mut rows = match &model_filter {
                Some(mid) => stmt.query(params![mid]).map_err(map_sqlite_err)?,
                None => stmt.query([]).map_err(map_sqlite_err)?,
            };
            let mut out = Vec::new();
            while let Some(row) = rows.next().map_err(map_sqlite_err)? {
                if let Some(block) = read_kv_block_row(row).map_err(map_sqlite_err)? {
                    out.push(block);
                }
            }
            Ok(out)
        })
        .await
    }

    pub(crate) async fn upsert_kv_blocks(
        &self,
        blocks: &[KvCacheBlock],
        evicted_hashes: &[KvBlockHash],
    ) -> Result<()> {
        let blocks = blocks.to_vec();
        let evicted = evicted_hashes.to_vec();
        self.with_conn(move |conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(map_sqlite_err)?;
            for h in &evicted {
                tx.execute(
                    "DELETE FROM kv_cache_blocks WHERE block_hash = ?1",
                    params![h.as_str()],
                )
                .map_err(map_sqlite_err)?;
            }
            for b in &blocks {
                tx.execute(
                    "INSERT INTO kv_cache_blocks (
                         block_hash, model_id, mode, tier, parent_hash, amu_id, prefix_hash,
                         scope, token_count, byte_size, warmed_at, last_hit_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                     ON CONFLICT(model_id, block_hash) DO UPDATE SET
                         tier = excluded.tier,
                         last_hit_at = excluded.last_hit_at",
                    params![
                        b.block_hash.as_str(),
                        b.model_id.as_str(),
                        b.mode.as_str(),
                        b.tier.as_str(),
                        b.parent_hash.as_ref().map(KvBlockHash::as_str),
                        b.amu_id.map(|id| id.to_string()),
                        b.prefix_hash.as_ref().map(ContentHash::as_str),
                        b.scope.as_str(),
                        i64::try_from(b.token_count).unwrap_or(i64::MAX),
                        i64::try_from(b.byte_size).unwrap_or(i64::MAX),
                        b.warmed_at.to_rfc3339(),
                        b.last_hit_at.to_rfc3339(),
                    ],
                )
                .map_err(map_sqlite_err)?;
            }
            tx.commit().map_err(map_sqlite_err)?;
            Ok(())
        })
        .await
    }
}

fn read_kv_block_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Option<KvCacheBlock>> {
    let hash_s: String = row.get(0)?;
    let model_s: String = row.get(1)?;
    let mode_s: String = row.get(2)?;
    let tier_s: String = row.get(3)?;
    let parent_s: Option<String> = row.get(4)?;
    let amu_s: Option<String> = row.get(5)?;
    let prefix_s: Option<String> = row.get(6)?;
    let scope_s: String = row.get(7)?;
    let token_count_i: i64 = row.get(8)?;
    let byte_size_i: i64 = row.get(9)?;
    let warmed_s: String = row.get(10)?;
    let last_hit_s: String = row.get(11)?;

    let (Ok(block_hash), Ok(model_id), Some(mode), Some(tier)) = (
        KvBlockHash::parse(&hash_s),
        ModelId::new(model_s),
        KvBlockMode::parse_str(&mode_s),
        KvStorageTier::parse_str(&tier_s),
    ) else {
        return Ok(None);
    };
    let warmed_at = DateTime::parse_from_rfc3339(&warmed_s)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now());
    let last_hit_at = DateTime::parse_from_rfc3339(&last_hit_s)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or(warmed_at);
    Ok(Some(KvCacheBlock {
        block_hash,
        model_id,
        mode,
        tier,
        parent_hash: parent_s.and_then(|s| KvBlockHash::parse(&s).ok()),
        amu_id: amu_s.and_then(|s| AmuId::parse(&s).ok()),
        prefix_hash: prefix_s.and_then(|s| ContentHash::parse(&s).ok()),
        scope: ScopeTag::new(scope_s).unwrap_or_default(),
        token_count: usize::try_from(token_count_i).unwrap_or(0),
        byte_size: usize::try_from(byte_size_i).unwrap_or(0),
        warmed_at,
        last_hit_at,
    }))
}

fn insert_api_key_tx(
    tx: &Transaction<'_>,
    key: &ScopedApiKey,
    key_hash: &str,
) -> rusqlite::Result<()> {
    let scope_strs: Vec<&str> = key.scopes().iter().map(ScopeTag::as_str).collect();
    let scopes_json = serde_json::to_string(&scope_strs).unwrap_or_else(|_| "[]".into());
    tx.execute(
        "INSERT INTO agent_api_keys (
             key_id, agent_id, key_hash, role, scopes_json, created_at, expires_at, revoked_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            key.key_id.as_str(),
            key.agent_id.as_str(),
            key_hash,
            key.role.as_str(),
            scopes_json,
            key.created_at.to_rfc3339(),
            key.expires_at.map(|dt| dt.to_rfc3339()),
            key.revoked_at.map(|dt| dt.to_rfc3339()),
        ],
    )?;
    Ok(())
}

fn read_api_key_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<(ScopedApiKey, String)> {
    let kid_s: String = row.get(0)?;
    let aid_s: String = row.get(1)?;
    let hash_s: String = row.get(2)?;
    let role_s: String = row.get(3)?;
    let scopes_s: String = row.get(4)?;
    let created_s: String = row.get(5)?;
    let expires_s: Option<String> = row.get(6)?;
    let revoked_s: Option<String> = row.get(7)?;

    let key_id = KeyId::parse(&kid_s).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(err))
    })?;
    let agent_id = AgentId::new(aid_s).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(err))
    })?;
    let role = AgentRole::parse_str(&role_s).unwrap_or(AgentRole::Reader);
    let scope_strs: Vec<String> = serde_json::from_str(&scopes_s).unwrap_or_default();
    let scopes: Vec<ScopeTag> = scope_strs
        .into_iter()
        .filter_map(|s| ScopeTag::new(s).ok())
        .collect();
    let created_at = DateTime::parse_from_rfc3339(&created_s)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now());
    let expires_at = expires_s
        .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
        .map(|dt| dt.with_timezone(&Utc));
    let revoked_at = revoked_s
        .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
        .map(|dt| dt.with_timezone(&Utc));

    let key = ScopedApiKey::new(
        key_id, agent_id, role, scopes, created_at, expires_at, revoked_at,
    )
    .map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(4, rusqlite::types::Type::Text, Box::new(err))
    })?;
    Ok((key, hash_s))
}

fn upsert_vault_meta_tx(tx: &Transaction<'_>, meta: &VaultMeta) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO vault_meta (
             singleton_id, schema_version, embedder_id, embedding_dim, updated_at
         ) VALUES (1, ?1, ?2, ?3, ?4)
         ON CONFLICT(singleton_id) DO UPDATE SET
             schema_version = excluded.schema_version,
             embedder_id = excluded.embedder_id,
             embedding_dim = excluded.embedding_dim,
             updated_at = excluded.updated_at",
        params![
            meta.schema_version,
            meta.embedder_id,
            meta.embedding_dim as i64,
            meta.updated_at.to_rfc3339(),
        ],
    )?;
    Ok(())
}

fn load_active_embed_inputs(
    tx: &Transaction<'_>,
) -> rusqlite::Result<Vec<(String, String, String)>> {
    let mut stmt = tx
        .prepare("SELECT amu_id, heading_path, distilled_fact FROM amus WHERE status = 'active'")?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

fn delete_kv_blocks_for_amu_tx(
    tx: &Transaction<'_>,
    amu_id: &str,
) -> rusqlite::Result<Vec<KvBlockHash>> {
    let mut stmt = tx.prepare("SELECT block_hash FROM kv_cache_blocks WHERE amu_id = ?1")?;
    let rows = stmt.query_map(params![amu_id], |r| r.get::<_, String>(0))?;
    let mut hashes = Vec::new();
    for row in rows {
        if let Ok(h) = KvBlockHash::parse(&row?) {
            hashes.push(h);
        }
    }
    drop(stmt);
    tx.execute(
        "DELETE FROM kv_cache_blocks WHERE amu_id = ?1",
        params![amu_id],
    )?;
    Ok(hashes)
}

fn delete_kv_paged_prefix_tx(tx: &Transaction<'_>) -> rusqlite::Result<Vec<KvBlockHash>> {
    let mut stmt =
        tx.prepare("SELECT block_hash FROM kv_cache_blocks WHERE mode = 'paged_prefix'")?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
    let mut hashes = Vec::new();
    for row in rows {
        if let Ok(h) = KvBlockHash::parse(&row?) {
            hashes.push(h);
        }
    }
    drop(stmt);
    tx.execute(
        "DELETE FROM kv_cache_blocks WHERE mode = 'paged_prefix'",
        [],
    )?;
    Ok(hashes)
}

fn delete_amus_for_rel_path(
    tx: &Transaction<'_>,
    rel_path: &str,
) -> rusqlite::Result<(usize, Vec<KvBlockHash>)> {
    let mut stmt = tx.prepare("SELECT amu_id, memory_id, tier FROM amus WHERE rel_path = ?1")?;
    let rows = stmt.query_map(params![rel_path], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    let mut ids = Vec::new();
    let mut had_hot = false;
    for row in rows {
        let (amu_id, mem_id, tier) = row?;
        if tier == "hot" {
            had_hot = true;
        }
        ids.push((amu_id, mem_id));
    }
    drop(stmt);

    let mut evicted = Vec::new();
    for (amu_id, mem_id) in &ids {
        evicted.extend(delete_kv_blocks_for_amu_tx(tx, amu_id)?);
        tx.execute("DELETE FROM amus_fts WHERE amu_id = ?1", params![amu_id])?;
        tx.execute(
            "DELETE FROM amu_edges WHERE source_memory_id = ?1",
            params![mem_id],
        )?;
    }
    if had_hot {
        evicted.extend(delete_kv_paged_prefix_tx(tx)?);
    }

    let deleted = tx.execute("DELETE FROM amus WHERE rel_path = ?1", params![rel_path])?;
    Ok((deleted, evicted))
}

fn insert_amu_tx(
    tx: &Transaction<'_>,
    rel_path: &str,
    amu: &Amu,
    embedding: &[f32],
) -> rusqlite::Result<()> {
    let amu_id_str = amu.amu_id.to_string();
    let mem_id_str = amu.amu_id.to_memory_id().to_string();
    let scope_strs: Vec<&str> = amu.scopes.iter().map(ScopeTag::as_str).collect();
    let scopes_json = serde_json::to_string(&scope_strs).unwrap_or_else(|_| "[]".into());
    let edges_json = serde_json::to_string(&amu.graph_edges).unwrap_or_else(|_| "[]".into());
    let edges_text = amu.graph_edges.join(" ");
    let valid_from_str = amu.valid_from.to_rfc3339();
    let superseded_by_str = amu.superseded_by.map(|id| id.to_string());
    let emb_blob = encode_embedding(embedding);

    tx.execute(
        "INSERT INTO amus (
             amu_id, memory_id, rel_path, tier, kind, status, heading_path,
             distilled_fact, source_pointer, graph_edges_json, valid_from,
             superseded_by, scopes_json, authority_weight, content_hash, embedding, version
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, 1)",
        params![
            amu_id_str,
            mem_id_str,
            rel_path,
            amu.tier.as_str(),
            amu.kind.as_str(),
            amu.status.as_str(),
            amu.heading_path,
            amu.distilled_fact,
            amu.source_pointer.as_str(),
            edges_json,
            valid_from_str,
            superseded_by_str,
            scopes_json,
            amu.authority_weight,
            amu.content_hash.as_str(),
            emb_blob,
        ],
    )?;

    if amu.status == AmuStatus::Active {
        tx.execute(
            "INSERT INTO amus_fts (amu_id, heading_path, distilled_fact, graph_edges_text)
             VALUES (?1, ?2, ?3, ?4)",
            params![amu_id_str, amu.heading_path, amu.distilled_fact, edges_text],
        )?;
    }

    let src_entity = note_entity_for_path(rel_path);
    let invalid_at = if amu.status == AmuStatus::Active {
        None
    } else {
        Some(valid_from_str.clone())
    };
    for dst in &amu.graph_edges {
        let relation_type = if dst.starts_with("tag:") {
            "tagged"
        } else {
            "links_to"
        };
        tx.execute(
            "INSERT OR IGNORE INTO amu_edges (
                 source_memory_id, src_entity, dst_entity, relation_type, valid_at, invalid_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                mem_id_str,
                src_entity,
                dst,
                relation_type,
                valid_from_str,
                invalid_at,
            ],
        )?;
    }

    Ok(())
}

fn supersede_amu_tx(
    tx: &Transaction<'_>,
    old_amu: AmuId,
    new_amu: AmuId,
    at: DateTime<Utc>,
) -> rusqlite::Result<Vec<KvBlockHash>> {
    let old_str = old_amu.to_string();
    let new_str = new_amu.to_string();
    let old_mem_str = old_amu.to_memory_id().to_string();
    let at_str = at.to_rfc3339();

    let old_tier: Option<String> = tx
        .query_row(
            "SELECT tier FROM amus WHERE amu_id = ?1",
            params![old_str],
            |r| r.get(0),
        )
        .optional()?;
    let mut evicted = delete_kv_blocks_for_amu_tx(tx, &old_str)?;
    if old_tier.as_deref() == Some("hot") {
        evicted.extend(delete_kv_paged_prefix_tx(tx)?);
    }

    tx.execute(
        "UPDATE amus SET status = 'superseded', superseded_by = ?1 WHERE amu_id = ?2",
        params![new_str, old_str],
    )?;
    tx.execute("DELETE FROM amus_fts WHERE amu_id = ?1", params![old_str])?;
    tx.execute(
        "UPDATE amu_edges SET invalid_at = ?1 WHERE source_memory_id = ?2 AND invalid_at IS NULL",
        params![at_str, old_mem_str],
    )?;
    Ok(evicted)
}

fn load_active_amus(conn: &Connection) -> rusqlite::Result<Vec<(Amu, Vec<f32>)>> {
    let mut stmt = conn.prepare(
        "SELECT amu_id, tier, kind, status, heading_path, distilled_fact, source_pointer,
                graph_edges_json, valid_from, superseded_by, scopes_json, authority_weight,
                content_hash, embedding
         FROM amus
         WHERE status = 'active'",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, String>(5)?,
            row.get::<_, String>(6)?,
            row.get::<_, String>(7)?,
            row.get::<_, String>(8)?,
            row.get::<_, Option<String>>(9)?,
            row.get::<_, String>(10)?,
            row.get::<_, f32>(11)?,
            row.get::<_, String>(12)?,
            row.get::<_, Vec<u8>>(13)?,
        ))
    })?;

    let mut out = Vec::new();
    for row in rows {
        let (
            aid_s,
            tier_s,
            kind_s,
            status_s,
            heading_path,
            distilled_fact,
            ptr_s,
            edges_s,
            valid_s,
            sup_s,
            scopes_s,
            authority_weight,
            hash_s,
            emb_blob,
        ) = row?;

        let Ok(amu_id) = AmuId::parse(&aid_s) else {
            continue;
        };
        let Ok(source_pointer) = SourcePointer::parse(&ptr_s) else {
            continue;
        };
        let Ok(content_hash) = ContentHash::parse(&hash_s) else {
            continue;
        };
        let valid_from = DateTime::parse_from_rfc3339(&valid_s)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now());
        let scope_strs: Vec<String> = serde_json::from_str(&scopes_s).unwrap_or_default();
        let scopes: Vec<ScopeTag> = scope_strs
            .into_iter()
            .filter_map(|s| ScopeTag::new(s).ok())
            .collect();
        let graph_edges: Vec<String> = serde_json::from_str(&edges_s).unwrap_or_default();

        out.push((
            Amu {
                amu_id,
                tier: VaultTier::parse_str(&tier_s).unwrap_or_default(),
                kind: AmuKind::parse_str(&kind_s).unwrap_or_default(),
                status: AmuStatus::parse_str(&status_s).unwrap_or_default(),
                heading_path,
                distilled_fact,
                source_pointer,
                graph_edges,
                valid_from,
                superseded_by: sup_s.and_then(|s| AmuId::parse(&s).ok()),
                scopes,
                authority_weight,
                content_hash,
            },
            decode_embedding(&emb_blob),
        ));
    }
    Ok(out)
}

fn build_recall_subgraph(
    conn: &Connection,
    query_text: &str,
    fts_candidates: &[Candidate],
    vector_candidates: &[Candidate],
    amus_by_memory_id: &HashMap<MemoryId, Amu>,
    active_mem_ids: &HashSet<String>,
) -> rusqlite::Result<(Vec<EntityId>, Subgraph)> {
    let mut seed_set: HashSet<EntityId> = HashSet::new();

    for cand in fts_candidates
        .iter()
        .take(4)
        .chain(vector_candidates.iter().take(4))
    {
        if let Some(amu) = amus_by_memory_id.get(&cand.item.id) {
            if let Ok(eid) = EntityId::new(note_entity_for_path(amu.source_pointer.relative_path()))
            {
                seed_set.insert(eid);
            }
            for edge in &amu.graph_edges {
                if let Ok(eid) = EntityId::new(edge.clone()) {
                    seed_set.insert(eid);
                }
            }
        }
    }

    for token in
        query_text.split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-' && c != '/')
    {
        let lower = token.trim().to_ascii_lowercase();
        if lower.len() < 2 {
            continue;
        }
        if let Ok(tag_eid) = EntityId::new(format!("tag:{lower}")) {
            seed_set.insert(tag_eid);
        }
    }

    if seed_set.is_empty() {
        return Ok((Vec::new(), Subgraph::default()));
    }

    let seed_entities: Vec<EntityId> = seed_set.into_iter().collect();

    let mut stmt = conn.prepare(
        "SELECT source_memory_id, src_entity, dst_entity, relation_type, valid_at
         FROM amu_edges
         WHERE invalid_at IS NULL",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
        ))
    })?;

    let mut all_edges = Vec::new();
    for row in rows {
        let (mem_s, src_s, dst_s, rel_type, valid_s) = row?;
        if !active_mem_ids.contains(&mem_s) {
            continue;
        }
        let Ok(mem_uuid) = uuid::Uuid::parse_str(&mem_s) else {
            continue;
        };
        let (Ok(src), Ok(dst)) = (EntityId::new(src_s), EntityId::new(dst_s)) else {
            continue;
        };
        let valid_at = DateTime::parse_from_rfc3339(&valid_s)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now());
        all_edges.push(GraphEdge {
            source: src,
            target: dst,
            relation_type: rel_type,
            source_memory_id: MemoryId::from_uuid(mem_uuid),
            valid_at,
            invalid_at: None,
        });
    }

    let mut visited: HashSet<EntityId> = seed_entities.iter().cloned().collect();
    let mut queue: VecDeque<(EntityId, u8)> =
        seed_entities.iter().cloned().map(|e| (e, 0)).collect();
    let mut collected_edges = Vec::new();

    while let Some((node, depth)) = queue.pop_front() {
        if depth >= 2 {
            continue;
        }
        for edge in &all_edges {
            if edge.source == node || edge.target == node {
                collected_edges.push(edge.clone());
                if edge.relation_type == "links_to" {
                    collected_edges.push(GraphEdge {
                        source: edge.target.clone(),
                        target: edge.source.clone(),
                        relation_type: "backlink".into(),
                        source_memory_id: edge.source_memory_id,
                        valid_at: edge.valid_at,
                        invalid_at: None,
                    });
                }
                let next = if edge.source == node {
                    edge.target.clone()
                } else {
                    edge.source.clone()
                };
                if visited.insert(next.clone()) {
                    queue.push_back((next, depth + 1));
                }
            }
        }
    }

    let mut seen_entities = HashSet::new();
    let mut entities = Vec::new();
    for edge in &collected_edges {
        for eid in [&edge.source, &edge.target] {
            if seen_entities.insert(eid.clone()) {
                let entity_type = if eid.as_str().starts_with("tag:") {
                    "Tag"
                } else {
                    "Note"
                };
                entities.push(Entity {
                    id: eid.clone(),
                    name: eid.as_str().to_string(),
                    entity_type: entity_type.to_string(),
                });
            }
        }
    }

    Ok((
        seed_entities,
        Subgraph {
            entities,
            edges: collected_edges,
        },
    ))
}

pub(crate) fn to_memory_item(amu: &Amu, project_id: &ProjectId) -> MemoryItem {
    let tier = match amu.kind {
        AmuKind::Profile => MemoryTier::Profile,
        AmuKind::Rule | AmuKind::Decision => MemoryTier::Procedural,
        AmuKind::Spec => MemoryTier::Semantic,
        AmuKind::Fleeting => MemoryTier::Episodic,
    };
    MemoryItem {
        id: amu.amu_id.to_memory_id(),
        project_id: project_id.clone(),
        content: amu.distilled_fact.clone(),
        embedding: None,
        entities: Vec::new(),
        relations: Vec::new(),
        tier,
        scope: MemoryScope::Project,
        idempotency_key: None,
        metadata: serde_json::json!({}),
        version: 1,
        created_at: amu.valid_from,
        updated_at: amu.valid_from,
    }
}

fn note_entity_for_path(rel_path: &str) -> String {
    let lower = rel_path.trim().replace('\\', "/").to_ascii_lowercase();
    let stem = lower.strip_suffix(".md").unwrap_or(&lower);
    format!("note:{stem}")
}

fn scope_policy_matches(scopes: &[ScopeTag], policy: &ScopePolicy) -> bool {
    match policy {
        ScopePolicy::Unrestricted => true,
        ScopePolicy::Allowed(allowed) => {
            if allowed.iter().any(|s| s.as_str() == "*") {
                return true;
            }
            scopes
                .iter()
                .any(|s| s.as_str() == "global" || allowed.contains(s))
        }
    }
}

fn build_fts5_query(query: &str) -> Option<String> {
    let terms: Vec<String> = query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(|s| format!("\"{}\"", s.to_ascii_lowercase()))
        .collect();
    if terms.is_empty() {
        None
    } else {
        Some(terms.join(" OR "))
    }
}

fn encode_embedding(vec: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(vec.len() * 4);
    for val in vec {
        out.extend_from_slice(&val.to_le_bytes());
    }
    out
}

fn decode_embedding(bytes: &[u8]) -> Vec<f32> {
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|chunk| f32::from_le_bytes(*chunk))
        .collect()
}

fn now_epoch_us() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as i64
}

fn permanent_err(message: impl Into<String>) -> AetherError {
    AetherError::Permanent {
        connector: CONNECTOR_NAME,
        message: message.into(),
    }
}

fn map_sqlite_err(err: rusqlite::Error) -> AetherError {
    if let rusqlite::Error::SqliteFailure(sqlite_err, _) = &err {
        if matches!(
            sqlite_err.code,
            ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked
        ) {
            return AetherError::Transient {
                connector: CONNECTOR_NAME,
                message: err.to_string(),
            };
        }
    }
    permanent_err(err.to_string())
}
