use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use muninn_aether_core::{AetherError, Result};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::auth::{
    AgentId, AgentRole, ApiKeySecret, AuthError, IssueApiKeyRequest, KeyId, ScopedApiKey,
};
use crate::engine::VaultEngine;
use crate::git::GitSyncOptions;
use crate::inbox::{APPROVED_MIRROR_REL_PATH, INBOX_REL_PATH};
use crate::kv::{KvStorageTier, ModelId, WarmKvRequest};
use crate::mcp::McpServer;
use crate::types::{
    AmuId, CandidateId, RecallRequest, ScopeTag, StageMemoryRequest, TokenBudget, VaultPushRequest,
    VaultTier,
};

const MAX_REQUEST_BYTES: usize = 1024 * 1024;

/// HTTP/1.1 JSON control plane and team gateway for Obsidian status, scoped MCP/recall/stage/inspect,
/// Git vault sync, API key management, and GPU KV-cache warming.
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

#[derive(Debug, Deserialize)]
struct RecallHttpRequest {
    query: String,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    max_tokens: Option<usize>,
    #[serde(default = "default_true")]
    include_hot_profile: bool,
    #[serde(default)]
    kv_model: Option<String>,
}

#[derive(Debug, Deserialize)]
struct StageHttpRequest {
    proposed_fact: String,
    #[serde(default)]
    suggested_tier: Option<String>,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    target_note: Option<String>,
    #[serde(default)]
    supersedes_amu_id: Option<String>,
    #[serde(default)]
    proposed_by: Option<String>,
}

#[derive(Debug, Deserialize)]
struct InspectHttpRequest {
    source_pointer: String,
}

#[derive(Debug, Default, Deserialize)]
struct GitSyncHttpRequest {
    #[serde(default)]
    remote: Option<String>,
    #[serde(default)]
    branch: Option<String>,
    #[serde(default)]
    push: bool,
    #[serde(default)]
    commit_message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WarmKvHttpRequest {
    model_id: String,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    amu_ids: Option<Vec<String>>,
    #[serde(default)]
    target_tier: Option<String>,
    #[serde(default = "default_true")]
    include_hot_prefix: bool,
    #[serde(default = "default_true")]
    include_cache_blend: bool,
    #[serde(default)]
    block_tokens: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct IssueKeyHttpRequest {
    agent_id: String,
    role: String,
    scopes: Vec<String>,
    #[serde(default)]
    expires_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RevokeKeyHttpRequest {
    key_id: String,
}

fn default_true() -> bool {
    true
}

struct ParsedHttpRequest {
    method: String,
    path: String,
    query: Option<String>,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

type HttpTuple = (u16, &'static str, Value);

impl ControlPlaneServer {
    /// Binds an unauthenticated HTTP control-plane server strictly on a loopback address (`127.0.0.1` or `::1`).
    pub async fn bind(addr: SocketAddr, engine: Arc<VaultEngine>) -> Result<Self> {
        Self::bind_with_auth(addr, engine, false).await
    }

    /// Binds an HTTP control-plane server with optional mandatory bearer authentication.
    ///
    /// Non-loopback bind addresses are permitted only when `require_auth` is `true`.
    pub async fn bind_with_auth(
        addr: SocketAddr,
        engine: Arc<VaultEngine>,
        require_auth: bool,
    ) -> Result<Self> {
        if !require_auth && !addr.ip().is_loopback() {
            return Err(AetherError::InvalidInput(format!(
                "unauthenticated control plane address `{addr}` must bind strictly to a loopback IP"
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
                            let _ = handle_connection(stream, eng, require_auth).await;
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

async fn handle_connection(
    mut stream: TcpStream,
    engine: Arc<VaultEngine>,
    require_auth: bool,
) -> std::io::Result<()> {
    let Some(req) = read_http_request(&mut stream).await? else {
        return write_json_response(
            &mut stream,
            400,
            "Bad Request",
            &json!({ "error": "malformed http request" }),
        )
        .await;
    };

    if req.method == "OPTIONS" {
        return write_no_content(&mut stream).await;
    }

    let agent = if is_public_route(&req) {
        None
    } else {
        match authenticate_http_request(&engine, &req.headers, require_auth).await {
            Ok(agent) => agent,
            Err(err_resp) => {
                return write_json_response(&mut stream, err_resp.0, err_resp.1, &err_resp.2).await;
            }
        }
    };

    let (status_code, reason, json_body) = dispatch_route(&engine, &req, agent.as_ref()).await;
    write_json_response(&mut stream, status_code, reason, &json_body).await
}

fn is_public_route(req: &ParsedHttpRequest) -> bool {
    req.method == "GET" && req.path == "/v1/openapi.json"
}

async fn authenticate_http_request(
    engine: &VaultEngine,
    headers: &[(String, String)],
    require_auth: bool,
) -> std::result::Result<Option<ScopedApiKey>, HttpTuple> {
    match find_bearer_header(headers) {
        Some(Ok(raw_token)) => {
            let secret = ApiKeySecret::parse(raw_token)
                .map_err(|_| auth_err_payload(AuthError::InvalidToken))?;
            let agent = engine
                .authenticate_api_key(&secret)
                .await
                .map_err(auth_err_payload)?;
            Ok(Some(agent))
        }
        Some(Err(err)) => Err(auth_err_payload(err)),
        None if require_auth => Err(auth_err_payload(AuthError::MissingToken)),
        None => Ok(None),
    }
}

fn find_header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.trim())
}

fn find_bearer_header(
    headers: &[(String, String)],
) -> Option<std::result::Result<&str, AuthError>> {
    let trimmed = find_header(headers, "authorization")?;
    if trimmed.len() >= 7 && trimmed[..7].eq_ignore_ascii_case("bearer ") {
        let token = trimmed[7..].trim();
        if token.is_empty() {
            return Some(Err(AuthError::InvalidToken));
        }
        return Some(Ok(token));
    }
    Some(Err(AuthError::InvalidToken))
}

async fn dispatch_route(
    engine: &Arc<VaultEngine>,
    req: &ParsedHttpRequest,
    agent: Option<&ScopedApiKey>,
) -> HttpTuple {
    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/v1/openapi.json") => handle_openapi(&req.headers),
        ("GET", "/v1/status") => match engine.status().await {
            Ok(rep) => ok_payload(rep),
            Err(err) => err_payload(err),
        },
        ("GET", "/v1/inbox") => handle_inbox(engine, agent).await,
        ("POST", "/v1/inbox/decide") => handle_decide(engine, &req.body, agent).await,
        ("POST", "/v1/inbox/reconcile") => {
            if let Err(resp) = require_role(agent, AgentRole::Admin) {
                return resp;
            }
            match engine.reconcile_inbox().await {
                Ok(rep) => ok_payload(rep),
                Err(err) => err_payload(err),
            }
        }
        ("POST", "/v1/dream") => {
            if let Err(resp) = require_role(agent, AgentRole::Admin) {
                return resp;
            }
            match engine.run_dream_pass().await {
                Ok(rep) => ok_payload(rep),
                Err(err) => err_payload(err),
            }
        }
        ("POST", "/v1/sync") => {
            if let Err(resp) = require_role(agent, AgentRole::Admin) {
                return resp;
            }
            match engine.sync_vault().await {
                Ok(rep) => ok_payload(rep),
                Err(err) => err_payload(err),
            }
        }
        ("POST", "/v1/git/sync") => handle_git_sync(engine, &req.body, agent).await,
        ("GET", "/v1/vault/pull") => handle_vault_pull(engine, agent).await,
        ("POST", "/v1/vault/push") => handle_vault_push(engine, &req.body, agent).await,
        ("POST", "/v1/recall") => handle_recall(engine, &req.body, agent).await,
        ("POST", "/v1/stage") => handle_stage(engine, &req.body, agent).await,
        ("POST", "/v1/inspect") => handle_inspect(engine, &req.body, agent).await,
        ("POST", "/v1/mcp") => handle_mcp(engine, &req.body, agent).await,
        ("POST", "/v1/kv/warm") => handle_kv_warm(engine, &req.body, agent).await,
        ("GET", "/v1/kv/stats") => handle_kv_stats(engine, req.query.as_deref()).await,
        ("GET", "/v1/auth/keys") => {
            if let Err(resp) = require_role(agent, AgentRole::Admin) {
                return resp;
            }
            match engine.list_api_keys().await {
                Ok(keys) => (200, "OK", json!({ "keys": keys })),
                Err(err) => err_payload(err),
            }
        }
        ("POST", "/v1/auth/keys") => handle_issue_key(engine, &req.body, agent).await,
        ("POST", "/v1/auth/keys/revoke") => handle_revoke_key(engine, &req.body, agent).await,
        _ => (
            404,
            "Not Found",
            json!({ "error": format!("unknown route {} {}", req.method, req.path) }),
        ),
    }
}

async fn handle_inbox(engine: &VaultEngine, agent: Option<&ScopedApiKey>) -> HttpTuple {
    let pending_res = match agent {
        Some(a) => engine.pending_inbox_as(a).await,
        None => engine.pending_inbox().await,
    };
    match pending_res {
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
    }
}

async fn handle_decide(
    engine: &VaultEngine,
    body: &[u8],
    agent: Option<&ScopedApiKey>,
) -> HttpTuple {
    if let Err(resp) = require_role(agent, AgentRole::Admin) {
        return resp;
    }
    let parsed: DecideRequest = match parse_json_body(body) {
        Ok(req) => req,
        Err(resp) => return resp,
    };
    let cid = match CandidateId::parse(&parsed.candidate_id) {
        Ok(id) => id,
        Err(err) => return err_payload(err),
    };
    if let Some(a) = agent {
        if !a.has_wildcard_scope() {
            match engine.pending_inbox().await {
                Ok(pending) => {
                    if let Some(entry) = pending.iter().find(|e| e.candidate_id == cid) {
                        if let Err(err) = a.authorize_write_scope(&entry.scope) {
                            return auth_err_payload(err);
                        }
                    }
                }
                Err(err) => return err_payload(err),
            }
        }
    }
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

async fn handle_git_sync(
    engine: &VaultEngine,
    body: &[u8],
    agent: Option<&ScopedApiKey>,
) -> HttpTuple {
    if let Err(resp) = require_role(agent, AgentRole::Admin) {
        return resp;
    }
    let parsed: GitSyncHttpRequest = if body.iter().all(|b| b.is_ascii_whitespace()) {
        GitSyncHttpRequest::default()
    } else {
        match parse_json_body(body) {
            Ok(req) => req,
            Err(resp) => return resp,
        }
    };
    let mut opts = GitSyncOptions::new().with_push(parsed.push);
    if let Some(remote) = parsed.remote.filter(|r| !r.trim().is_empty()) {
        opts = opts.with_remote(remote);
    }
    if let Some(branch) = parsed.branch.filter(|b| !b.trim().is_empty()) {
        opts = opts.with_branch(branch);
    }
    if let Some(msg) = parsed.commit_message.filter(|m| !m.trim().is_empty()) {
        opts = opts.with_commit_message(msg);
    }
    match engine.git_sync(opts).await {
        Ok(rep) => ok_payload(rep),
        Err(err) => err_payload(err),
    }
}

async fn handle_vault_pull(engine: &VaultEngine, agent: Option<&ScopedApiKey>) -> HttpTuple {
    let res = match agent {
        Some(a) => engine.pull_vault_state_as(a).await,
        None => engine.pull_vault_state().await,
    };
    match res {
        Ok(rep) => ok_payload(rep),
        Err(err) => err_payload(err),
    }
}

async fn handle_vault_push(
    engine: &VaultEngine,
    body: &[u8],
    agent: Option<&ScopedApiKey>,
) -> HttpTuple {
    let parsed: VaultPushRequest = if body.iter().all(|b| b.is_ascii_whitespace()) {
        VaultPushRequest::default()
    } else {
        match parse_json_body(body) {
            Ok(req) => req,
            Err(resp) => return resp,
        }
    };
    let res = match agent {
        Some(a) => engine.push_vault_changes_as(a, parsed).await,
        None => engine.push_vault_changes(parsed).await,
    };
    match res {
        Ok(rep) => ok_payload(rep),
        Err(err) => err_payload(err),
    }
}

async fn handle_recall(
    engine: &VaultEngine,
    body: &[u8],
    agent: Option<&ScopedApiKey>,
) -> HttpTuple {
    let parsed: RecallHttpRequest = match parse_json_body(body) {
        Ok(req) => req,
        Err(resp) => return resp,
    };
    let mut req = RecallRequest::new(parsed.query);
    req.include_hot_profile = parsed.include_hot_profile;
    if let Some(scope_raw) = parsed.scope.filter(|s| !s.trim().is_empty()) {
        match ScopeTag::new(scope_raw) {
            Ok(s) => req.scope = Some(s),
            Err(err) => return err_payload(err),
        }
    }
    if let Some(max_tok) = parsed.max_tokens {
        match TokenBudget::new(max_tok) {
            Ok(b) => req.max_tokens = b,
            Err(err) => return err_payload(err),
        }
    }
    if let Some(model_raw) = parsed.kv_model.filter(|m| !m.trim().is_empty()) {
        match ModelId::new(model_raw) {
            Ok(m) => req.kv_model = Some(m),
            Err(err) => return err_payload(err),
        }
    }

    let res = match agent {
        Some(a) => engine.recall_as(a, req).await,
        None => engine.recall(req).await,
    };
    match res {
        Ok(rep) => ok_payload(rep),
        Err(err) => err_payload(err),
    }
}

async fn handle_stage(
    engine: &VaultEngine,
    body: &[u8],
    agent: Option<&ScopedApiKey>,
) -> HttpTuple {
    let parsed: StageHttpRequest = match parse_json_body(body) {
        Ok(req) => req,
        Err(resp) => return resp,
    };
    let tier = match parsed.suggested_tier {
        Some(ref raw) => match VaultTier::parse_str(raw) {
            Some(t) => t,
            None => {
                return (
                    400,
                    "Bad Request",
                    json!({ "error": format!("suggested_tier must be hot, warm, or cold (got `{raw}`)") }),
                );
            }
        },
        None => VaultTier::Warm,
    };

    let mut req = StageMemoryRequest::new(parsed.proposed_fact, tier);
    if let Some(scope_raw) = parsed.scope.filter(|s| !s.trim().is_empty()) {
        match ScopeTag::new(scope_raw) {
            Ok(s) => req.scope = s,
            Err(err) => return err_payload(err),
        }
    } else if let Some(a) = agent {
        req.scope = a.default_write_scope();
    }
    if let Some(target) = parsed.target_note.filter(|t| !t.trim().is_empty()) {
        req = req.with_target_note(target);
    }
    if let Some(sup_raw) = parsed.supersedes_amu_id.filter(|s| !s.trim().is_empty()) {
        match AmuId::parse(&sup_raw) {
            Ok(id) => req = req.with_supersedes(id),
            Err(err) => return err_payload(err),
        }
    }
    if let Some(by) = parsed.proposed_by.filter(|p| !p.trim().is_empty()) {
        req.proposed_by = by;
    }

    let res = match agent {
        Some(a) => engine.stage_memory_as(a, req).await,
        None => engine.stage_memory(req).await,
    };
    match res {
        Ok(cid) => (
            200,
            "OK",
            json!({
                "candidate_id": cid.as_str(),
                "inbox_path": INBOX_REL_PATH,
            }),
        ),
        Err(err) => err_payload(err),
    }
}

async fn handle_inspect(
    engine: &VaultEngine,
    body: &[u8],
    agent: Option<&ScopedApiKey>,
) -> HttpTuple {
    let parsed: InspectHttpRequest = match parse_json_body(body) {
        Ok(req) => req,
        Err(resp) => return resp,
    };
    let res = match agent {
        Some(a) => engine.inspect_source_as(a, &parsed.source_pointer).await,
        None => engine.inspect_source(&parsed.source_pointer).await,
    };
    match res {
        Ok(content) => (
            200,
            "OK",
            json!({
                "source_pointer": parsed.source_pointer,
                "content": content,
            }),
        ),
        Err(err) => err_payload(err),
    }
}

fn handle_openapi(headers: &[(String, String)]) -> HttpTuple {
    let server_url = infer_server_url(headers);
    (200, "OK", build_openapi_spec(&server_url))
}

fn infer_server_url(headers: &[(String, String)]) -> String {
    let scheme = find_header(headers, "x-forwarded-proto")
        .and_then(|raw| raw.split(',').next())
        .map(str::trim)
        .map(str::to_ascii_lowercase)
        .filter(|proto| matches!(proto.as_str(), "http" | "https"))
        .unwrap_or_else(|| "http".to_string());

    let host = find_header(headers, "x-forwarded-host")
        .or_else(|| find_header(headers, "host"))
        .and_then(|raw| raw.split(',').next())
        .map(str::trim)
        .filter(|candidate| is_valid_http_host(candidate));

    match host {
        Some(h) => format!("{scheme}://{h}"),
        None => "http://127.0.0.1:4771".to_string(),
    }
}

fn is_valid_http_host(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b':' | b'[' | b']'))
}

fn error_response_spec(description: &str) -> Value {
    json!({
        "description": description,
        "content": {
            "application/json": {
                "schema": { "$ref": "#/components/schemas/ErrorResponse" }
            }
        }
    })
}

fn build_openapi_spec(server_url: &str) -> Value {
    json!({
        "openapi": "3.1.0",
        "info": {
            "title": "Aether Vault Control Plane API",
            "version": env!("CARGO_PKG_VERSION"),
            "description": "Obsidian-native memory vault HTTP API for hybrid recall, Inbox memory staging, provenance inspection, and health status.",
            "license": {
                "name": "Apache-2.0"
            }
        },
        "servers": [
            {
                "url": server_url
            }
        ],
        "security": [
            { "bearerAuth": [] }
        ],
        "paths": {
            "/v1/status": {
                "get": {
                    "operationId": "getVaultStatus",
                    "summary": "Get vault health and index status",
                    "description": "Returns active AMU counts, pending and approved Inbox counts, HOT prefix token usage, and the deterministic prefix hash.",
                    "responses": {
                        "200": {
                            "description": "Current vault health and index metrics",
                            "content": {
                                "application/json": {
                                    "schema": { "$ref": "#/components/schemas/VaultStatusReport" }
                                }
                            }
                        },
                        "401": error_response_spec("Missing, invalid, expired, or revoked bearer token")
                    }
                }
            },
            "/v1/recall": {
                "post": {
                    "operationId": "recallMemory",
                    "summary": "Retrieve distilled, source-grounded memory from the vault",
                    "description": "Runs hybrid lexical, vector, and Personalized PageRank recall across Hot, Warm, and Cold memory tiers while excluding private notes and respecting agent scopes.",
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/RecallHttpRequest" }
                            }
                        }
                    },
                    "responses": {
                        "200": {
                            "description": "Token-budgeted recall context with static/dynamic prefix split and source-grounded AMUs",
                            "content": {
                                "application/json": {
                                    "schema": { "$ref": "#/components/schemas/RecallResponse" }
                                }
                            }
                        },
                        "400": error_response_spec("Invalid recall request payload, scope, token budget, or model ID"),
                        "401": error_response_spec("Missing, invalid, expired, or revoked bearer token"),
                        "403": error_response_spec("Caller role or scope is not permitted to query the requested scope")
                    }
                }
            },
            "/v1/stage": {
                "post": {
                    "operationId": "stageMemory",
                    "summary": "Stage a proposed fact in Aether Inbox for human approval",
                    "description": "Appends a candidate memory unit to Aether/Inbox.md for human review before compiling into active vault memory. Requires Contributor or Admin role.",
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/StageHttpRequest" }
                            }
                        }
                    },
                    "responses": {
                        "200": {
                            "description": "Candidate staged into Aether/Inbox.md",
                            "content": {
                                "application/json": {
                                    "schema": { "$ref": "#/components/schemas/StageMemoryResponse" }
                                }
                            }
                        },
                        "400": error_response_spec("Invalid stage payload, tier, scope, or supersedes_amu_id"),
                        "401": error_response_spec("Missing, invalid, expired, or revoked bearer token"),
                        "403": error_response_spec("Caller lacks Contributor+ role or write authorization for the target scope")
                    }
                }
            },
            "/v1/inspect": {
                "post": {
                    "operationId": "inspectSource",
                    "summary": "Read the verbatim Markdown section for a source pointer",
                    "description": "Fetches exact Markdown source text for a validated provenance pointer (for example Arch/DB.md#Schema) while enforcing traversal, ignore, privacy, and scope guards.",
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/InspectHttpRequest" }
                            }
                        }
                    },
                    "responses": {
                        "200": {
                            "description": "Verbatim Markdown content for the requested source pointer",
                            "content": {
                                "application/json": {
                                    "schema": { "$ref": "#/components/schemas/InspectSourceResponse" }
                                }
                            }
                        },
                        "400": error_response_spec("Invalid or forbidden source pointer"),
                        "401": error_response_spec("Missing, invalid, expired, or revoked bearer token"),
                        "403": error_response_spec("Caller lacks read authorization for the source scope"),
                        "404": error_response_spec("Referenced note or heading section does not exist")
                    }
                }
            }
        },
        "components": {
            "securitySchemes": {
                "bearerAuth": {
                    "type": "http",
                    "scheme": "bearer",
                    "bearerFormat": "aek_<64-hex>",
                    "description": "Scoped Aether Vault API key sent as Authorization: Bearer aek_<64-hex>. Required when the server runs with --require-auth."
                }
            },
            "schemas": {
                "VaultStatusReport": {
                    "type": "object",
                    "required": [
                        "status",
                        "schema_version",
                        "embedder_id",
                        "embedding_dim",
                        "active_amus",
                        "pending_inbox_count",
                        "approved_inbox_count",
                        "hot_tokens_used",
                        "hot_token_budget",
                        "prefix_hash"
                    ],
                    "properties": {
                        "status": { "type": "string" },
                        "schema_version": { "type": "integer", "minimum": 1 },
                        "embedder_id": { "type": "string" },
                        "embedding_dim": { "type": "integer", "minimum": 1 },
                        "active_amus": { "type": "integer", "minimum": 0 },
                        "pending_inbox_count": { "type": "integer", "minimum": 0 },
                        "approved_inbox_count": { "type": "integer", "minimum": 0 },
                        "hot_tokens_used": { "type": "integer", "minimum": 0 },
                        "hot_token_budget": { "type": "integer", "minimum": 1 },
                        "prefix_hash": { "type": "string", "description": "64-character lowercase SHA-256 hex digest of the static HOT prefix" }
                    }
                },
                "RecallHttpRequest": {
                    "type": "object",
                    "required": ["query"],
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Natural language or keyword query for Warm/Cold recall"
                        },
                        "scope": {
                            "type": "string",
                            "description": "Optional scope filter (e.g. engineering, global); intersected with caller scopes"
                        },
                        "max_tokens": {
                            "type": "integer",
                            "minimum": 1,
                            "default": 1500,
                            "description": "Strict positive token ceiling across static_prefix and dynamic_context"
                        },
                        "include_hot_profile": {
                            "type": "boolean",
                            "default": true,
                            "description": "Include the deterministic byte-stable HOT profile prefix block"
                        },
                        "kv_model": {
                            "type": "string",
                            "description": "Optional model identifier for PagedAttention/CacheBlend KV-cache reuse hints"
                        }
                    }
                },
                "Amu": {
                    "type": "object",
                    "required": [
                        "amu_id",
                        "tier",
                        "kind",
                        "status",
                        "heading_path",
                        "distilled_fact",
                        "source_pointer",
                        "graph_edges",
                        "valid_from",
                        "superseded_by",
                        "scopes",
                        "authority_weight",
                        "content_hash"
                    ],
                    "properties": {
                        "amu_id": { "type": "string" },
                        "tier": { "type": "string", "enum": ["hot", "warm", "cold"] },
                        "kind": { "type": "string", "enum": ["profile", "rule", "decision", "spec", "fleeting"] },
                        "status": { "type": "string", "enum": ["active", "superseded", "archived"] },
                        "heading_path": { "type": "string" },
                        "distilled_fact": { "type": "string" },
                        "source_pointer": { "type": "string" },
                        "graph_edges": {
                            "type": "array",
                            "items": { "type": "string" }
                        },
                        "valid_from": { "type": "string", "format": "date-time" },
                        "superseded_by": { "type": ["string", "null"] },
                        "scopes": {
                            "type": "array",
                            "items": { "type": "string" }
                        },
                        "authority_weight": { "type": "number" },
                        "content_hash": { "type": "string" }
                    }
                },
                "RecallEntry": {
                    "type": "object",
                    "required": ["amu", "score"],
                    "properties": {
                        "amu": { "$ref": "#/components/schemas/Amu" },
                        "score": { "type": "number" }
                    }
                },
                "KvRecallHint": {
                    "type": "object",
                    "required": [
                        "model_id",
                        "prefix_block_hashes",
                        "cache_blend_hashes",
                        "cache_hit_tokens",
                        "total_tokens",
                        "estimated_ttft_saved_ms"
                    ],
                    "properties": {
                        "model_id": { "type": "string" },
                        "prefix_block_hashes": { "type": "array", "items": { "type": "string" } },
                        "cache_blend_hashes": { "type": "array", "items": { "type": "string" } },
                        "cache_hit_tokens": { "type": "integer", "minimum": 0 },
                        "total_tokens": { "type": "integer", "minimum": 0 },
                        "estimated_ttft_saved_ms": { "type": "number" }
                    }
                },
                "RecallResponse": {
                    "type": "object",
                    "required": [
                        "formatted_context",
                        "hot_prefix",
                        "static_prefix",
                        "dynamic_context",
                        "hot_amus",
                        "warm_cold_amus",
                        "entries",
                        "static_prefix_tokens",
                        "dynamic_tokens",
                        "tokens_used",
                        "prefix_hash"
                    ],
                    "properties": {
                        "formatted_context": { "type": "string" },
                        "hot_prefix": { "type": "string" },
                        "static_prefix": { "type": "string" },
                        "dynamic_context": { "type": "string" },
                        "hot_amus": {
                            "type": "array",
                            "items": { "$ref": "#/components/schemas/Amu" }
                        },
                        "warm_cold_amus": {
                            "type": "array",
                            "items": { "$ref": "#/components/schemas/Amu" }
                        },
                        "entries": {
                            "type": "array",
                            "items": { "$ref": "#/components/schemas/RecallEntry" }
                        },
                        "static_prefix_tokens": { "type": "integer", "minimum": 0 },
                        "dynamic_tokens": { "type": "integer", "minimum": 0 },
                        "tokens_used": { "type": "integer", "minimum": 0 },
                        "prefix_hash": { "type": "string" },
                        "kv_hint": { "$ref": "#/components/schemas/KvRecallHint" }
                    }
                },
                "StageHttpRequest": {
                    "type": "object",
                    "required": ["proposed_fact"],
                    "properties": {
                        "proposed_fact": {
                            "type": "string",
                            "description": "Concise, atomic fact or decision to propose for Obsidian review"
                        },
                        "suggested_tier": {
                            "type": "string",
                            "enum": ["hot", "warm", "cold"],
                            "default": "warm",
                            "description": "Suggested memory tier upon approval"
                        },
                        "scope": {
                            "type": "string",
                            "description": "Target memory scope (defaults to the caller write scope or global)"
                        },
                        "target_note": {
                            "type": "string",
                            "description": "Suggested Obsidian note path or [[wikilink]] topic"
                        },
                        "supersedes_amu_id": {
                            "type": "string",
                            "description": "Optional amu_<32-hex> or UUID of an active AMU this candidate replaces"
                        },
                        "proposed_by": {
                            "type": "string",
                            "description": "Optional author label (overridden by agent_id for scoped keys)"
                        }
                    }
                },
                "StageMemoryResponse": {
                    "type": "object",
                    "required": ["candidate_id", "inbox_path"],
                    "properties": {
                        "candidate_id": { "type": "string" },
                        "inbox_path": { "type": "string" }
                    }
                },
                "InspectHttpRequest": {
                    "type": "object",
                    "required": ["source_pointer"],
                    "properties": {
                        "source_pointer": {
                            "type": "string",
                            "description": "Vault path and optional heading anchor (e.g. Arch/DB.md#Schema)"
                        }
                    }
                },
                "InspectSourceResponse": {
                    "type": "object",
                    "required": ["source_pointer", "content"],
                    "properties": {
                        "source_pointer": { "type": "string" },
                        "content": { "type": "string" }
                    }
                },
                "ErrorResponse": {
                    "type": "object",
                    "required": ["error"],
                    "properties": {
                        "error": { "type": "string" }
                    }
                }
            }
        }
    })
}

async fn handle_mcp(
    engine: &Arc<VaultEngine>,
    body: &[u8],
    agent: Option<&ScopedApiKey>,
) -> HttpTuple {
    let rpc_req: Value = match parse_json_body(body) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let mcp = match agent {
        Some(a) => McpServer::scoped(Arc::clone(engine), a.clone()),
        None => McpServer::new(Arc::clone(engine)),
    };
    match mcp.handle_jsonrpc(rpc_req).await {
        Some(resp) => (200, "OK", resp),
        None => (200, "OK", json!({})),
    }
}

async fn handle_kv_warm(
    engine: &VaultEngine,
    body: &[u8],
    agent: Option<&ScopedApiKey>,
) -> HttpTuple {
    let parsed: WarmKvHttpRequest = match parse_json_body(body) {
        Ok(req) => req,
        Err(resp) => return resp,
    };
    let model_id = match ModelId::new(parsed.model_id) {
        Ok(m) => m,
        Err(err) => return err_payload(err),
    };
    let mut req = WarmKvRequest::new(model_id);
    req.include_hot_prefix = parsed.include_hot_prefix;
    req.include_cache_blend = parsed.include_cache_blend;

    if let Some(scope_raw) = parsed.scope.filter(|s| !s.trim().is_empty()) {
        match ScopeTag::new(scope_raw) {
            Ok(s) => req.scope = Some(s),
            Err(err) => return err_payload(err),
        }
    }
    if let Some(raw_ids) = parsed.amu_ids {
        let mut parsed_ids = Vec::with_capacity(raw_ids.len());
        for raw in raw_ids {
            match AmuId::parse(&raw) {
                Ok(id) => parsed_ids.push(id),
                Err(err) => return err_payload(err),
            }
        }
        req.amu_ids = Some(parsed_ids);
    }
    if let Some(tier_raw) = parsed.target_tier {
        match KvStorageTier::parse_str(&tier_raw) {
            Some(t) => req.target_tier = t,
            None => {
                return (
                    400,
                    "Bad Request",
                    json!({ "error": format!("invalid target_tier `{tier_raw}`") }),
                );
            }
        }
    }
    if let Some(bt) = parsed.block_tokens {
        if bt == 0 {
            return (
                400,
                "Bad Request",
                json!({ "error": "block_tokens must be greater than zero" }),
            );
        }
        req.block_tokens = bt;
    }

    let res = match agent {
        Some(a) => engine.warm_kv_cache_as(a, req).await,
        None => engine.warm_kv_cache(req).await,
    };
    match res {
        Ok(rep) => ok_payload(rep),
        Err(err) => err_payload(err),
    }
}

async fn handle_kv_stats(engine: &VaultEngine, query: Option<&str>) -> HttpTuple {
    let model_id = match extract_query_param(query, "model_id") {
        Some(raw) if !raw.trim().is_empty() => match ModelId::new(raw) {
            Ok(m) => Some(m),
            Err(err) => return err_payload(err),
        },
        _ => None,
    };
    match engine.kv_stats(model_id.as_ref()).await {
        Ok(stats) => ok_payload(stats),
        Err(err) => err_payload(err),
    }
}

async fn handle_issue_key(
    engine: &VaultEngine,
    body: &[u8],
    agent: Option<&ScopedApiKey>,
) -> HttpTuple {
    if let Err(resp) = require_role(agent, AgentRole::Admin) {
        return resp;
    }
    let parsed: IssueKeyHttpRequest = match parse_json_body(body) {
        Ok(req) => req,
        Err(resp) => return resp,
    };
    let agent_id = match AgentId::new(parsed.agent_id) {
        Ok(id) => id,
        Err(err) => return err_payload(err),
    };
    let Some(role) = AgentRole::parse_str(&parsed.role) else {
        return (
            400,
            "Bad Request",
            json!({ "error": format!("role must be reader, contributor, or admin (got `{}`)", parsed.role) }),
        );
    };
    let mut scopes = Vec::with_capacity(parsed.scopes.len());
    for raw_scope in parsed.scopes {
        match ScopeTag::new(raw_scope) {
            Ok(s) => scopes.push(s),
            Err(err) => return err_payload(err),
        }
    }
    let mut req = match IssueApiKeyRequest::new(agent_id, role, scopes) {
        Ok(r) => r,
        Err(err) => return err_payload(err),
    };
    if let Some(exp_raw) = parsed.expires_at.filter(|s| !s.trim().is_empty()) {
        match DateTime::parse_from_rfc3339(&exp_raw) {
            Ok(dt) => req = req.with_expires_at(dt.with_timezone(&Utc)),
            Err(err) => {
                return (
                    400,
                    "Bad Request",
                    json!({ "error": format!("invalid RFC 3339 expires_at `{exp_raw}`: {err}") }),
                );
            }
        }
    }

    match engine.issue_api_key(req).await {
        Ok(issued) => (
            200,
            "OK",
            json!({
                "key": issued.key,
                "secret": issued.secret.expose_secret(),
            }),
        ),
        Err(err) => err_payload(err),
    }
}

async fn handle_revoke_key(
    engine: &VaultEngine,
    body: &[u8],
    agent: Option<&ScopedApiKey>,
) -> HttpTuple {
    if let Err(resp) = require_role(agent, AgentRole::Admin) {
        return resp;
    }
    let parsed: RevokeKeyHttpRequest = match parse_json_body(body) {
        Ok(req) => req,
        Err(resp) => return resp,
    };
    let key_id = match KeyId::parse(&parsed.key_id) {
        Ok(id) => id,
        Err(err) => return err_payload(err),
    };
    match engine.revoke_api_key(&key_id).await {
        Ok(revoked) => (
            200,
            "OK",
            json!({
                "key_id": key_id.as_str(),
                "revoked": revoked,
            }),
        ),
        Err(err) => err_payload(err),
    }
}

fn require_role(
    agent: Option<&ScopedApiKey>,
    required: AgentRole,
) -> std::result::Result<(), HttpTuple> {
    if let Some(a) = agent {
        a.require_role(required).map_err(auth_err_payload)?;
    }
    Ok(())
}

fn parse_json_body<T: DeserializeOwned>(body: &[u8]) -> std::result::Result<T, HttpTuple> {
    serde_json::from_slice(body).map_err(|err| {
        (
            400,
            "Bad Request",
            json!({ "error": format!("invalid json body: {err}") }),
        )
    })
}

fn extract_query_param(query: Option<&str>, key: &str) -> Option<String> {
    let q = query?;
    for pair in q.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            if k == key {
                return Some(percent_decode(v));
            }
        }
    }
    None
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut idx = 0usize;
    while idx < bytes.len() {
        if bytes[idx] == b'%' && idx + 2 < bytes.len() {
            if let Ok(hex_str) = std::str::from_utf8(&bytes[idx + 1..idx + 3]) {
                if let Ok(val) = u8::from_str_radix(hex_str, 16) {
                    out.push(val);
                    idx += 3;
                    continue;
                }
            }
        } else if bytes[idx] == b'+' {
            out.push(b' ');
            idx += 1;
            continue;
        }
        out.push(bytes[idx]);
        idx += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn ok_payload<T: serde::Serialize>(val: T) -> HttpTuple {
    match serde_json::to_value(val) {
        Ok(v) => (200, "OK", v),
        Err(err) => (
            500,
            "Internal Server Error",
            json!({ "error": format!("serialization failure: {err}") }),
        ),
    }
}

fn auth_err_payload(err: AuthError) -> HttpTuple {
    let (code, reason) = match &err {
        AuthError::MissingToken
        | AuthError::InvalidToken
        | AuthError::RevokedToken(_)
        | AuthError::ExpiredToken(_) => (401, "Unauthorized"),
        AuthError::InsufficientRole { .. } | AuthError::ScopeDenied { .. } => (403, "Forbidden"),
    };
    (code, reason, json!({ "error": err.to_string() }))
}

fn err_payload(err: AetherError) -> HttpTuple {
    let (code, reason) = match &err {
        AetherError::InvalidInput(_) => (400, "Bad Request"),
        AetherError::NotFound(_) => (404, "Not Found"),
        AetherError::Permanent {
            connector: "vault-auth",
            message,
        } => {
            if message.starts_with("missing bearer")
                || message.starts_with("invalid API key")
                || message.starts_with("API key ")
            {
                (401, "Unauthorized")
            } else {
                (403, "Forbidden")
            }
        }
        _ => (500, "Internal Server Error"),
    };
    (code, reason, json!({ "error": err.to_string() }))
}

async fn read_http_request(stream: &mut TcpStream) -> std::io::Result<Option<ParsedHttpRequest>> {
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
    let (path, query) = match raw_target.split_once('?') {
        Some((p, q)) => (p.to_string(), Some(q.to_string())),
        None => (raw_target.to_string(), None),
    };

    let mut content_length: usize = 0;
    let mut headers = Vec::new();
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            let name = k.trim().to_string();
            let val = v.trim().to_string();
            if name.eq_ignore_ascii_case("content-length") {
                content_length = val.parse().unwrap_or(0);
            }
            headers.push((name, val));
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
    Ok(Some(ParsedHttpRequest {
        method,
        path,
        query,
        headers,
        body,
    }))
}

fn find_double_crlf(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

async fn write_no_content(stream: &mut TcpStream) -> std::io::Result<()> {
    let resp = "HTTP/1.1 204 No Content\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type, Authorization\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
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
        "HTTP/1.1 {code} {reason}\r\nContent-Type: application/json\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type, Authorization\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
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
        http_call_with_auth(addr, method, path, None, body).await
    }

    async fn http_call_with_auth(
        addr: SocketAddr,
        method: &str,
        path: &str,
        bearer: Option<&str>,
        body: Option<&str>,
    ) -> (u16, Value) {
        let (code, _, parsed) = http_call_raw(addr, method, path, bearer, &[], body).await;
        (code, parsed)
    }

    async fn http_call_raw(
        addr: SocketAddr,
        method: &str,
        path: &str,
        bearer: Option<&str>,
        extra_headers: &[(&str, &str)],
        body: Option<&str>,
    ) -> (u16, String, Value) {
        let mut stream = TcpStream::connect(addr).await.unwrap();
        let body_str = body.unwrap_or("");
        let auth_hdr = match bearer {
            Some(tok) => format!("Authorization: Bearer {tok}\r\n"),
            None => String::new(),
        };
        let extra = extra_headers
            .iter()
            .map(|(k, v)| format!("{k}: {v}\r\n"))
            .collect::<String>();
        let req = format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n{auth_hdr}{extra}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body_str}",
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
        (code, head.to_string(), parsed)
    }

    fn git_run(dir: &std::path::Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
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

    #[tokio::test]
    async fn control_plane_with_auth_enforces_bearer_tokens_roles_and_team_endpoints() {
        let remote_dir = TempVault::new();
        git_run(&remote_dir.root, &["init", "--bare"]);

        let vault = TempVault::new();
        git_run(&vault.root, &["init"]);
        git_run(&vault.root, &["config", "user.name", "Team Test"]);
        git_run(&vault.root, &["config", "user.email", "team@aether.local"]);
        git_run(
            &vault.root,
            &["remote", "add", "origin", remote_dir.root.to_str().unwrap()],
        );

        vault.write(
            "Profile/Core.md",
            "---\naether_tier: hot\naether_scope:\n  - global\naether_kind: profile\n---\n# Core\nAlways write idiomatic Rust.\n",
        );
        vault.write(
            "Specs/Auth.md",
            "---\naether_tier: warm\naether_scope:\n  - engineering\n---\n# Auth\nRotate API keys via SHA-256 hashing.\n",
        );
        vault.write(
            "Finance/Budget.md",
            "---\naether_tier: warm\naether_scope:\n  - finance\n---\n# Budget\nConfidential payroll ledger 9988.\n",
        );
        git_run(&vault.root, &["add", "."]);
        git_run(&vault.root, &["commit", "-m", "initial team vault"]);
        git_run(&vault.root, &["branch", "-M", "main"]);
        git_run(&vault.root, &["push", "-u", "origin", "main"]);

        let engine = Arc::new(
            VaultEngine::open(VaultConfig::in_memory(&vault.root))
                .await
                .unwrap(),
        );
        let admin_secret = crate::auth::ApiKeySecret::generate();
        engine
            .ensure_bootstrap_admin_key(
                crate::auth::AgentId::new("bootstrap-admin").unwrap(),
                &admin_secret,
            )
            .await
            .unwrap();
        let admin_tok = admin_secret.expose_secret();

        let non_loopback_server = ControlPlaneServer::bind_with_auth(
            "0.0.0.0:0".parse().unwrap(),
            Arc::clone(&engine),
            true,
        )
        .await
        .unwrap();
        assert_ne!(non_loopback_server.local_addr().port(), 0);
        non_loopback_server.shutdown().await.unwrap();

        let server = ControlPlaneServer::bind_with_auth(
            "127.0.0.1:0".parse().unwrap(),
            Arc::clone(&engine),
            true,
        )
        .await
        .unwrap();
        let addr = server.local_addr();

        let (code, _) = http_call_with_auth(addr, "GET", "/v1/status", None, None).await;
        assert_eq!(code, 401);

        let (code, _) =
            http_call_with_auth(addr, "GET", "/v1/status", Some("aek_invalid0000"), None).await;
        assert_eq!(code, 401);

        let (code, reader_issued) = http_call_with_auth(
            addr,
            "POST",
            "/v1/auth/keys",
            Some(admin_tok),
            Some(r#"{"agent_id":"eng-reader","role":"reader","scopes":["engineering"]}"#),
        )
        .await;
        assert_eq!(code, 200);
        let reader_tok = reader_issued["secret"].as_str().unwrap().to_string();
        let reader_key_id = reader_issued["key"]["key_id"].as_str().unwrap().to_string();

        let (code, contrib_issued) = http_call_with_auth(
            addr,
            "POST",
            "/v1/auth/keys",
            Some(admin_tok),
            Some(r#"{"agent_id":"eng-contrib","role":"contributor","scopes":["engineering"]}"#),
        )
        .await;
        assert_eq!(code, 200);
        let contrib_tok = contrib_issued["secret"].as_str().unwrap().to_string();

        let (code, listed) =
            http_call_with_auth(addr, "GET", "/v1/auth/keys", Some(admin_tok), None).await;
        assert_eq!(code, 200);
        assert!(listed["keys"].as_array().unwrap().len() >= 3);

        let (code, _) =
            http_call_with_auth(addr, "GET", "/v1/auth/keys", Some(&reader_tok), None).await;
        assert_eq!(code, 403);

        let (code, recall_res) = http_call_with_auth(
            addr,
            "POST",
            "/v1/recall",
            Some(&reader_tok),
            Some(r#"{"query":"payroll ledger Auth SHA-256"}"#),
        )
        .await;
        assert_eq!(code, 200);
        let dyn_ctx = recall_res["dynamic_context"].as_str().unwrap();
        assert!(dyn_ctx.contains("Rotate API keys via SHA-256"));
        assert!(!dyn_ctx.contains("9988"));

        let (code, _) = http_call_with_auth(
            addr,
            "POST",
            "/v1/recall",
            Some(&reader_tok),
            Some(r#"{"query":"payroll","scope":"finance"}"#),
        )
        .await;
        assert_eq!(code, 403);

        let (code, _) = http_call_with_auth(
            addr,
            "POST",
            "/v1/stage",
            Some(&reader_tok),
            Some(r#"{"proposed_fact":"Reader cannot stage."}"#),
        )
        .await;
        assert_eq!(code, 403);

        let (code, staged) = http_call_with_auth(
            addr,
            "POST",
            "/v1/stage",
            Some(&contrib_tok),
            Some(r#"{"proposed_fact":"Use PKCE for team OAuth."}"#),
        )
        .await;
        assert_eq!(code, 200);
        let staged_cid = staged["candidate_id"].as_str().unwrap().to_string();
        let pending = engine.pending_inbox().await.unwrap();
        let entry = pending
            .iter()
            .find(|e| e.candidate_id.as_str() == staged_cid)
            .unwrap();
        assert_eq!(entry.scope.as_str(), "engineering");
        assert_eq!(entry.proposed_by, "eng-contrib");

        let (code, _) = http_call_with_auth(
            addr,
            "POST",
            "/v1/stage",
            Some(&contrib_tok),
            Some(r#"{"proposed_fact":"Cross scope blocked.","scope":"finance"}"#),
        )
        .await;
        assert_eq!(code, 403);

        let (code, inspected) = http_call_with_auth(
            addr,
            "POST",
            "/v1/inspect",
            Some(&reader_tok),
            Some(r#"{"source_pointer":"Specs/Auth.md#Auth"}"#),
        )
        .await;
        assert_eq!(code, 200);
        assert!(inspected["content"]
            .as_str()
            .unwrap()
            .contains("SHA-256 hashing"));

        let (code, _) = http_call_with_auth(
            addr,
            "POST",
            "/v1/inspect",
            Some(&reader_tok),
            Some(r#"{"source_pointer":"Finance/Budget.md#Budget"}"#),
        )
        .await;
        assert_eq!(code, 403);

        let (code, mcp_res) = http_call_with_auth(
            addr,
            "POST",
            "/v1/mcp",
            Some(&reader_tok),
            Some(
                r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"aether_recall","arguments":{"query":"payroll Auth","structured":true}}}"#,
            ),
        )
        .await;
        assert_eq!(code, 200);
        assert_eq!(mcp_res["result"]["isError"], false);
        let mcp_dyn = mcp_res["result"]["structuredContent"]["dynamic_context"]
            .as_str()
            .unwrap();
        assert!(mcp_dyn.contains("SHA-256"));
        assert!(!mcp_dyn.contains("9988"));

        let (code, _) = http_call_with_auth(
            addr,
            "POST",
            "/v1/kv/warm",
            Some(&reader_tok),
            Some(r#"{"model_id":"llama-3.1-8b"}"#),
        )
        .await;
        assert_eq!(code, 403);

        let (code, warm_res) = http_call_with_auth(
            addr,
            "POST",
            "/v1/kv/warm",
            Some(&contrib_tok),
            Some(r#"{"model_id":"llama-3.1-8b"}"#),
        )
        .await;
        assert_eq!(code, 200);
        assert!(warm_res["blocks_warmed"].as_u64().unwrap() >= 1);
        assert!(warm_res["ttft_reduction_pct"].as_f64().unwrap() >= 80.0);

        let (code, kv_stats) = http_call_with_auth(
            addr,
            "GET",
            "/v1/kv/stats?model_id=llama-3.1-8b",
            Some(&reader_tok),
            None,
        )
        .await;
        assert_eq!(code, 200);
        assert!(kv_stats["persisted_blocks"].as_u64().unwrap() >= 1);

        let (code, _) =
            http_call_with_auth(addr, "POST", "/v1/git/sync", Some(&contrib_tok), Some("{}")).await;
        assert_eq!(code, 403);

        let (code, git_res) =
            http_call_with_auth(addr, "POST", "/v1/git/sync", Some(admin_tok), Some("{}")).await;
        assert_eq!(code, 200);
        assert_eq!(git_res["merged_cleanly"], true);

        let revoke_body = json!({ "key_id": reader_key_id }).to_string();
        let (code, revoked) = http_call_with_auth(
            addr,
            "POST",
            "/v1/auth/keys/revoke",
            Some(admin_tok),
            Some(&revoke_body),
        )
        .await;
        assert_eq!(code, 200);
        assert_eq!(revoked["revoked"], true);

        let (code, _) =
            http_call_with_auth(addr, "GET", "/v1/status", Some(&reader_tok), None).await;
        assert_eq!(code, 401);

        server.shutdown().await.unwrap();
        engine.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn control_plane_vault_pull_push_feeds_mcp_queries_and_enforces_scopes() {
        let server_vault = TempVault::new();
        let engine = Arc::new(
            VaultEngine::open(VaultConfig::in_memory(&server_vault.root))
                .await
                .unwrap(),
        );
        let admin_secret = crate::auth::ApiKeySecret::generate();
        engine
            .ensure_bootstrap_admin_key(
                crate::auth::AgentId::new("bootstrap-admin").unwrap(),
                &admin_secret,
            )
            .await
            .unwrap();
        let admin_tok = admin_secret.expose_secret();

        let server = ControlPlaneServer::bind_with_auth(
            "127.0.0.1:0".parse().unwrap(),
            Arc::clone(&engine),
            true,
        )
        .await
        .unwrap();
        let addr = server.local_addr();

        let (_, reader_issued) = http_call_with_auth(
            addr,
            "POST",
            "/v1/auth/keys",
            Some(admin_tok),
            Some(r#"{"agent_id":"mcp-reader","role":"reader","scopes":["engineering"]}"#),
        )
        .await;
        let reader_tok = reader_issued["secret"].as_str().unwrap().to_string();

        let (_, contrib_issued) = http_call_with_auth(
            addr,
            "POST",
            "/v1/auth/keys",
            Some(admin_tok),
            Some(r#"{"agent_id":"obsidian-plugin","role":"contributor","scopes":["engineering"]}"#),
        )
        .await;
        let contrib_tok = contrib_issued["secret"].as_str().unwrap().to_string();

        let (code, _) = http_call_with_auth(
            addr,
            "POST",
            "/v1/vault/push",
            Some(&reader_tok),
            Some(r#"{"notes":[{"path":"Arch/Cache.md","content":"hello"}]}"#),
        )
        .await;
        assert_eq!(code, 403);

        let (code, _) = http_call_with_auth(
            addr,
            "POST",
            "/v1/vault/push",
            Some(&contrib_tok),
            Some(r#"{"notes":[{"path":"Finance/Budget.md","content":"---\naether_scope: finance\n---\n# Budget\nSecret 99"}]}"#),
        )
        .await;
        assert_eq!(code, 403);

        let push_payload = json!({
            "notes": [
                {
                    "path": "Arch/Cache.md",
                    "content": "---\naether_tier: warm\naether_scope:\n  - engineering\n---\n# Cache\nUse SGLang RadixAttention for prefix reuse.\n"
                },
                {
                    "path": "Personal/Private.md",
                    "content": "---\naether_tier: private\n---\n# Private\nsecret_vault_entry_xyz\n"
                }
            ]
        })
        .to_string();
        let (code, pushed) = http_call_with_auth(
            addr,
            "POST",
            "/v1/vault/push",
            Some(&contrib_tok),
            Some(&push_payload),
        )
        .await;
        assert_eq!(code, 200);
        assert_eq!(pushed["sync_report"]["files_updated"], 1);
        assert_eq!(pushed["sync_report"]["private_skipped"], 1);

        let (code, pulled) =
            http_call_with_auth(addr, "GET", "/v1/vault/pull", Some(&reader_tok), None).await;
        assert_eq!(code, 200);
        assert!(pulled["manifest"]["Arch/Cache.md"].is_string());
        assert!(pulled["manifest"].get("Personal/Private.md").is_none());

        let (code, mcp_recall) = http_call_with_auth(
            addr,
            "POST",
            "/v1/mcp",
            Some(&reader_tok),
            Some(
                r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"aether_recall","arguments":{"query":"RadixAttention secret_vault_entry_xyz","structured":true}}}"#,
            ),
        )
        .await;
        assert_eq!(code, 200);
        let dyn_ctx = mcp_recall["result"]["structuredContent"]["dynamic_context"]
            .as_str()
            .unwrap();
        assert!(dyn_ctx.contains("RadixAttention"));
        assert!(!dyn_ctx.contains("secret_vault_entry_xyz"));

        let (code, mcp_inspect) = http_call_with_auth(
            addr,
            "POST",
            "/v1/mcp",
            Some(&reader_tok),
            Some(
                r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"aether_inspect_source","arguments":{"source_pointer":"Arch/Cache.md#Cache"}}}"#,
            ),
        )
        .await;
        assert_eq!(code, 200);
        assert!(mcp_inspect["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Use SGLang RadixAttention"));

        let (code, _) = http_call_with_auth(
            addr,
            "POST",
            "/v1/vault/push",
            Some(&contrib_tok),
            Some(r#"{"run_dream":true}"#),
        )
        .await;
        assert_eq!(code, 403);

        let (code, _) = http_call_with_auth(
            addr,
            "POST",
            "/v1/vault/push",
            Some(&contrib_tok),
            Some(r#"{"inbox_markdown":"- [x] **[WARM | engineering]** Direct approval\n  - `amu_candidate_id`: `cand_abcdef`"}"#),
        )
        .await;
        assert_eq!(code, 403);

        let (code, dream_pushed) = http_call_with_auth(
            addr,
            "POST",
            "/v1/vault/push",
            Some(admin_tok),
            Some(
                r#"{"notes":[{"path":"Daily/2026-11-19.md","content":"---\naether_scope: engineering\n---\n# Daily\n## Decisions\n- Decision: Always validate VaultPath at API boundaries."}],"run_dream":true}"#,
            ),
        )
        .await;
        assert_eq!(code, 200);
        assert_eq!(dream_pushed["dream_report"]["candidates_staged"], 1);
        assert!(dream_pushed["inbox_markdown"]
            .as_str()
            .unwrap()
            .contains("Always validate VaultPath"));

        let delete_payload = json!({
            "deleted_paths": ["Arch/Cache.md"]
        })
        .to_string();
        let (code, deleted_res) = http_call_with_auth(
            addr,
            "POST",
            "/v1/vault/push",
            Some(&contrib_tok),
            Some(&delete_payload),
        )
        .await;
        assert_eq!(code, 200);
        assert_eq!(deleted_res["sync_report"]["files_removed"], 1);

        let (code, pulled_after) =
            http_call_with_auth(addr, "GET", "/v1/vault/pull", Some(&reader_tok), None).await;
        assert_eq!(code, 200);
        assert!(pulled_after["manifest"].get("Arch/Cache.md").is_none());

        server.shutdown().await.unwrap();
        engine.shutdown().await.unwrap();
    }

    fn header_value<'a>(head: &'a str, name: &str) -> Option<&'a str> {
        head.lines().skip(1).find_map(|line| {
            let (k, v) = line.split_once(':')?;
            if k.trim().eq_ignore_ascii_case(name) {
                Some(v.trim())
            } else {
                None
            }
        })
    }

    #[tokio::test]
    async fn control_plane_serves_public_openapi_3_1_schema_while_protecting_data_routes() {
        let vault = TempVault::new();
        vault.write("Specs/Note.md", "# Note\nOpenAPI test fixture.\n");
        let engine = Arc::new(
            VaultEngine::open(VaultConfig::in_memory(&vault.root))
                .await
                .unwrap(),
        );
        let admin_secret = crate::auth::ApiKeySecret::generate();
        engine
            .ensure_bootstrap_admin_key(
                crate::auth::AgentId::new("bootstrap-admin").unwrap(),
                &admin_secret,
            )
            .await
            .unwrap();
        let admin_tok = admin_secret.expose_secret();

        let server = ControlPlaneServer::bind_with_auth(
            "127.0.0.1:0".parse().unwrap(),
            Arc::clone(&engine),
            true,
        )
        .await
        .unwrap();
        let addr = server.local_addr();

        let (code, head, spec) =
            http_call_raw(addr, "GET", "/v1/openapi.json", None, &[], None).await;
        assert_eq!(code, 200);
        assert!(head.starts_with("HTTP/1.1 200 OK\r\n"));
        assert_eq!(
            header_value(&head, "content-type"),
            Some("application/json")
        );
        assert_eq!(spec["openapi"], "3.1.0");
        assert!(spec["info"]["title"]
            .as_str()
            .is_some_and(|s| !s.is_empty()));
        assert!(spec["info"]["version"]
            .as_str()
            .is_some_and(|s| !s.is_empty()));
        assert_eq!(spec["servers"][0]["url"], "http://127.0.0.1");

        let bearer_scheme = &spec["components"]["securitySchemes"]["bearerAuth"];
        assert_eq!(bearer_scheme["type"], "http");
        assert_eq!(bearer_scheme["scheme"], "bearer");
        assert_eq!(spec["security"][0]["bearerAuth"], json!([]));

        for (path, method) in [
            ("/v1/recall", "post"),
            ("/v1/stage", "post"),
            ("/v1/inspect", "post"),
            ("/v1/status", "get"),
        ] {
            let op = &spec["paths"][path][method];
            assert!(
                op.is_object(),
                "expected OpenAPI path {method} {path} to be present"
            );
            assert!(
                op["operationId"].as_str().is_some_and(|s| !s.is_empty()),
                "expected operationId on {method} {path}"
            );
            assert!(
                op["responses"]["200"].is_object(),
                "expected 200 response on {method} {path}"
            );
        }

        let recall_schema_ref = spec["paths"]["/v1/recall"]["post"]["requestBody"]["content"]
            ["application/json"]["schema"]["$ref"]
            .as_str();
        assert_eq!(
            recall_schema_ref,
            Some("#/components/schemas/RecallHttpRequest")
        );
        assert_eq!(
            spec["components"]["schemas"]["RecallHttpRequest"]["required"],
            json!(["query"])
        );
        assert_eq!(
            spec["components"]["schemas"]["StageHttpRequest"]["required"],
            json!(["proposed_fact"])
        );
        assert_eq!(
            spec["components"]["schemas"]["InspectHttpRequest"]["required"],
            json!(["source_pointer"])
        );

        let (code, _, authed_spec) =
            http_call_raw(addr, "GET", "/v1/openapi.json", Some(admin_tok), &[], None).await;
        assert_eq!(code, 200);
        assert_eq!(authed_spec["openapi"], "3.1.0");

        let (code, _, proxied_spec) = http_call_raw(
            addr,
            "GET",
            "/v1/openapi.json",
            None,
            &[
                ("X-Forwarded-Proto", "https"),
                ("X-Forwarded-Host", "vault.example.com"),
            ],
            None,
        )
        .await;
        assert_eq!(code, 200);
        assert_eq!(
            proxied_spec["servers"][0]["url"],
            "https://vault.example.com"
        );

        let (code, _, bad_host_spec) = http_call_raw(
            addr,
            "GET",
            "/v1/openapi.json",
            None,
            &[("X-Forwarded-Host", "evil.example.com/inject")],
            None,
        )
        .await;
        assert_eq!(code, 200);
        assert_eq!(bad_host_spec["servers"][0]["url"], "http://127.0.0.1:4771");

        assert_openapi_refs_resolve(&spec);
        assert_required_fields_match_live_response(
            &spec,
            "VaultStatusReport",
            &http_call_with_auth(addr, "GET", "/v1/status", Some(admin_tok), None)
                .await
                .1,
        );
        assert_required_fields_match_live_response(
            &spec,
            "RecallResponse",
            &http_call_with_auth(
                addr,
                "POST",
                "/v1/recall",
                Some(admin_tok),
                Some(r#"{"query":"OpenAPI"}"#),
            )
            .await
            .1,
        );
        assert_required_fields_match_live_response(
            &spec,
            "StageMemoryResponse",
            &http_call_with_auth(
                addr,
                "POST",
                "/v1/stage",
                Some(admin_tok),
                Some(r#"{"proposed_fact":"OpenAPI schema test fact."}"#),
            )
            .await
            .1,
        );
        assert_required_fields_match_live_response(
            &spec,
            "InspectSourceResponse",
            &http_call_with_auth(
                addr,
                "POST",
                "/v1/inspect",
                Some(admin_tok),
                Some(r#"{"source_pointer":"Specs/Note.md#Note"}"#),
            )
            .await
            .1,
        );

        let (code, _) = http_call(addr, "POST", "/v1/openapi.json", Some("{}")).await;
        assert_eq!(code, 401);
        let (code, _) = http_call_with_auth(
            addr,
            "POST",
            "/v1/openapi.json",
            Some(admin_tok),
            Some("{}"),
        )
        .await;
        assert_eq!(code, 404);

        for (method, path, body) in [
            ("GET", "/v1/status", None),
            ("POST", "/v1/recall", Some(r#"{"query":"note"}"#)),
            ("POST", "/v1/stage", Some(r#"{"proposed_fact":"test"}"#)),
            (
                "POST",
                "/v1/inspect",
                Some(r#"{"source_pointer":"Specs/Note.md"}"#),
            ),
        ] {
            let (unauth_code, _) = http_call(addr, method, path, body).await;
            assert_eq!(
                unauth_code, 401,
                "{method} {path} must still require bearer auth"
            );
        }

        server.shutdown().await.unwrap();

        let loopback_server =
            ControlPlaneServer::bind("127.0.0.1:0".parse().unwrap(), Arc::clone(&engine))
                .await
                .unwrap();
        let (code, _, loopback_spec) = http_call_raw(
            loopback_server.local_addr(),
            "GET",
            "/v1/openapi.json",
            None,
            &[],
            None,
        )
        .await;
        assert_eq!(code, 200);
        assert_eq!(loopback_spec["openapi"], "3.1.0");

        loopback_server.shutdown().await.unwrap();
        engine.shutdown().await.unwrap();
    }

    fn assert_openapi_refs_resolve(spec: &Value) {
        fn walk(root: &Value, node: &Value) {
            match node {
                Value::Object(map) => {
                    assert!(
                        !map.contains_key("nullable"),
                        "OpenAPI 3.1.0 must use type arrays instead of `nullable`"
                    );
                    if let Some(reference) = map.get("$ref").and_then(Value::as_str) {
                        let schema_name = reference
                            .strip_prefix("#/components/schemas/")
                            .unwrap_or_else(|| panic!("unexpected $ref target `{reference}`"));
                        assert!(
                            root["components"]["schemas"][schema_name].is_object(),
                            "missing schema for $ref `{reference}`"
                        );
                    }
                    for child in map.values() {
                        walk(root, child);
                    }
                }
                Value::Array(items) => {
                    for child in items {
                        walk(root, child);
                    }
                }
                _ => {}
            }
        }
        walk(spec, spec);
    }

    fn assert_required_fields_match_live_response(
        spec: &Value,
        schema_name: &str,
        payload: &Value,
    ) {
        let schema = &spec["components"]["schemas"][schema_name];
        let required = schema["required"].as_array().unwrap();
        let props = schema["properties"].as_object().unwrap();
        let obj = payload.as_object().unwrap();
        for field in required {
            let name = field.as_str().unwrap();
            assert!(
                obj.contains_key(name),
                "live payload missing required `{schema_name}` field `{name}`"
            );
        }
        for key in obj.keys() {
            assert!(
                props.contains_key(key),
                "live payload returned undocumented `{schema_name}` field `{key}`"
            );
        }
    }
}
