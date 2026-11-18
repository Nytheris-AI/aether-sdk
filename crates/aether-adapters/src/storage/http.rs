//! Shared HTTP status and transport error mapping for REST-based connectors.

use nytheris_aether_core::{AetherError, Result};
use reqwest::{Response, StatusCode};

/// Maps an HTTP transport error into a retryable `AetherError::Transient`.
pub(crate) fn map_transport_err(connector: &'static str, err: reqwest::Error) -> AetherError {
    AetherError::Transient {
        connector,
        message: err.to_string(),
    }
}

/// Maps HTTP status codes into `AetherError::Transient` (408/429/5xx) or `Permanent` (4xx).
pub(crate) async fn ensure_success(connector: &'static str, resp: Response) -> Result<Response> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    let body = resp.text().await.unwrap_or_default();
    Err(map_status_err(connector, status, body))
}

/// Converts a non-success HTTP status code and response body into `AetherError`.
pub(crate) fn map_status_err(
    connector: &'static str,
    status: StatusCode,
    body: String,
) -> AetherError {
    let msg = if body.is_empty() {
        format!("HTTP {}", status.as_u16())
    } else {
        format!("HTTP {}: {}", status.as_u16(), body)
    };
    if status == StatusCode::REQUEST_TIMEOUT
        || status == StatusCode::TOO_MANY_REQUESTS
        || status.is_server_error()
    {
        AetherError::Transient {
            connector,
            message: msg,
        }
    } else {
        AetherError::Permanent {
            connector,
            message: msg,
        }
    }
}

/// Normalizes a base URL by trimming trailing slashes.
pub(crate) fn normalize_base_url(raw: &str) -> Result<String> {
    let trimmed = raw.trim().trim_end_matches('/').to_string();
    if trimmed.is_empty() {
        return Err(AetherError::InvalidInput(
            "base_url must not be empty".into(),
        ));
    }
    Ok(trimmed)
}
