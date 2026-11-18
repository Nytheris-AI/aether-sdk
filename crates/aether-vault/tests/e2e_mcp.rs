use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use nytheris_aether_vault::{McpServer, VaultConfig, VaultEngine};
use serde_json::{json, Value};
use tokio::io::BufReader;
use uuid::Uuid;

struct TempVault {
    root: PathBuf,
}

impl TempVault {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("aether_mcp_e2e_{}", Uuid::new_v4()));
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

#[tokio::test]
async fn mcp_stdio_roundtrip_serves_all_three_tools_under_150ms() {
    let vault = TempVault::new();
    vault.write(
        "Profile/Preferences.md",
        "---\naether_tier: hot\naether_scope:\n  - global\naether_kind: profile\n---\n# Preferences\nPrefer strict TDD and micro commits.\n",
    );
    vault.write(
        "Arch/DB.md",
        "---\naether_tier: warm\naether_scope:\n  - engineering\n---\n# Schema\nUse SQLite WAL mode with `--port 8080`.\n",
    );
    vault.write(
        "Private/Journal.md",
        "---\naether_tier: private\n---\n# Diary\nConfidential secret phrase.\n",
    );

    let engine = Arc::new(
        VaultEngine::open(VaultConfig::in_memory(&vault.root))
            .await
            .unwrap(),
    );
    let server = McpServer::new(Arc::clone(&engine));

    let requests = [
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "test-client", "version": "0.1.0" }
            }
        }),
        json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        }),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list"
        }),
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "aether_recall",
                "arguments": {
                    "query": "SQLite WAL mode port",
                    "scope": "engineering",
                    "max_tokens": 1500,
                    "include_hot_profile": true
                }
            }
        }),
        json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "tools/call",
            "params": {
                "name": "aether_stage_memory",
                "arguments": {
                    "proposed_fact": "Enable SQLite FTS5 porter tokenizer for note search.",
                    "suggested_tier": "warm",
                    "target_note": "Arch/DB"
                }
            }
        }),
        json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "tools/call",
            "params": {
                "name": "aether_inspect_source",
                "arguments": {
                    "source_pointer": "Arch/DB.md#Schema"
                }
            }
        }),
        json!({
            "jsonrpc": "2.0",
            "id": 6,
            "method": "tools/call",
            "params": {
                "name": "aether_inspect_source",
                "arguments": {
                    "source_pointer": "Private/Journal.md#Diary"
                }
            }
        }),
    ];

    let mut input_buf = String::new();
    for req in &requests {
        input_buf.push_str(&serde_json::to_string(req).unwrap());
        input_buf.push('\n');
    }

    let mut output_buf: Vec<u8> = Vec::new();
    let started = Instant::now();
    server
        .serve_streams(BufReader::new(input_buf.as_bytes()), &mut output_buf)
        .await
        .unwrap();
    let elapsed = started.elapsed();
    assert!(
        elapsed.as_millis() < 150,
        "expected full MCP call sequence under 150ms, got {}ms",
        elapsed.as_millis()
    );

    let out_str = String::from_utf8(output_buf).unwrap();
    let responses: Vec<Value> = out_str
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();

    // Notification produces no response, so 7 frames -> 6 responses.
    assert_eq!(responses.len(), 6);

    assert_eq!(responses[0]["id"], 1);
    assert_eq!(responses[0]["result"]["protocolVersion"], "2024-11-05");

    let tools = responses[1]["result"]["tools"].as_array().unwrap();
    let tool_names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(
        tool_names,
        vec![
            "aether_recall",
            "aether_stage_memory",
            "aether_inspect_source"
        ]
    );

    let recall_text = responses[2]["result"]["content"][0]["text"]
        .as_str()
        .unwrap();
    assert_ne!(responses[2]["result"]["isError"], true);
    assert!(recall_text.contains("Prefer strict TDD"));
    assert!(recall_text.contains("SQLite WAL mode"));
    assert!(!recall_text.contains("Confidential secret phrase"));

    let stage_text = responses[3]["result"]["content"][0]["text"]
        .as_str()
        .unwrap();
    assert!(stage_text.contains("cand_"));
    let inbox_contents = fs::read_to_string(vault.root.join("Aether/Inbox.md")).unwrap();
    assert!(inbox_contents.contains("Enable SQLite FTS5 porter tokenizer"));

    let inspect_text = responses[4]["result"]["content"][0]["text"]
        .as_str()
        .unwrap();
    assert!(inspect_text.contains("--port 8080"));

    assert_eq!(responses[5]["result"]["isError"], true);

    engine.shutdown().await.unwrap();
}

#[tokio::test]
async fn mcp_rejects_malformed_json_unknown_methods_and_private_stage_tier() {
    let vault = TempVault::new();
    vault.write("Note.md", "# Note\nSample content.\n");
    let engine = Arc::new(
        VaultEngine::open(VaultConfig::in_memory(&vault.root))
            .await
            .unwrap(),
    );
    let server = McpServer::new(Arc::clone(&engine));

    let payload = format!(
        "{{not json\n{}\n{}\n{}\n",
        json!({ "jsonrpc": "2.0", "id": 10, "method": "ping" }),
        json!({ "jsonrpc": "2.0", "id": 11, "method": "unknown/method" }),
        json!({
            "jsonrpc": "2.0",
            "id": 12,
            "method": "tools/call",
            "params": {
                "name": "aether_stage_memory",
                "arguments": {
                    "proposed_fact": "Secret",
                    "suggested_tier": "private"
                }
            }
        }),
    );

    let mut out: Vec<u8> = Vec::new();
    server
        .serve_streams(BufReader::new(payload.as_bytes()), &mut out)
        .await
        .unwrap();

    let resps: Vec<Value> = String::from_utf8(out)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(resps.len(), 4);
    assert_eq!(resps[0]["error"]["code"], -32700);
    assert_eq!(resps[1]["id"], 10);
    assert_eq!(resps[1]["result"], json!({}));
    assert_eq!(resps[2]["error"]["code"], -32601);
    assert_eq!(resps[3]["result"]["isError"], true);

    engine.shutdown().await.unwrap();
}
