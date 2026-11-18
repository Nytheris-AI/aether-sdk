use std::sync::Arc;

use nytheris_aether_core::{AetherError, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};

use crate::auth::{ApiKeySecret, ScopedApiKey};
use crate::engine::VaultEngine;
use crate::kv::ModelId;
use crate::types::{AmuId, RecallRequest, ScopeTag, StageMemoryRequest, TokenBudget, VaultTier};

const PROTOCOL_VERSION: &str = "2024-11-05";

/// 3-tool Model Context Protocol (MCP) JSON-RPC 2.0 server backed by [`VaultEngine`].
pub struct McpServer {
    engine: Arc<VaultEngine>,
    agent: Option<ScopedApiKey>,
}

#[derive(Debug, Deserialize)]
struct JsonRpcRequest {
    jsonrpc: String,
    #[serde(default)]
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Debug, Serialize)]
struct JsonRpcResponse {
    jsonrpc: &'static str,
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
struct JsonRpcError {
    code: i32,
    message: String,
}

impl McpServer {
    pub fn new(engine: Arc<VaultEngine>) -> Self {
        Self {
            engine,
            agent: None,
        }
    }

    /// Binds an MCP server session directly to a pre-authenticated `ScopedApiKey`.
    pub fn scoped(engine: Arc<VaultEngine>, agent: ScopedApiKey) -> Self {
        Self {
            engine,
            agent: Some(agent),
        }
    }

    /// Authenticates `secret` against the vault engine and returns a scoped MCP session.
    pub async fn with_api_key(engine: Arc<VaultEngine>, secret: &ApiKeySecret) -> Result<Self> {
        let agent = engine
            .authenticate_api_key(secret)
            .await
            .map_err(AetherError::from)?;
        Ok(Self::scoped(engine, agent))
    }

    /// Handles a single JSON-RPC 2.0 request or notification.
    pub async fn handle_jsonrpc(&self, raw: Value) -> Option<Value> {
        let req: JsonRpcRequest = match serde_json::from_value(raw) {
            Ok(parsed) => parsed,
            Err(err) => {
                return Some(error_json(
                    Value::Null,
                    -32600,
                    format!("invalid request: {err}"),
                ));
            }
        };

        if req.jsonrpc != "2.0" {
            let id = req.id?;
            return Some(error_json(id, -32600, "jsonrpc must equal \"2.0\""));
        }

        let id = req.id?;

        let resp = match req.method.as_str() {
            "initialize" => ok_json(
                id,
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": { "tools": {} },
                    "serverInfo": {
                        "name": "aether-vault-mcp",
                        "version": env!("CARGO_PKG_VERSION"),
                    }
                }),
            ),
            "ping" => ok_json(id, json!({})),
            "tools/list" => ok_json(id, json!({ "tools": tool_descriptors() })),
            "tools/call" => self.handle_tool_call(id, req.params).await,
            other => error_json(id, -32601, format!("method not found: {other}")),
        };
        Some(resp)
    }

    async fn handle_tool_call(&self, id: Value, params: Value) -> Value {
        let Some(name) = params.get("name").and_then(Value::as_str) else {
            return error_json(id, -32602, "missing string field `params.name`");
        };
        let args = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));

        let outcome: Result<(String, Option<Value>)> = match name {
            "aether_recall" => self.call_recall(&args).await,
            "aether_stage_memory" => self.call_stage_memory(&args).await.map(|s| (s, None)),
            "aether_inspect_source" => self.call_inspect_source(&args).await.map(|s| (s, None)),
            unknown => {
                return error_json(id, -32602, format!("unknown tool `{unknown}`"));
            }
        };

        match outcome {
            Ok((text, structured)) => {
                let mut payload = json!({
                    "content": [{ "type": "text", "text": text }],
                    "isError": false
                });
                if let Some(sc) = structured {
                    payload["structuredContent"] = sc;
                }
                ok_json(id, payload)
            }
            Err(err) => ok_json(
                id,
                json!({
                    "content": [{ "type": "text", "text": err.to_string() }],
                    "isError": true
                }),
            ),
        }
    }

    async fn call_recall(&self, args: &Value) -> Result<(String, Option<Value>)> {
        let query = args
            .get("query")
            .and_then(Value::as_str)
            .ok_or_else(|| AetherError::InvalidInput("missing string argument `query`".into()))?;

        let mut req = RecallRequest::new(query);
        if let Some(scope_raw) = args.get("scope").and_then(Value::as_str) {
            if !scope_raw.trim().is_empty() {
                req.scope = Some(ScopeTag::new(scope_raw)?);
            }
        }
        if let Some(max_tok) = args.get("max_tokens").and_then(Value::as_u64) {
            let usize_val = usize::try_from(max_tok)
                .map_err(|_| AetherError::InvalidInput("max_tokens overflow".into()))?;
            req.max_tokens = TokenBudget::new(usize_val)?;
        }
        if let Some(include_hot) = args.get("include_hot_profile").and_then(Value::as_bool) {
            req.include_hot_profile = include_hot;
        }
        if let Some(model_raw) = args.get("kv_model").and_then(Value::as_str) {
            if !model_raw.trim().is_empty() {
                req.kv_model = Some(ModelId::new(model_raw)?);
            }
        }
        let structured_flag = args
            .get("structured")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        let res = match &self.agent {
            Some(agent) => self.engine.recall_as(agent, req).await?,
            None => self.engine.recall(req).await?,
        };
        let mut structured_val = json!({
            "static_prefix": res.static_prefix,
            "dynamic_context": res.dynamic_context,
            "prefix_hash": res.prefix_hash.as_str(),
            "static_prefix_tokens": res.static_prefix_tokens,
            "dynamic_tokens": res.dynamic_tokens,
            "tokens_used": res.tokens_used,
        });
        if let Some(hint) = &res.kv_hint {
            structured_val["kv_hint"] = serde_json::to_value(hint)?;
        }
        let text = if structured_flag {
            serde_json::to_string(&structured_val)?
        } else if res.formatted_context.is_empty() {
            "(no matching vault memory)".to_string()
        } else {
            res.formatted_context
        };
        Ok((text, Some(structured_val)))
    }

    async fn call_stage_memory(&self, args: &Value) -> Result<String> {
        let fact = args
            .get("proposed_fact")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                AetherError::InvalidInput("missing string argument `proposed_fact`".into())
            })?;

        let tier = match args.get("suggested_tier").and_then(Value::as_str) {
            Some(raw) => VaultTier::parse_str(raw).ok_or_else(|| {
                AetherError::InvalidInput(format!(
                    "suggested_tier must be one of hot, warm, cold (got `{raw}`)"
                ))
            })?,
            None => VaultTier::Warm,
        };

        let mut req = StageMemoryRequest::new(fact, tier);
        if let Some(scope_raw) = args.get("scope").and_then(Value::as_str) {
            if !scope_raw.trim().is_empty() {
                req.scope = ScopeTag::new(scope_raw)?;
            } else if let Some(agent) = &self.agent {
                req.scope = agent.default_write_scope();
            }
        } else if let Some(agent) = &self.agent {
            req.scope = agent.default_write_scope();
        }
        if let Some(target) = args.get("target_note").and_then(Value::as_str) {
            if !target.trim().is_empty() {
                req = req.with_target_note(target);
            }
        }
        if let Some(sup_raw) = args.get("supersedes_amu_id").and_then(Value::as_str) {
            if !sup_raw.trim().is_empty() {
                req = req.with_supersedes(AmuId::parse(sup_raw)?);
            }
        }

        let candidate_id = match &self.agent {
            Some(agent) => self.engine.stage_memory_as(agent, req).await?,
            None => self.engine.stage_memory(req).await?,
        };
        Ok(json!({
            "candidate_id": candidate_id.as_str(),
            "inbox_path": "Aether/Inbox.md"
        })
        .to_string())
    }

    async fn call_inspect_source(&self, args: &Value) -> Result<String> {
        let ptr = args
            .get("source_pointer")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                AetherError::InvalidInput("missing string argument `source_pointer`".into())
            })?;
        match &self.agent {
            Some(agent) => self.engine.inspect_source_as(agent, ptr).await,
            None => self.engine.inspect_source(ptr).await,
        }
    }

    /// Serves newline-delimited JSON-RPC 2.0 messages over `stdin` and `stdout`.
    pub async fn serve_stdio(&self) -> Result<()> {
        let stdin = tokio::io::BufReader::new(tokio::io::stdin());
        let stdout = tokio::io::stdout();
        self.serve_streams(stdin, stdout).await
    }

    /// Serves newline-delimited JSON-RPC 2.0 messages over arbitrary async streams.
    pub async fn serve_streams<R, W>(&self, mut reader: R, mut writer: W) -> Result<()>
    where
        R: AsyncBufRead + Unpin,
        W: AsyncWrite + Unpin,
    {
        let mut line = String::new();
        loop {
            line.clear();
            let read = reader
                .read_line(&mut line)
                .await
                .map_err(|err| AetherError::Permanent {
                    connector: "mcp-stdio",
                    message: format!("read error: {err}"),
                })?;
            if read == 0 {
                break;
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            let response = match serde_json::from_str::<Value>(trimmed) {
                Ok(val) => self.handle_jsonrpc(val).await,
                Err(err) => Some(error_json(
                    Value::Null,
                    -32700,
                    format!("parse error: {err}"),
                )),
            };

            if let Some(resp_val) = response {
                let serialized = serde_json::to_string(&resp_val)?;
                writer
                    .write_all(serialized.as_bytes())
                    .await
                    .map_err(io_err)?;
                writer.write_all(b"\n").await.map_err(io_err)?;
                writer.flush().await.map_err(io_err)?;
            }
        }
        Ok(())
    }
}

fn tool_descriptors() -> Value {
    json!([
        {
            "name": "aether_recall",
            "description": "Retrieve distilled, source-grounded memory from the user's Obsidian vault.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Natural language or keyword query for Warm/Cold recall." },
                    "scope": { "type": "string", "description": "Active agent scope (e.g., 'engineering', 'global')." },
                    "max_tokens": { "type": "integer", "default": 1500, "description": "Strict token ceiling for returned context." },
                    "include_hot_profile": { "type": "boolean", "default": true },
                    "structured": { "type": "boolean", "default": false, "description": "When true, returns JSON separating static_prefix (with prefix_hash) and dynamic_context for provider prompt caching." },
                    "kv_model": { "type": "string", "description": "Optional target model identifier for PagedAttention/CacheBlend KV-cache reuse hints." }
                },
                "required": ["query"]
            }
        },
        {
            "name": "aether_stage_memory",
            "description": "Propose a new durable fact, decision, or preference for human approval in Obsidian.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "proposed_fact": { "type": "string", "description": "Concise, atomic statement to remember." },
                    "suggested_tier": { "type": "string", "enum": ["hot", "warm", "cold"], "default": "warm" },
                    "scope": { "type": "string", "description": "Target memory scope (defaults to the agent primary scope or global)." },
                    "target_note": { "type": "string", "description": "Suggested Obsidian note or [[wikilink]] topic." },
                    "supersedes_amu_id": { "type": "string", "description": "Optional ID of an older fact this replaces." }
                },
                "required": ["proposed_fact"]
            }
        },
        {
            "name": "aether_inspect_source",
            "description": "Fetch the exact verbatim Markdown section from Obsidian using an AMU provenance pointer.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "source_pointer": { "type": "string", "description": "Vault path and heading anchor, e.g., 'Arch/DB.md#Schema'." }
                },
                "required": ["source_pointer"]
            }
        }
    ])
}

fn ok_json(id: Value, result: Value) -> Value {
    serde_json::to_value(JsonRpcResponse {
        jsonrpc: "2.0",
        id,
        result: Some(result),
        error: None,
    })
    .expect("JsonRpcResponse serialization is infallible")
}

fn error_json(id: Value, code: i32, message: impl Into<String>) -> Value {
    serde_json::to_value(JsonRpcResponse {
        jsonrpc: "2.0",
        id,
        result: None,
        error: Some(JsonRpcError {
            code,
            message: message.into(),
        }),
    })
    .expect("JsonRpcResponse serialization is infallible")
}

fn io_err(err: std::io::Error) -> AetherError {
    AetherError::Permanent {
        connector: "mcp-stdio",
        message: format!("write error: {err}"),
    }
}
