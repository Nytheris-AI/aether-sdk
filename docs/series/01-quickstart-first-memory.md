---
id: 01-quickstart-first-memory
slug: /series/01-quickstart-first-memory
sidebar_position: 1
---

# Part 1: Quickstart & Your First Memory

In this tutorial, you build a small Rust program that stores and queries durable memory records and dense vectors with `InMemoryBackend`. You also run the official Level 1 example from the workspace.

**Before you begin.** Install Rust `1.89` or newer (`edition = "2021"`). Clone the repository so you can run the workspace examples alongside your own binary.

## Workspace layout and model-free boundary

Project Aether splits memory infrastructure across five workspace crates under `crates/`.

| Crate | Package name | Responsibility |
| --- | --- | --- |
| `crates/aether-core` | `nytheris-aether-core` | Domain types, validated IDs, scoring, filters, and storage traits |
| `crates/aether-cortex` | `nytheris-aether-cortex` | `Indexer` sync engine, `PersonalizedPageRank`, and `recall` steps |
| `crates/aether-adapters` | `nytheris-aether-adapters` | `InMemoryBackend`, middleware wrappers, contract tests, and 7 drivers |
| `crates/aether-sdk` | `nytheris-aether-sdk` | Optional `MemoryManager` facade and `otel` telemetry preset |
| `crates/aether-vault` | `nytheris-aether-vault` | Obsidian vault compiler, team HTTP and Git gateway, KV warmer, and `aether-mcp` |

Aether is model-free. It does not call LLMs, split documents into chunks, generate embeddings, or extract entities. Your application computes vectors and extracts entities in its own pipeline, then passes plain Rust structs to Aether.

## Step 1: Add the crate dependencies

Create a new binary crate, or add these dependencies to an existing `Cargo.toml`:

```toml
[dependencies]
nytheris-aether-core = "0.1.0"
nytheris-aether-adapters = "0.1.0"
serde_json = "1.0"
tokio = { version = "1.40", features = ["macros", "rt-multi-thread"] }
```

If you build from a local checkout of `aether-sdk`, point `path` at `crates/aether-core` and `crates/aether-adapters` instead of registry versions.

## Step 2: Run the workspace Level 1 example

The workspace ships a runnable vector example in [`level_1_connector.rs`](https://github.com/Nytheris-AI/aether-sdk/blob/main/crates/aether-sdk/examples/level_1_connector.rs). Run it from the repository root:

```bash
cargo run -p nytheris-aether-sdk --example level_1_connector
```

You see one line showing the matched vector count and generated `MemoryId`:

```text
level 1 hits: 1 (top id=<uuid>)
```

The example initializes a 3-dimensional cosine namespace on `InMemoryBackend`, inserts one `VectorRecord`, and calls `VectorStore::search`.

## Step 3: Store both a record and a vector

A vector hit only gives you an ID, a similarity score, and filterable metadata. In Aether, the full text lives in `RecordStore`, while `VectorStore` holds the derived index entry.

Replace `src/main.rs` with the following code to store a `MemoryItem` in `RecordStore`, index its vector in `VectorStore`, and resolve the matched record back:

```rust
use nytheris_aether_adapters::InMemoryBackend;
use nytheris_aether_core::{
    DistanceMetric, MemoryInput, MemoryItem, ProjectId, RecordStore, VectorRecord, VectorStore,
};
use serde_json::json;

#[tokio::main]
async fn main() -> nytheris_aether_core::Result<()> {
    let backend = InMemoryBackend::new();
    let ns = ProjectId::new("level1-demo")?;

    backend.initialize(&ns, 3, DistanceMetric::Cosine).await?;

    let mut input = MemoryInput::new(ns.clone(), "Service auth uses Ed25519 keys.");
    input.embedding = Some(vec![1.0, 0.0, 0.0]);
    input.metadata = json!({"topic": "connectors"});
    input.validate()?;

    let item = MemoryItem::from_input(input, 1);
    backend.create(&ns, &item, None).await?;

    backend
        .upsert_batch(
            &ns,
            &[VectorRecord {
                id: item.id,
                vector: item.embedding.clone().unwrap_or_default(),
                metadata: item.metadata.clone(),
            }],
        )
        .await?;

    let hits = backend.search(&ns, &[1.0, 0.0, 0.0], 5, None).await?;
    let ids: Vec<_> = hits.iter().map(|hit| hit.id).collect();
    let records = backend.get_many(&ns, &ids).await?;

    println!("hits: {}, top content: {}", hits.len(), records[0].content);
    Ok(())
}
```

Run your binary with `cargo run`. You see the resolved memory content:

```text
hits: 1, top content: Service auth uses Ed25519 keys.
```

## What just happened

1. `ProjectId::new("level1-demo")?` validates a non-empty tenant namespace at the boundary.
2. `backend.initialize(&ns, 3, DistanceMetric::Cosine).await?` fixes the expected vector width (`3`) and similarity metric (`Cosine`) for that namespace.
3. `input.validate()?` rejects empty content, non-finite vector numbers, or caller metadata keys starting with `_aether_`.
4. `backend.create` stores the canonical `MemoryItem` at version `1`, while `backend.upsert_batch` indexes its vector for similarity search.
5. `backend.get_many` fetches live records by ID and drops any record that was deleted.

Writing to two stores by hand leaves room for drift if one call fails. In Part 2, you add retry middleware, compose hybrid graph recall, and replace manual dual writes with `Indexer` and `MemoryManager`.

**Next in series.** Continue to [Part 2: Mastering the Five Composable Levels](./02-five-composable-levels.md).
