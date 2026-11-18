use aether_core::{Entity, EntityId, GraphEdge, MemoryId, Subgraph};
use aether_cortex::PersonalizedPageRank;
use chrono::Utc;

#[test]
fn personalized_page_rank_ranks_seed_neighborhood_higher() {
    let alice = EntityId::new("person:alice").unwrap();
    let turbopuffer = EntityId::new("db:turbopuffer").unwrap();
    let rust = EntityId::new("lang:rust").unwrap();
    let isolated = EntityId::new("other:node").unwrap();

    let now = Utc::now();
    let mid = MemoryId::new();
    let subgraph = Subgraph {
        entities: vec![
            Entity {
                id: alice.clone(),
                name: "Alice".into(),
                entity_type: "Person".into(),
            },
            Entity {
                id: turbopuffer.clone(),
                name: "Turbopuffer".into(),
                entity_type: "Database".into(),
            },
            Entity {
                id: rust.clone(),
                name: "Rust".into(),
                entity_type: "Language".into(),
            },
            Entity {
                id: isolated.clone(),
                name: "Isolated".into(),
                entity_type: "Unknown".into(),
            },
        ],
        edges: vec![
            GraphEdge {
                source: alice.clone(),
                target: turbopuffer.clone(),
                relation_type: "USES".into(),
                source_memory_id: mid,
                valid_at: now,
                invalid_at: None,
            },
            GraphEdge {
                source: turbopuffer.clone(),
                target: rust.clone(),
                relation_type: "WRITTEN_FOR".into(),
                source_memory_id: mid,
                valid_at: now,
                invalid_at: None,
            },
        ],
    };

    let ranks = PersonalizedPageRank::default().rank(&subgraph, std::slice::from_ref(&alice));
    let score_of = |id: &EntityId| {
        ranks
            .iter()
            .find(|(e, _)| e == id)
            .map(|(_, s)| *s)
            .unwrap_or(0.0)
    };

    assert!(score_of(&alice) > score_of(&turbopuffer));
    assert!(score_of(&turbopuffer) > score_of(&isolated));
}
