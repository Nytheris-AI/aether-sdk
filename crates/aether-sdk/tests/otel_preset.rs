#![cfg(feature = "otel")]

use aether_sdk::telemetry::{init_otel, LogFormat, OtelConfig};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn init_otel_preset_is_one_liner_and_idempotent() {
    let cfg = OtelConfig::from_env("aether-test-service")
        .with_log_format(LogFormat::Json)
        .with_otlp_logs(true);
    assert_eq!(cfg.service_name, "aether-test-service");

    let mut guard1 = init_otel(cfg.clone()).expect("first init_otel should succeed");
    // Second call must not panic if a global subscriber is already installed.
    let mut guard2 = init_otel(cfg).expect("second init_otel should be idempotent");

    guard2.shutdown();
    guard1.shutdown();
}

#[test]
fn otel_config_debug_redacts_endpoint_credentials() {
    let cfg = OtelConfig::from_env("aether-test-service")
        .with_endpoint("https://collector:s3cr3tpass@otel.example.com:4317");
    let dbg = format!("{cfg:?}");
    assert!(!dbg.contains("s3cr3tpass"));
    assert!(dbg.contains("[REDACTED]@otel.example.com:4317"));
}
