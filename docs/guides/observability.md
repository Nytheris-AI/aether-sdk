# Observability & OpenTelemetry Traces

Aether SDK emits structured **`tracing`** spans across every layer (`MemoryManager` → `aether-cortex` → `aether-adapters`) out of the box—so distributed traces connect end-to-end with **zero per-call or per-store boilerplate**.

---

## 1. Zero-Boilerplate Trace Hierarchy

When you construct `MemoryManager::new(kv, vec, graph)`, all three stores are **automatically wrapped** in `Traced<T>` by default. Every operation produces a connected parent-child span tree:

```text
aether.recall (MemoryManager)                  [project_id, mode, consistency, top_k, otel.status_code]
└── aether.cortex.query_plan (QueryPlanner)    [project_id, mode, consistency, top_k]
    ├── aether.cortex.ppr (PersonalizedPageRank)
    └── connector_op (Traced<T>)               [otel.kind="client", db.system, connector, project_id, op, otel.status_code]
```

And for writes (`remember`, `update`):

```text
aether.remember (MemoryManager)                [project_id, scope, durability, otel.status_code]
├── connector_op (Traced<KvStore>)             [op="kv_put", otel.kind="client", db.system, otel.status_code]
└── aether.cortex.enqueue (IngestionBuffer)    [project_id, memory_id, durability]
    └── aether.cortex.flush (IngestionBuffer)  [item_count]
        └── connector_op (Traced<Vector/Graph>)
```

### Span & Event Details
- **Root spans (`aether-sdk`)**: `aether.remember`, `aether.recall`, `aether.update`, `aether.forget`, `aether.flush`, `aether.shutdown` — record `otel.status_code` (`"OK"` or `"ERROR"`) and emit `tracing::error!` on failure.
- **Orchestration spans (`aether-cortex`)**: `aether.cortex.enqueue`, `aether.cortex.flush` (`item_count`), `aether.cortex.query_plan`, `aether.cortex.ppr`.
- **Connector client spans (`aether-adapters::Traced`)**: `connector_op` with `otel.kind = "client"`, `db.system`, `connector`, `project_id`, `op`, and `otel.status_code` (`"OK"` / `"ERROR"`).
- **Resilience events (`aether-adapters`)**: `Resilient<T>` emits `tracing::warn!` on transient retry attempts; `Timeout<T>` emits `tracing::warn!` on timeout.

### Opting Out of Auto-Wrapping
If you already wrap your connectors manually (or want to disable connector spans), use `MemoryManager::builder`:

```rust
let manager = MemoryManager::builder(kv, vec, graph)
    .with_connector_tracing(false)
    .build();
```

---

## 2. One-Line OpenTelemetry Setup (`feature = "otel"`)

Enable the `otel` feature on `aether-sdk` to use the built-in OTLP gRPC preset:

```toml
[dependencies]
aether-sdk = { version = "0.1.0", features = ["otel"] }
```

In your application entrypoint:

```rust
use aether_sdk::telemetry::{init_otel, LogFormat, OtelConfig};
use aether_sdk::MemoryManager;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Reads OTEL_EXPORTER_OTLP_ENDPOINT and RUST_LOG from the environment.
    // Sends OTLP gRPC traces + OTLP gRPC logs + stdout logs (Pretty by default, or Json/None).
    // Dropping `_telemetry` (or calling `_telemetry.shutdown()`) flushes pending spans & logs.
    let _telemetry = init_otel(
        OtelConfig::from_env("my-agent-service")
            .with_log_format(LogFormat::Pretty),
    )?;

    // Full end-to-end traces and correlated logs are emitted automatically:
    let brain = MemoryManager::new(kv, vec, graph);
    Ok(())
}
```

### Where Logs & Traces Go
- **OTLP Traces**: Exported via gRPC to `OTEL_EXPORTER_OTLP_ENDPOINT` (default `http://localhost:4317`).
- **OTLP Logs**: Exported via gRPC (`opentelemetry-appender-tracing`) to the same OTLP endpoint when `otlp_logs = true` (the default), automatically correlated with `trace_id` and `span_id`. Toggle with `.with_otlp_logs(false)`.
- **Console Logs**: Printed to `stdout` via `tracing_subscriber::fmt` (`LogFormat::Pretty`, `LogFormat::Json`, or `LogFormat::None`), filtered by `RUST_LOG` (default `info`).

---

## 3. Testing OpenTelemetry Locally

Spin up a local all-in-one **Grafana LGTM** container (includes OTel Collector + Tempo for traces + Loki for logs + Grafana UI):

```bash
docker run --rm -p 3000:3000 -p 4317:4317 -p 4318:4318 \
  grafana/otel-lgtm:latest
```

*(Or, if you only need traces, `docker run --rm -p 16686:16686 -p 4317:4317 jaegertracing/jaeger:latest`.)*

Then run your app with `OTEL_EXPORTER_OTLP_ENDPOINT` and `RUST_LOG`:

```bash
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4317 \
RUST_LOG=info \
cargo run --features otel
```

Open **http://localhost:3000** (Grafana → Explore → **Tempo** for traces, **Loki** for `trace_id`-correlated logs) or **http://localhost:16686** (Jaeger UI).

---

### Bringing Your Own Subscriber
`init_otel` uses `tracing_subscriber::registry().try_init()`. If your host application already installed its own `tracing` subscriber / OpenTelemetry pipeline:
- You do **not** need to enable the `otel` feature—Aether's `tracing` spans flow into your existing subscriber automatically.
- Even if `init_otel` is called when a global subscriber is already set, it is **idempotent** and returns `Ok` without panicking.
