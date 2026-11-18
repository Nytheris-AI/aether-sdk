use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use nytheris_aether_core::{AetherError, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::engine::VaultEngine;
use crate::inbox::{APPROVED_MIRROR_REL_PATH, INBOX_REL_PATH};
use crate::types::CandidateId;

const MAX_REQUEST_BYTES: usize = 1024 * 1024;

/// Loopback-only HTTP/1.1 JSON control plane for Obsidian companion status,
/// 1-click Inbox decisions, and Dream pass execution.
pub struct ControlPlaneServer {
    local_addr: SocketAddr,
    shutdown_tx: watch::Sender<bool>,
    handle: Mutex<Option<JoinHandle<()>>>,
}

#[derive(Debug, Deserialize)]
struct DecideRequest {
    candidate_id: String,
    decision: String,
}

impl ControlPlaneServer {
    /// Binds an HTTP control-plane server strictly on a loopback address (`127.0.0.1` or `::1`).
    pub async fn bind(addr: SocketAddr, engine: Arc<VaultEngine>) -> Result<Self> {
        if !addr.ip().is_loopback() {
            return Err(AetherError::InvalidInput(format!(
                "control plane address `{addr}` must bind strictly to a loopback IP"
            )));
        }

        let listener = TcpListener::bind(addr)
            .await
            .map_err(|err| AetherError::Permanent {
                connector: "http-control-plane",
                message: format!("failed to bind `{addr}`: {err}"),
            })?;
        let local_addr = listener
            .local_addr()
            .map_err(|err| AetherError::Permanent {
                connector: "http-control-plane",
                message: format!("failed to read local_addr: {err}"),
            })?;
        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);

        let handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = shutdown_rx.changed() => break,
                    accepted = listener.accept() => {
                        let Ok((stream, _)) = accepted else {
                            break;
                        };
                        let eng = Arc::clone(&engine);
                        tokio::spawn(async move {
                            let _ = handle_connection(stream, eng).await;
                        });
                    }
                }
            }
        });

        Ok(Self {
            local_addr,
            shutdown_tx,
            handle: Mutex::new(Some(handle)),
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub async fn shutdown(&self) -> Result<()> {
        let _ = self.shutdown_tx.send(true);
        let handle = self
            .handle
            .lock()
            .map_err(|_| AetherError::Permanent {
                connector: "http-control-plane",
                message: "http server mutex poisoned".into(),
            })?
            .take();
        if let Some(h) = handle {
            let _ = h.await;
        }
        Ok(())
    }
}

async fn handle_connection(mut stream: TcpStream, engine: Arc<VaultEngine>) -> std::io::Result<()> {
    let Some((method, path, body)) = read_http_request(&mut stream).await? else {
        return write_json_response(
            &mut stream,
            400,
            "Bad Request",
            &json!({ "error": "malformed http request" }),
        )
        .await;
    };

    if method == "OPTIONS" {
        return write_no_content(&mut stream).await;
    }

    // Green implementation of all HTTP control plane routes.
    let (status_code, reason, json_body) = match (method.as_str(), path.as_str()) {
        ("GET", "/v1/status") => match engine.status().await {
            Ok(rep) => ok_payload(rep),
            Err(err) => err_payload(err),
        },
        ("GET", "/v1/inbox") => match engine.pending_inbox().await {
            Ok(pending) => (
                200,
                "OK",
                json!({
                    "inbox_path": INBOX_REL_PATH,
                    "approved_mirror_path": APPROVED_MIRROR_REL_PATH,
                    "pending": pending,
                }),
            ),
            Err(err) => err_payload(err),
        },
        ("POST", "/v1/inbox/decide") => handle_decide(&engine, &body).await,
        ("POST", "/v1/inbox/reconcile") => match engine.reconcile_inbox().await {
            Ok(rep) => ok_payload(rep),
            Err(err) => err_payload(err),
        },
        ("POST", "/v1/dream") => match engine.run_dream_pass().await {
            Ok(rep) => ok_payload(rep),
            Err(err) => err_payload(err),
        },
        ("POST", "/v1/sync") => match engine.sync_vault().await {
            Ok(rep) => ok_payload(rep),
            Err(err) => err_payload(err),
        },
        _ => (
            404,
            "Not Found",
            json!({ "error": format!("unknown route {method} {path}") }),
        ),
    };

    write_json_response(&mut stream, status_code, reason, &json_body).await
}

async fn handle_decide(engine: &VaultEngine, body: &[u8]) -> (u16, &'static str, Value) {
    let parsed: DecideRequest = match serde_json::from_slice(body) {
        Ok(req) => req,
        Err(err) => {
            return (
                400,
                "Bad Request",
                json!({ "error": format!("invalid json body: {err}") }),
            );
        }
    };
    let cid = match CandidateId::parse(&parsed.candidate_id) {
        Ok(id) => id,
        Err(err) => return err_payload(err),
    };
    let approve = match parsed.decision.trim().to_ascii_lowercase().as_str() {
        "approve" | "approved" => true,
        "reject" | "rejected" => false,
        other => {
            return (
                400,
                "Bad Request",
                json!({ "error": format!("decision must be 'approve' or 'reject' (got `{other}`)") }),
            );
        }
    };

    match engine.decide_inbox_candidate(&cid, approve).await {
        Ok(reconcile) => (
            200,
            "OK",
            json!({
                "candidate_id": cid.as_str(),
                "decision": if approve { "approve" } else { "reject" },
                "reconcile": reconcile,
            }),
        ),
        Err(err) => err_payload(err),
    }
}

fn ok_payload<T: serde::Serialize>(val: T) -> (u16, &'static str, Value) {
    match serde_json::to_value(val) {
        Ok(v) => (200, "OK", v),
        Err(err) => (
            500,
            "Internal Server Error",
            json!({ "error": format!("serialization failure: {err}") }),
        ),
    }
}

fn err_payload(err: AetherError) -> (u16, &'static str, Value) {
    let (code, reason) = match &err {
        AetherError::InvalidInput(_) => (400, "Bad Request"),
        AetherError::NotFound(_) => (404, "Not Found"),
        _ => (500, "Internal Server Error"),
    };
    (code, reason, json!({ "error": err.to_string() }))
}

async fn read_http_request(
    stream: &mut TcpStream,
) -> std::io::Result<Option<(String, String, Vec<u8>)>> {
    let mut buf = Vec::with_capacity(1024);
    let mut tmp = [0u8; 2048];
    let header_end = loop {
        if let Some(pos) = find_double_crlf(&buf) {
            break pos;
        }
        if buf.len() >= MAX_REQUEST_BYTES {
            return Ok(None);
        }
        let n = stream.read(&mut tmp).await?;
        if n == 0 {
            return Ok(None);
        }
        buf.extend_from_slice(&tmp[..n]);
    };

    let header_bytes = &buf[..header_end];
    let header_str = std::str::from_utf8(header_bytes).ok();
    let Some(header_str) = header_str else {
        return Ok(None);
    };

    let mut lines = header_str.split("\r\n");
    let Some(request_line) = lines.next() else {
        return Ok(None);
    };
    let mut parts = request_line.split_whitespace();
    let (Some(raw_method), Some(raw_target)) = (parts.next(), parts.next()) else {
        return Ok(None);
    };
    let method = raw_method.to_ascii_uppercase();
    let path = raw_target
        .split('?')
        .next()
        .unwrap_or(raw_target)
        .to_string();

    let mut content_length: usize = 0;
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            if k.trim().eq_ignore_ascii_case("content-length") {
                content_length = v.trim().parse().unwrap_or(0);
            }
        }
    }
    if content_length > MAX_REQUEST_BYTES {
        return Ok(None);
    }

    let body_start = header_end + 4;
    while buf.len() < body_start + content_length {
        let n = stream.read(&mut tmp).await?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
    }
    let body_end = (body_start + content_length).min(buf.len());
    let body = buf[body_start..body_end].to_vec();
    Ok(Some((method, path, body)))
}

fn find_double_crlf(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

async fn write_no_content(stream: &mut TcpStream) -> std::io::Result<()> {
    let resp = "HTTP/1.1 204 No Content\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
    stream.write_all(resp.as_bytes()).await?;
    stream.flush().await
}

async fn write_json_response(
    stream: &mut TcpStream,
    code: u16,
    reason: &str,
    payload: &Value,
) -> std::io::Result<()> {
    let body = serde_json::to_string(payload).unwrap_or_else(|_| "{}".to_string());
    let header = format!(
        "HTTP/1.1 {code} {reason}\r\nContent-Type: application/json\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes()).await?;
    stream.write_all(body.as_bytes()).await?;
    stream.flush().await
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use uuid::Uuid;

    use super::*;
    use crate::types::VaultConfig;

    struct TempVault {
        root: PathBuf,
    }

    impl TempVault {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("aether_http_test_{}", Uuid::new_v4()));
            fs::create_dir_all(&root).unwrap();
            Self { root }
        }

        fn write(&self, rel: &str, content: &str) {
            let full = self.root.join(rel);
            if let Some(parent) = full.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(full, content).unwrap();
        }
    }

    impl Drop for TempVault {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    async fn http_call(
        addr: SocketAddr,
        method: &str,
        path: &str,
        body: Option<&str>,
    ) -> (u16, Value) {
        let mut stream = TcpStream::connect(addr).await.unwrap();
        let body_str = body.unwrap_or("");
        let req = format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body_str}",
            body_str.len()
        );
        stream.write_all(req.as_bytes()).await.unwrap();
        stream.flush().await.unwrap();
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).await.unwrap();
        let text = String::from_utf8(raw).unwrap();
        let (head, json_body) = text.split_once("\r\n\r\n").unwrap();
        let status_line = head.lines().next().unwrap();
        let code: u16 = status_line
            .split_whitespace()
            .nth(1)
            .unwrap()
            .parse()
            .unwrap();
        let parsed: Value = serde_json::from_str(json_body).unwrap();
        (code, parsed)
    }

    #[tokio::test]
    async fn control_plane_rejects_non_loopback_bind_addresses() {
        let vault = TempVault::new();
        vault.write("Note.md", "# Note\nSample.\n");
        let engine = Arc::new(
            VaultEngine::open(VaultConfig::in_memory(&vault.root))
                .await
                .unwrap(),
        );

        assert!(matches!(
            ControlPlaneServer::bind("0.0.0.0:0".parse().unwrap(), Arc::clone(&engine)).await,
            Err(AetherError::InvalidInput(_))
        ));
        assert!(matches!(
            ControlPlaneServer::bind("192.168.1.50:8080".parse().unwrap(), Arc::clone(&engine))
                .await,
            Err(AetherError::InvalidInput(_))
        ));

        engine.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn control_plane_serves_status_dream_and_one_click_inbox_approval() {
        let vault = TempVault::new();
        vault.write(
            "Arch/Vector.md",
            "---\naether_tier: warm\naether_scope:\n  - engineering\n---\n# Engine\nUse Qdrant for local vector search.\n",
        );
        vault.write(
            "Daily/2026-05-10.md",
            "---\naether_scope: engineering\n---\n# Daily\n## Decisions\n- Decision: Switched local vector search from Qdrant to LanceDB in [[Arch/Vector]].\n",
        );

        let engine = Arc::new(
            VaultEngine::open(VaultConfig::in_memory(&vault.root))
                .await
                .unwrap(),
        );
        let server = ControlPlaneServer::bind("127.0.0.1:0".parse().unwrap(), Arc::clone(&engine))
            .await
            .unwrap();
        let addr = server.local_addr();

        let (code, status) = http_call(addr, "GET", "/v1/status", None).await;
        assert_eq!(code, 200);
        assert_eq!(status["status"], "ok");
        assert_eq!(status["pending_inbox_count"], 0);

        let (code, dream) = http_call(addr, "POST", "/v1/dream", Some("{}")).await;
        assert_eq!(code, 200);
        assert_eq!(dream["candidates_staged"], 1);
        let cand_id = dream["staged_candidate_ids"][0]
            .as_str()
            .unwrap()
            .to_string();

        let (code, inbox) = http_call(addr, "GET", "/v1/inbox", None).await;
        assert_eq!(code, 200);
        assert_eq!(inbox["pending"].as_array().unwrap().len(), 1);

        let decide_body = json!({ "candidate_id": cand_id, "decision": "approve" }).to_string();
        let (code, decided) = http_call(addr, "POST", "/v1/inbox/decide", Some(&decide_body)).await;
        assert_eq!(code, 200);
        assert_eq!(decided["reconcile"]["compiled"], 1);
        assert!(vault.root.join("Aether/Compiled/Approved.md").exists());

        server.shutdown().await.unwrap();
        engine.shutdown().await.unwrap();
    }
}
