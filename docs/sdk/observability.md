# Observability & OpenTelemetry Traces

Aether SDK emits structured **`tracing`** spans across `muninn-aether-sdk`, `muninn-aether-cortex`, and `muninn-aether-adapters` out of the box so distributed traces connect end-to-end with zero per-call boilerplate.

---

## 1. Zero-Boilerplate Trace Hierarchy

`MemoryManager::builder(record_store)` wraps configured stores in `Traced<T>` by default (`with_connector_tracing(true)`). Every operation produces a connected parent-child span tree:

```text
aether.recall (MemoryManager)                    [project_id, mode, consistency, top_k, otel.status_code]
├── aether.cortex.vector_search                  [project_id, top_k]
├── aether.cortex.graph_search                   [project_id, seed_count, hops]
├── aether.cortex.ppr                            (PersonalizedPageRank)
└── connector_op (Traced<T>)                     [otel.kind="client", db.system, connector, project_id, op, otel.status_code]
```

And for writes and background sync (`remember`, `update`, `forget`, `flush`):

```text
aether.remember (MemoryManager)                  [project_id, scope, durability, otel.status_code]
├── connector_op (Traced<RecordStore>)           [op="record_create", otel.kind="client", db.system, otel.status_code]
└── aether.cortex.index_record (Indexer)         [project_id, memory_id, tombstone]
    └── connector_op (Traced<Vector/Graph/Rec>)  [op="upsert_batch" | "upsert_relations" | "record_mark_indexed"]

aether.flush (MemoryManager)                     [otel.status_code]
└── aether.cortex.index_pass (Indexer)           [indexed, failed]
    └── aether.cortex.index_record               [project_id, memory_id, tombstone]
```

### Span & Event Details

- **Root spans (`muninn-aether-sdk`)**: `aether.remember`, `aether.recall`, `aether.update`, `aether.forget`, `aether.flush`, `aether.shutdown` record `otel.status_code` (`"OK"` or `"ERROR"`) and emit `tracing::error!` on failure.
- **Orchestration spans (`muninn-aether-cortex`)**: `aether.cortex.index_pass`, `aether.cortex.index_record`, `aether.cortex.vector_search`, `aether.cortex.graph_search`, `aether.cortex.ppr`.
- **Connector client spans (`muninn-aether-adapters::Traced`)**: `connector_op` with `otel.kind = "client"`, `db.system`, `connector`, `project_id`, `op` (`record_create`, `record_get`, `record_get_many`, `record_cas_update`, `record_tombstone`, `record_pending`, `record_pending_for`, `record_session_items`, `record_mark_indexed`, `initialize`, `describe`, `upsert_batch`, `delete`, `search`, `upsert_entities`, `upsert_relations`, `retract_memory_facts`, `neighborhood`), and `otel.status_code`.
- **Resilience events (`muninn-aether-adapters`)**: `Resilient<T>` emits `tracing::warn!` on transient retry attempts; `Timeout<T>` emits `tracing::warn!` on timeout.

### Opting Out of Auto-Wrapping

```rust
let manager = MemoryManager::builder(record_store)
    .vector(vector_store)
    .graph(graph_store)
    .with_connector_tracing(false)
    .build();
```

---

## 2. One-Line OpenTelemetry Setup (`feature = "otel"`)

Enable the `otel` feature on `muninn-aether-sdk` to use the built-in OTLP gRPC preset:

```toml
[dependencies]
muninn-aether-sdk = { version = "0.1.0", features = ["otel"] }
```

In your application entrypoint:

```rust
use muninn_aether_sdk::telemetry::{init_otel, LogFormat, OtelConfig};
use muninn_aether_sdk::MemoryManager;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Reads OTEL_EXPORTER_OTLP_ENDPOINT and RUST_LOG from the environment.
    // Dropping `_telemetry` (or calling `_telemetry.shutdown()`) flushes pending spans & logs.
    let _telemetry = init_otel(
        OtelConfig::from_env("my-agent-service")
            .with_log_format(LogFormat::Pretty),
    )?;

    let manager = MemoryManager::builder(record_store)
        .vector(vector_store)
        .graph(graph_store)
        .build();
    Ok(())
}
```

### Where Logs & Traces Go

- **OTLP Traces**: Exported via gRPC to `OTEL_EXPORTER_OTLP_ENDPOINT` (default `http://localhost:4317`).
- **OTLP Logs**: Exported via gRPC (`opentelemetry-appender-tracing`) to the same OTLP endpoint when `otlp_logs = true` (the default), automatically correlated with `trace_id` and `span_id`. Toggle with `.with_otlp_logs(false)`.
- **Console Logs**: Printed to `stdout` via `tracing_subscriber::fmt` (`LogFormat::Pretty`, `LogFormat::Json`, or `LogFormat::None`), filtered by `RUST_LOG` (default `info`).

---

## 3. Testing OpenTelemetry Locally

Spin up a local all-in-one **Grafana LGTM** container (OTel Collector + Tempo + Loki + Grafana UI):

```bash
docker run --rm -p 3000:3000 -p 4317:4317 -p 4318:4318 \
  grafana/otel-lgtm:latest
```

Then run your app or the included demo (`crates/aether-sdk/examples/otel_demo.rs`) with `OTEL_EXPORTER_OTLP_ENDPOINT`:

```bash
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4317 \
RUST_LOG=info \
cargo run -p muninn-aether-sdk --example otel_demo --features otel
```

### Bringing Your Own Subscriber

`init_otel` uses `tracing_subscriber::registry().try_init()`. If your host application already installed a global subscriber / OpenTelemetry pipeline, `tracing` spans flow into it automatically and calling `init_otel` remains idempotent without panicking.
