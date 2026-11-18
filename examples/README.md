# Aether SDK Examples

Runnable Rust examples live in [`crates/aether-sdk/examples/`](../crates/aether-sdk/examples/).

## Progressive SDK Levels

Run any level directly against the zero-infra `InMemoryBackend`:

```bash
cargo run -p aether-sdk --example level_1_connector
cargo run -p aether-sdk --example level_2_middleware
cargo run -p aether-sdk --example level_3_hybrid_recall
cargo run -p aether-sdk --example level_4_sync_engine
cargo run -p aether-sdk --example level_5_facade
```

See the [Usage Guide](../docs/guides/usage.md) for a walkthrough of each level.

## OpenTelemetry Demo

Run the end-to-end memory lifecycle with OpenTelemetry spans and logs enabled:

```bash
cargo run -p aether-sdk --example otel_demo --features otel
```

See [`otel_demo.rs`](../crates/aether-sdk/examples/otel_demo.rs) and the [Observability Guide](../docs/guides/observability.md).
