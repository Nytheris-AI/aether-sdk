use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use aether_adapters::{InMemoryBackend, Traced};
use aether_core::{
    DistanceMetric, MemoryId, MemoryInput, MemoryUpdate, ProjectId, RecallMode, RecallQuery,
    VectorStore, WriteDurability,
};
use aether_sdk::MemoryManager;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Metadata, Subscriber};

#[derive(Debug, Clone)]
struct RecordedSpan {
    name: &'static str,
    parent_id: Option<u64>,
    fields: HashMap<String, String>,
}

#[derive(Default, Clone)]
struct SpanCollector {
    spans: Arc<Mutex<HashMap<u64, RecordedSpan>>>,
    next_id: Arc<Mutex<u64>>,
    stack: Arc<Mutex<Vec<u64>>>,
}

impl SpanCollector {
    fn snapshot(&self) -> Vec<RecordedSpan> {
        self.spans.lock().unwrap().values().cloned().collect()
    }

    fn find_by_name(&self, name: &str) -> Vec<RecordedSpan> {
        self.snapshot()
            .into_iter()
            .filter(|s| s.name == name)
            .collect()
    }

    fn has_parent(&self, child: &RecordedSpan, parent_name: &str) -> bool {
        let spans = self.spans.lock().unwrap();
        let mut curr = child.parent_id;
        while let Some(pid) = curr {
            if let Some(parent) = spans.get(&pid) {
                if parent.name == parent_name {
                    return true;
                }
                curr = parent.parent_id;
            } else {
                break;
            }
        }
        false
    }
}

struct FieldVisitor<'a>(&'a mut HashMap<String, String>);

impl Visit for FieldVisitor<'_> {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0
            .insert(field.name().to_string(), format!("{value:?}"));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.insert(field.name().to_string(), value.to_string());
    }
}

impl Subscriber for SpanCollector {
    fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, attrs: &Attributes<'_>) -> Id {
        let mut next = self.next_id.lock().unwrap();
        *next += 1;
        let id = *next;

        let mut fields = HashMap::new();
        attrs.record(&mut FieldVisitor(&mut fields));

        let parent_id = if attrs.is_contextual() {
            self.stack.lock().unwrap().last().copied()
        } else {
            attrs.parent().map(|p| p.into_u64())
        };

        self.spans.lock().unwrap().insert(
            id,
            RecordedSpan {
                name: attrs.metadata().name(),
                parent_id,
                fields,
            },
        );
        Id::from_u64(id)
    }

    fn record(&self, span: &Id, values: &Record<'_>) {
        if let Some(recorded) = self.spans.lock().unwrap().get_mut(&span.into_u64()) {
            values.record(&mut FieldVisitor(&mut recorded.fields));
        }
    }

    fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

    fn event(&self, _event: &Event<'_>) {}

    fn enter(&self, span: &Id) {
        self.stack.lock().unwrap().push(span.into_u64());
    }

    fn exit(&self, span: &Id) {
        let mut stack = self.stack.lock().unwrap();
        if let Some(pos) = stack.iter().rposition(|id| *id == span.into_u64()) {
            stack.remove(pos);
        }
    }
}

#[test]
fn otel_traces_connect_memory_manager_cortex_and_adapters() {
    let collector = SpanCollector::default();
    let _guard = tracing::subscriber::set_default(collector.clone());

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    rt.block_on(async {
        let raw = Arc::new(InMemoryBackend::new());
        let kv = Arc::new(Traced::new(raw.clone()));
        let vec_store = Arc::new(Traced::new(raw.clone()));
        let graph = Arc::new(Traced::new(raw.clone()));

        let pid = ProjectId::new("proj-otel").unwrap();
        vec_store
            .initialize(&pid, 3, DistanceMetric::Cosine)
            .await
            .unwrap();

        let manager = MemoryManager::builder(kv)
            .vector(vec_store)
            .graph(graph)
            .build();

        // 1. Remember (WaitFlush) -> should emit root aether.remember -> aether.cortex.index_record -> connector_op
        let mut input = MemoryInput::new(pid.clone(), "OpenTelemetry trace propagation test");
        input.embedding = Some(vec![1.0, 0.0, 0.0]);
        input.durability = WriteDurability::WaitFlush;
        manager.remember(input).await.unwrap();

        // 2. Recall -> should emit root aether.recall -> aether.cortex.vector_search -> connector_op
        let mut query = RecallQuery::new(pid.clone(), "trace propagation");
        query.query_vector = Some(vec![1.0, 0.0, 0.0]);
        query.mode = RecallMode::Vector;
        manager.recall(query).await.unwrap();

        // 3. Failing update -> should emit aether.update with otel.status_code = "ERROR"
        let missing_id = MemoryId::new();
        let _ = manager
            .update(
                &pid,
                missing_id,
                MemoryUpdate {
                    content: Some("won't exist".into()),
                    embedding: None,
                    metadata: None,
                    entities: None,
                    relations: None,
                    expected_version: None,
                },
            )
            .await;
    });

    // Verify root spans and otel.status_code
    let remember_spans = collector.find_by_name("aether.remember");
    assert_eq!(remember_spans.len(), 1);
    assert_eq!(
        remember_spans[0]
            .fields
            .get("otel.status_code")
            .map(String::as_str),
        Some("OK")
    );

    let recall_spans = collector.find_by_name("aether.recall");
    assert_eq!(recall_spans.len(), 1);
    assert_eq!(
        recall_spans[0]
            .fields
            .get("otel.status_code")
            .map(String::as_str),
        Some("OK")
    );

    let update_spans = collector.find_by_name("aether.update");
    assert_eq!(update_spans.len(), 1);
    assert_eq!(
        update_spans[0]
            .fields
            .get("otel.status_code")
            .map(String::as_str),
        Some("ERROR")
    );

    // Verify cortex child spans
    let index_spans = collector.find_by_name("aether.cortex.index_record");
    assert_eq!(index_spans.len(), 1);
    assert!(collector.has_parent(&index_spans[0], "aether.remember"));

    let search_spans = collector.find_by_name("aether.cortex.vector_search");
    assert_eq!(search_spans.len(), 1);
    assert!(collector.has_parent(&search_spans[0], "aether.recall"));

    // Verify connector_op spans are no longer orphans (nested under aether.remember / aether.recall)
    let connector_spans = collector.find_by_name("connector_op");
    assert!(connector_spans
        .iter()
        .any(|s| collector.has_parent(s, "aether.remember")));
    assert!(connector_spans
        .iter()
        .any(|s| collector.has_parent(s, "aether.recall")));
}

#[test]
fn zero_boilerplate_memory_manager_emits_otel_client_spans() {
    let collector = SpanCollector::default();
    let _guard = tracing::subscriber::set_default(collector.clone());

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    rt.block_on(async {
        let raw = Arc::new(InMemoryBackend::new());
        let pid = ProjectId::new("proj-zero-bp").unwrap();
        raw.initialize(&pid, 3, DistanceMetric::Cosine)
            .await
            .unwrap();

        // No manual Traced::new(...) wrapping!
        let manager = MemoryManager::builder(raw.clone())
            .vector(raw.clone())
            .graph(raw)
            .build();

        let mut input = MemoryInput::new(pid.clone(), "Zero boilerplate telemetry");
        input.embedding = Some(vec![1.0, 0.0, 0.0]);
        input.durability = WriteDurability::WaitFlush;
        let receipt = manager.remember(input).await.unwrap();

        // Trigger a CAS conflict at the RecordStore level so connector_op itself fails
        let _ = manager
            .update(
                &pid,
                receipt.memory_id,
                MemoryUpdate {
                    content: Some("conflict".into()),
                    embedding: None,
                    metadata: None,
                    entities: None,
                    relations: None,
                    expected_version: Some(999),
                },
            )
            .await;
    });

    let connector_spans = collector.find_by_name("connector_op");
    assert!(
        !connector_spans.is_empty(),
        "MemoryManager::new should auto-wrap stores with Traced"
    );
    assert!(connector_spans
        .iter()
        .any(|s| collector.has_parent(s, "aether.remember")));

    let create_span = connector_spans
        .iter()
        .find(|s| s.fields.get("op").map(String::as_str) == Some("record_create"))
        .expect("expected record_create connector_op span");
    assert_eq!(
        create_span.fields.get("otel.kind").map(String::as_str),
        Some("client")
    );
    assert_eq!(
        create_span.fields.get("db.system").map(String::as_str),
        Some("in_memory")
    );
    assert_eq!(
        create_span
            .fields
            .get("otel.status_code")
            .map(String::as_str),
        Some("OK")
    );
}
