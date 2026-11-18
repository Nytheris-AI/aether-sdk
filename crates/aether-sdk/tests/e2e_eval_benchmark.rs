mod common;

use std::collections::{BTreeMap, HashMap};

use aether_core::{
    ConsistencyMode, DistanceMetric, FilterExpr, FilterValue, MemoryId, ProjectId, RecallMode,
    RecallQuery, VectorStore,
};
use common::{
    build_resilient_manager, embed_text, fixture_path, transform_locomo_sample, LocomoSample,
    EMBED_DIM,
};

#[derive(Default)]
struct CategoryMetrics {
    count: usize,
    sum_recall_at_5: f64,
    sum_recall_at_10: f64,
    sum_mrr: f64,
}

impl CategoryMetrics {
    fn record(&mut self, r5: f64, r10: f64, mrr: f64) {
        self.count += 1;
        self.sum_recall_at_5 += r5;
        self.sum_recall_at_10 += r10;
        self.sum_mrr += mrr;
    }

    fn avg_r5(&self) -> f64 {
        self.sum_recall_at_5 / self.count.max(1) as f64
    }

    fn avg_r10(&self) -> f64 {
        self.sum_recall_at_10 / self.count.max(1) as f64
    }

    fn avg_mrr(&self) -> f64 {
        self.sum_mrr / self.count.max(1) as f64
    }
}

#[tokio::test]
#[ignore = "eval benchmark: run explicitly with --ignored --nocapture"]
async fn eval_locomo_recall_at_k_and_mrr() {
    let raw_json = std::fs::read_to_string(fixture_path("locomo_sample_2conv.json")).unwrap();
    let samples: Vec<LocomoSample> = serde_json::from_str(&raw_json).unwrap();
    assert_eq!(samples.len(), 2);

    let (backend, manager) = build_resilient_manager();
    let mut ingested_by_dia: HashMap<(String, String), MemoryId> = HashMap::new();

    for sample in &samples {
        let pid = ProjectId::new(sample.sample_id.clone()).unwrap();
        backend
            .initialize(&pid, EMBED_DIM, DistanceMetric::Cosine)
            .await
            .unwrap();

        for input in transform_locomo_sample(sample) {
            let dia_id = input
                .metadata
                .get("dia_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let source_type = input
                .metadata
                .get("source_type")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let receipt = manager.remember(input).await.unwrap();
            if source_type == "locomo_turn" && !dia_id.is_empty() {
                ingested_by_dia.insert((sample.sample_id.clone(), dia_id), receipt.memory_id);
            }
        }
    }
    manager.flush().await.unwrap();

    let mut by_category: BTreeMap<u8, CategoryMetrics> = BTreeMap::new();
    let mut overall = CategoryMetrics::default();

    for sample in &samples {
        let pid = ProjectId::new(sample.sample_id.clone()).unwrap();
        for qa in sample.qa.iter().filter(|q| !q.evidence.is_empty()) {
            let expected_ids: Vec<MemoryId> = qa
                .evidence
                .iter()
                .filter_map(|ev| {
                    ingested_by_dia
                        .get(&(sample.sample_id.clone(), ev.clone()))
                        .copied()
                })
                .collect();
            if expected_ids.is_empty() {
                continue;
            }

            let mut query = RecallQuery::new(pid.clone(), qa.question.clone());
            query.query_vector = Some(embed_text(&qa.question));
            query.mode = RecallMode::Vector;
            query.top_k = 10;
            query.consistency = ConsistencyMode::ReadYourWrites;
            query.filters = Some(FilterExpr::Eq {
                field: "sample_id".into(),
                value: FilterValue::String(sample.sample_id.clone()),
            });

            let result = manager.recall(query).await.unwrap();
            let top5_hits = result
                .hits
                .iter()
                .take(5)
                .filter(|h| expected_ids.contains(&h.memory_id))
                .count();
            let top10_hits = result
                .hits
                .iter()
                .take(10)
                .filter(|h| expected_ids.contains(&h.memory_id))
                .count();

            let r5 = top5_hits as f64 / expected_ids.len() as f64;
            let r10 = top10_hits as f64 / expected_ids.len() as f64;
            let mrr = result
                .hits
                .iter()
                .take(10)
                .position(|h| expected_ids.contains(&h.memory_id))
                .map_or(0.0, |idx| 1.0 / (idx + 1) as f64);

            by_category
                .entry(qa.category)
                .or_default()
                .record(r5, r10, mrr);
            overall.record(r5, r10, mrr);
        }
    }

    println!("\n=== LoCoMo Retrieval Eval (conv-26 + conv-30, hash-64d) ===");
    println!(
        "{:<12} {:>6} {:>10} {:>10} {:>10}",
        "Category", "Count", "Recall@5", "Recall@10", "MRR"
    );
    for (cat, m) in &by_category {
        println!(
            "{:<12} {:>6} {:>10.4} {:>10.4} {:>10.4}",
            format!("Cat {cat}"),
            m.count,
            m.avg_r5(),
            m.avg_r10(),
            m.avg_mrr()
        );
    }
    println!(
        "{:<12} {:>6} {:>10.4} {:>10.4} {:>10.4}",
        "Overall",
        overall.count,
        overall.avg_r5(),
        overall.avg_r10(),
        overall.avg_mrr()
    );

    assert!(overall.count > 0, "expected at least one QA probe");
    manager.shutdown().await.unwrap();
}
