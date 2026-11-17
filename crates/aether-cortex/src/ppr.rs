//! Personalized PageRank and memory-level graph scoring over retrieved subgraphs.

use std::collections::{HashMap, HashSet};

use aether_core::{EntityId, Subgraph};
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::EdgeRef;

/// Personalized PageRank over a retrieved `Subgraph` using `petgraph`.
pub struct PersonalizedPageRank {
    pub damping: f32,
    pub iterations: usize,
}

impl Default for PersonalizedPageRank {
    fn default() -> Self {
        Self {
            damping: 0.85,
            iterations: 20,
        }
    }
}

impl PersonalizedPageRank {
    pub fn rank(&self, subgraph: &Subgraph, seeds: &[EntityId]) -> Vec<(EntityId, f32)> {
        let _span = tracing::debug_span!(
            "aether.cortex.ppr",
            iterations = self.iterations,
            seed_count = seeds.len(),
            entity_count = subgraph.entities.len(),
            edge_count = subgraph.edges.len(),
        )
        .entered();
        if subgraph.entities.is_empty() {
            return Vec::new();
        }

        let mut graph = DiGraph::<EntityId, ()>::new();
        let mut indices: HashMap<EntityId, NodeIndex> = HashMap::new();
        for entity in &subgraph.entities {
            let idx = graph.add_node(entity.id.clone());
            indices.insert(entity.id.clone(), idx);
        }

        for edge in &subgraph.edges {
            if let (Some(&src), Some(&dst)) = (indices.get(&edge.source), indices.get(&edge.target))
            {
                graph.add_edge(src, dst, ());
            }
        }

        let n = graph.node_count();
        let seed_set: HashSet<&EntityId> = seeds.iter().collect();
        let mut personalization = vec![0.0f32; n];
        let seed_hits: usize = graph
            .node_indices()
            .filter(|&idx| seed_set.contains(&graph[idx]))
            .count();

        if seed_hits > 0 {
            let weight = 1.0 / seed_hits as f32;
            for idx in graph.node_indices() {
                if seed_set.contains(&graph[idx]) {
                    personalization[idx.index()] = weight;
                }
            }
        } else {
            personalization.fill(1.0 / n as f32);
        }

        let mut scores = personalization.clone();
        for _ in 0..self.iterations {
            let mut next = vec![0.0f32; n];
            for idx in graph.node_indices() {
                let out_deg = graph.edges(idx).count();
                if out_deg == 0 {
                    for (j, p) in personalization.iter().enumerate() {
                        next[j] += self.damping * scores[idx.index()] * p;
                    }
                } else {
                    let share = self.damping * scores[idx.index()] / out_deg as f32;
                    for edge in graph.edges(idx) {
                        next[edge.target().index()] += share;
                    }
                }
            }
            for (j, p) in personalization.iter().enumerate() {
                next[j] += (1.0 - self.damping) * p;
            }
            scores = next;
        }

        let mut ranked: Vec<(EntityId, f32)> = graph
            .node_indices()
            .map(|idx| (graph[idx].clone(), scores[idx.index()]))
            .collect();
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
        ranked
    }
}
