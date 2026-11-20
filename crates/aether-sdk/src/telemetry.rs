//! Opt-in OpenTelemetry OTLP trace & log initialization preset (enabled via `feature = "otel"`).

use std::fmt as std_fmt;

use muninn_aether_core::{redact_url_userinfo, AetherError, Result};
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_otlp::{LogExporter, SpanExporter, WithExportConfig};
use opentelemetry_sdk::logs::SdkLoggerProvider;
use opentelemetry_sdk::trace::SdkTracerProvider;
use opentelemetry_sdk::Resource;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{fmt, EnvFilter};

/// Console log output format for `tracing_subscriber::fmt`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LogFormat {
    #[default]
    Pretty,
    Json,
    None,
}

/// Configuration for the one-line OpenTelemetry OTLP trace & log preset.
#[derive(Clone)]
pub struct OtelConfig {
    pub service_name: String,
    pub otlp_endpoint: Option<String>,
    pub env_filter: Option<String>,
    pub log_format: LogFormat,
    pub otlp_logs: bool,
}

impl std_fmt::Debug for OtelConfig {
    fn fmt(&self, f: &mut std_fmt::Formatter<'_>) -> std_fmt::Result {
        let redacted_endpoint = self.otlp_endpoint.as_deref().map(redact_url_userinfo);
        f.debug_struct("OtelConfig")
            .field("service_name", &self.service_name)
            .field("otlp_endpoint", &redacted_endpoint)
            .field("env_filter", &self.env_filter)
            .field("log_format", &self.log_format)
            .field("otlp_logs", &self.otlp_logs)
            .finish()
    }
}

impl OtelConfig {
    /// Creates a configuration from standard environment variables
    /// (`OTEL_EXPORTER_OTLP_ENDPOINT`, `RUST_LOG`), falling back to `service_name`.
    pub fn from_env(service_name: impl Into<String>) -> Self {
        Self {
            service_name: service_name.into(),
            otlp_endpoint: std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").ok(),
            env_filter: std::env::var("RUST_LOG").ok(),
            log_format: LogFormat::Pretty,
            otlp_logs: true,
        }
    }

    pub fn with_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.otlp_endpoint = Some(endpoint.into());
        self
    }

    pub fn with_env_filter(mut self, filter: impl Into<String>) -> Self {
        self.env_filter = Some(filter.into());
        self
    }

    pub fn with_log_format(mut self, format: LogFormat) -> Self {
        self.log_format = format;
        self
    }

    pub fn with_otlp_logs(mut self, enabled: bool) -> Self {
        self.otlp_logs = enabled;
        self
    }
}

/// Guard that flushes and shuts down the OpenTelemetry `SdkTracerProvider` and `SdkLoggerProvider` on drop.
pub struct TelemetryGuard {
    tracer_provider: Option<SdkTracerProvider>,
    logger_provider: Option<SdkLoggerProvider>,
}

impl TelemetryGuard {
    /// Explicitly flushes pending spans/logs and shuts down the OpenTelemetry providers.
    pub fn shutdown(&mut self) {
        if let Some(provider) = self.tracer_provider.take() {
            let _ = provider.shutdown();
        }
        if let Some(provider) = self.logger_provider.take() {
            let _ = provider.shutdown();
        }
    }
}

impl Drop for TelemetryGuard {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Initializes a global `tracing` subscriber wired to OpenTelemetry OTLP traces,
/// optional OTLP logs, and optional stdout `fmt` logs.
///
/// Idempotent: if a global `tracing` subscriber is already installed by the host application,
/// this returns `Ok(TelemetryGuard)` without panicking so SDK spans still flow into the host's pipeline.
pub fn init_otel(config: OtelConfig) -> Result<TelemetryGuard> {
    let resource = Resource::builder()
        .with_service_name(config.service_name)
        .build();

    let mut span_exporter = SpanExporter::builder().with_tonic();
    if let Some(endpoint) = &config.otlp_endpoint {
        span_exporter = span_exporter.with_endpoint(endpoint.clone());
    }
    let span_exporter = span_exporter.build().map_err(|e| AetherError::Permanent {
        connector: "otel",
        message: format!("failed to build OTLP span exporter: {e}"),
    })?;

    let tracer_provider = SdkTracerProvider::builder()
        .with_resource(resource.clone())
        .with_batch_exporter(span_exporter)
        .build();
    let tracer = tracer_provider.tracer("aether-sdk");
    let otel_trace_layer = tracing_opentelemetry::layer().with_tracer(tracer);

    let logger_provider = if config.otlp_logs {
        let mut log_exporter = LogExporter::builder().with_tonic();
        if let Some(endpoint) = &config.otlp_endpoint {
            log_exporter = log_exporter.with_endpoint(endpoint.clone());
        }
        let log_exporter = log_exporter.build().map_err(|e| AetherError::Permanent {
            connector: "otel",
            message: format!("failed to build OTLP log exporter: {e}"),
        })?;
        Some(
            SdkLoggerProvider::builder()
                .with_resource(resource)
                .with_batch_exporter(log_exporter)
                .build(),
        )
    } else {
        None
    };

    let otel_log_layer = logger_provider
        .as_ref()
        .map(OpenTelemetryTracingBridge::new);

    let pretty_layer = (config.log_format == LogFormat::Pretty).then(fmt::layer);
    let json_layer = (config.log_format == LogFormat::Json).then(|| fmt::layer().json());

    let filter = config
        .env_filter
        .as_deref()
        .map(EnvFilter::new)
        .unwrap_or_else(|| {
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"))
        });

    let installed = tracing_subscriber::registry()
        .with(filter)
        .with(otel_trace_layer)
        .with(otel_log_layer)
        .with(pretty_layer)
        .with(json_layer)
        .try_init()
        .is_ok();

    if installed {
        Ok(TelemetryGuard {
            tracer_provider: Some(tracer_provider),
            logger_provider,
        })
    } else {
        let _ = tracer_provider.shutdown();
        if let Some(lp) = logger_provider {
            let _ = lp.shutdown();
        }
        Ok(TelemetryGuard {
            tracer_provider: None,
            logger_provider: None,
        })
    }
}
