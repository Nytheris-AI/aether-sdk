//! Level 3: Compose hybrid vector + graph recall and Reciprocal Rank Fusion directly.

use chrono::Utc;
use muninn_aether_adapters::InMemoryBackend;
use muninn_aether_core::{
    DistanceMetric, Entity, EntityId, GraphStore, MemoryInput, MemoryItem, ProjectId, RecordStore,
    Relation, TemporalFilter, VectorRecord, VectorStore,
};
use muninn_aether_cortex::recall::{self, RRF_K};
use muninn_aether_cortex::{index_metadata, PersonalizedPageRank};

#[tokio::main]
async fn main() -> muninn_aether_core::Result<()> {
    let backend = InMemoryBackend::new();
    let ns = ProjectId::new("level3-demo")?;
    backend.initialize(&ns, 2, DistanceMetric::Cosine).await?;

    let alice = Entity {
        id: EntityId::new("person:alice")?,
        name: "Alice".into(),
        entity_type: "Person".into(),
    };
    let bob = Entity {
        id: EntityId::new("person:bob")?,
        name: "Bob".into(),
        entity_type: "Person".into(),
    };

    let mut m1_in = MemoryInput::new(ns.clone(), "Alice works with Bob");
    m1_in.embedding = Some(vec![1.0, 0.0]);
    m1_in.entities = vec![alice.clone()];
    let m1 = MemoryItem::from_input(m1_in, 1);
    backend.create(&ns, &m1, None).await?;
    backend
        .upsert_batch(
            &ns,
            &[VectorRecord {
                id: m1.id,
                vector: vec![1.0, 0.0],
                metadata: index_metadata(&m1),
            }],
        )
        .await?;

    let mut m2_in = MemoryInput::new(ns.clone(), "Bob leads Infra");
    m2_in.entities = vec![bob.clone()];
    m2_in.relations = vec![Relation {
        source: alice.id.clone(),
        target: bob.id.clone(),
        relation_type: "MENTORS".into(),
        valid_at: Utc::now(),
        invalid_at: None,
    }];
    let m2 = MemoryItem::from_input(m2_in, 1);
    backend.create(&ns, &m2, None).await?;
    backend.upsert_entities(&ns, &[alice.clone(), bob]).await?;
    backend.upsert_relations(&ns, m2.id, &m2.relations).await?;

    let vec_cands = recall::vector_search(&backend, &backend, &ns, &[1.0, 0.0], 5, None).await?;
    let (graph_cands, subgraph) = recall::graph_search(
        &backend,
        &backend,
        &ns,
        &[alice.id],
        2,
        10,
        &TemporalFilter::default(),
        &PersonalizedPageRank::default(),
    )
    .await?;

    let fused = recall::rrf(&[vec_cands, graph_cands], RRF_K);
    println!(
        "level 3 fused hits: {}, subgraph edges: {}",
        fused.len(),
        subgraph.edges.len()
    );
    Ok(())
}
