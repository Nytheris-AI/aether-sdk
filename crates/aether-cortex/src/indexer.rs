//! Sync engine deriving vector and graph indexes from versioned `RecordStore` entries.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use aether_core::{
    GraphStore, MemoryId, MemoryItem, PendingRecord, RecordStore, Result, VectorRecord, VectorStore,
};
use chrono::Utc;
use serde_json::Value;
use tokio::sync::Notify;
use tracing::Instrument;

/// Metadata written to the vector index: caller metadata object plus `_aether_tier`.
pub fn index_metadata(item: &MemoryItem) -> Value {
    let mut meta = match &item.metadata {
        Value::Object(map) => map.clone(),
        _ => serde_json::Map::new(),
    };
    if let Ok(tier_val) = serde_json::to_value(item.tier) {
        meta.insert("_aether_tier".into(), tier_val);
    }
    Value::Object(meta)
}

/// Summary returned by [`Indexer::run_pass`].
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PassReport {
    pub indexed: usize,
    pub failed: usize,
}

/// Pulls pending records from [`RecordStore`] and reconciles optional vector and graph indexes.
pub struct Indexer {
    record: Arc<dyn RecordStore>,
    vector: Option<Arc<dyn VectorStore>>,
    graph: Option<Arc<dyn GraphStore>>,
    notify: Arc<Notify>,
}

impl Indexer {
    /// Creates an indexer backed by `record` with no derived indexes configured yet.
    pub fn new(record: Arc<dyn RecordStore>) -> Self {
        Self {
            record,
            vector: None,
            graph: None,
            notify: Arc::new(Notify::new()),
        }
    }

    /// Attaches a derived [`VectorStore`] index.
    pub fn with_vector(mut self, vector: Arc<dyn VectorStore>) -> Self {
        self.vector = Some(vector);
        self
    }

    /// Attaches a derived [`GraphStore`] index.
    pub fn with_graph(mut self, graph: Arc<dyn GraphStore>) -> Self {
        self.graph = Some(graph);
        self
    }

    /// Wakes an active background loop spawned via [`Indexer::spawn`].
    pub fn notify(&self) {
        self.notify.notify_one();
    }

    /// Applies one pending record (live upsert or tombstone retraction) and marks its version indexed.
    pub async fn apply(&self, pending: &PendingRecord) -> Result<()> {
        let item = &pending.item;
        let span = tracing::info_span!(
            "aether.cortex.index_record",
            project_id = %item.project_id,
            memory_id = %item.id,
            tombstone = pending.tombstone,
        );
        async move {
            if pending.tombstone {
                if let Some(vector) = &self.vector {
                    vector.delete(&item.project_id, &[item.id]).await?;
                }
                if let Some(graph) = &self.graph {
                    graph
                        .retract_memory_facts(&item.project_id, item.id, Utc::now())
                        .await?;
                }
            } else {
                if let Some(vector) = &self.vector {
                    if let Some(vec) = &item.embedding {
                        let rec = VectorRecord {
                            id: item.id,
                            vector: vec.clone(),
                            metadata: index_metadata(item),
                        };
                        vector
                            .upsert_batch(&item.project_id, std::slice::from_ref(&rec))
                            .await?;
                    } else {
                        vector.delete(&item.project_id, &[item.id]).await?;
                    }
                }
                if let Some(graph) = &self.graph {
                    graph
                        .retract_memory_facts(&item.project_id, item.id, Utc::now())
                        .await?;
                    if !item.entities.is_empty() {
                        graph
                            .upsert_entities(&item.project_id, &item.entities)
                            .await?;
                    }
                    if !item.relations.is_empty() {
                        graph
                            .upsert_relations(&item.project_id, item.id, &item.relations)
                            .await?;
                    }
                }
            }
            self.record
                .mark_indexed(&item.project_id, item.id, item.version)
                .await
        }
        .instrument(span)
        .await
    }

    /// Reconciles all pending records in batches, continuing past per-record errors.
    pub async fn run_pass(&self) -> Result<PassReport> {
        let span = tracing::info_span!(
            "aether.cortex.index_pass",
            indexed = tracing::field::Empty,
            failed = tracing::field::Empty,
        );
        async move {
            let mut report = PassReport::default();
            let mut failed_ids: HashSet<MemoryId> = HashSet::new();
            let mut last_err = None;

            loop {
                let batch = self.record.pending(256).await?;
                if batch.is_empty() {
                    break;
                }
                let mut progressed = false;
                for pending in &batch {
                    if failed_ids.contains(&pending.item.id) {
                        continue;
                    }
                    progressed = true;
                    match self.apply(pending).await {
                        Ok(()) => report.indexed += 1,
                        Err(err) => {
                            report.failed += 1;
                            failed_ids.insert(pending.item.id);
                            last_err = Some(err);
                        }
                    }
                }
                if !progressed || batch.len() < 256 {
                    break;
                }
            }

            let cur = tracing::Span::current();
            cur.record("indexed", report.indexed);
            cur.record("failed", report.failed);

            if let Some(err) = last_err {
                return Err(err);
            }
            Ok(report)
        }
        .instrument(span)
        .await
    }

    /// Spawns an opt-in background loop that exits automatically when the `Arc<Indexer>` drops.
    pub fn spawn(self: &Arc<Self>, interval: Duration) -> tokio::task::JoinHandle<()> {
        let weak = Arc::downgrade(self);
        let notify = Arc::clone(&self.notify);
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = notify.notified() => {}
                    _ = tokio::time::sleep(interval) => {}
                }
                let Some(indexer) = weak.upgrade() else {
                    break;
                };
                if let Err(err) = indexer.run_pass().await {
                    tracing::warn!(error = %err, "background indexer pass failed");
                }
            }
        })
    }
}
