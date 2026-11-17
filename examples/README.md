# Aether SDK Examples

Runnable Rust examples live in `crates/aether-sdk/examples/`.

## OpenTelemetry Demo

Run the end-to-end memory lifecycle with OpenTelemetry spans and logs enabled:

```bash
cargo run -p aether-sdk --example otel_demo --features otel
```

See [crates/aether-sdk/examples/otel_demo.rs](../crates/aether-sdk/examples/otel_demo.rs) for the source.
