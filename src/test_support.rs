//! Fixtures for tests that check what Servitor publishes to the feed.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::body::Bytes;
use axum::extract::State;
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{Map, Value};

use crate::egregore::EgregoreClient;
use crate::mcp::{McpClient, McpNotification, ToolCallResult, ToolDefinition};
use thallus_core::mcp::client::{InitializeResult, ServerCapabilities, ServerInfo};

/// A stand-in egregore node that records the raw body of every publish
/// request, so tests compare exactly the bytes that would be signed and
/// replicated.
pub(crate) struct CaptureNode {
    pub(crate) client: EgregoreClient,
    bodies: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl CaptureNode {
    pub(crate) async fn start() -> Self {
        let bodies = Arc::new(Mutex::new(Vec::new()));
        let router = Router::new()
            .route("/v1/publish", post(record_publish))
            .with_state(bodies.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router).await });
        Self {
            client: EgregoreClient::new(&format!("http://{addr}")),
            bodies,
        }
    }

    /// Every publish request body so far, in order.
    pub(crate) fn bodies(&self) -> Vec<Vec<u8>> {
        self.bodies.lock().unwrap().clone()
    }

    /// Publish request bodies whose content has the given `type`.
    pub(crate) fn bodies_of_type(&self, content_type: &str) -> Vec<Vec<u8>> {
        self.bodies()
            .into_iter()
            .filter(|body| {
                serde_json::from_slice::<Value>(body).unwrap()["content"]["type"] == content_type
            })
            .collect()
    }

    pub(crate) fn clear(&self) {
        self.bodies.lock().unwrap().clear();
    }
}

async fn record_publish(
    State(bodies): State<Arc<Mutex<Vec<Vec<u8>>>>>,
    body: Bytes,
) -> Json<Value> {
    bodies.lock().unwrap().push(body.to_vec());
    Json(serde_json::json!({
        "success": true,
        "data": { "hash": "0".repeat(64), "sequence": 1 }
    }))
}

/// Fail with the first differing offset when two published bodies differ,
/// instead of printing two possibly huge payloads.
pub(crate) fn assert_same_bytes(expected: &[u8], actual: &[u8], label: &str) {
    if expected == actual {
        return;
    }
    let at = expected
        .iter()
        .zip(actual)
        .position(|(left, right)| left != right)
        .unwrap_or(expected.len().min(actual.len()));
    let window = |bytes: &[u8]| {
        let start = at.saturating_sub(60);
        String::from_utf8_lossy(&bytes[start..bytes.len().min(at + 140)]).into_owned()
    };
    panic!(
        "{label}: published bytes differ at offset {at} (lengths {} vs {})\n  expected: …{}…\n  actual:   …{}…",
        expected.len(),
        actual.len(),
        window(expected),
        window(actual),
    );
}

/// An MCP client with a single scripted reply.
pub(crate) struct ScriptedClient {
    reply: ToolCallResult,
}

impl ScriptedClient {
    pub(crate) fn output(text: impl Into<String>) -> Box<dyn McpClient> {
        Box::new(Self {
            reply: ToolCallResult::text(text),
        })
    }
}

#[async_trait]
impl McpClient for ScriptedClient {
    async fn initialize(&mut self) -> thallus_core::Result<InitializeResult> {
        Ok(InitializeResult {
            protocol_version: "2024-11-05".to_string(),
            server_info: ServerInfo {
                name: "scripted".to_string(),
                version: "1.0.0".to_string(),
            },
            capabilities: ServerCapabilities::default(),
        })
    }

    async fn list_tools(&self) -> thallus_core::Result<Vec<ToolDefinition>> {
        Ok(vec![])
    }

    async fn call_tool(
        &self,
        _name: &str,
        _arguments: Value,
    ) -> thallus_core::Result<ToolCallResult> {
        Ok(self.reply.clone())
    }

    async fn ping(&self) -> thallus_core::Result<()> {
        Ok(())
    }

    async fn drain_notifications(&self) -> thallus_core::Result<Vec<McpNotification>> {
        Ok(vec![])
    }

    async fn shutdown(&mut self) -> thallus_core::Result<()> {
        Ok(())
    }

    fn name(&self) -> &str {
        "scripted"
    }
}

/// Seeded generator of secret-bearing payloads: arbitrary JSON with random
/// keys, types, nesting, Unicode and sizes, each carrying `CANARY`.
///
/// A seeded generator rather than proptest: the crate has no property-testing
/// dependency, and a failure here reports its seed for exact replay.
pub(crate) struct SecretGen {
    state: u64,
}

impl SecretGen {
    /// Marker embedded in every generated secret.
    pub(crate) const CANARY: &'static str = "canary-7f3a-hunter2";

    pub(crate) fn new(seed: u64) -> Self {
        Self {
            state: seed ^ 0x9e37_79b9_7f4a_7c15,
        }
    }

    fn next(&mut self) -> u64 {
        // SplitMix64.
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn below(&mut self, bound: u64) -> u64 {
        self.next() % bound
    }

    /// A string carrying the canary among quotes, escapes, control
    /// characters and non-ASCII text, occasionally tens of kilobytes long.
    pub(crate) fn secret(&mut self) -> String {
        const PIECES: &[&str] = &[
            " ",
            "-p",
            "--password=",
            "\"",
            "'",
            "\\",
            "\n",
            "\t",
            "\u{0}",
            "=",
            "/",
            "é",
            "日本",
            "🔑",
            "\u{202e}",
            "$(",
            "`",
            "%2F",
            "AKIA",
            "Bearer ",
        ];
        let mut out = String::new();
        let parts = 1 + self.below(12);
        let canary_at = self.below(parts);
        for index in 0..parts {
            if index == canary_at {
                out.push_str(Self::CANARY);
            }
            match self.below(4) {
                0 => out.push_str(PIECES[self.below(PIECES.len() as u64) as usize]),
                1 => out.push_str(&format!("{:x}", self.next())),
                2 => out.push(char::from_u32(0x20 + self.below(0x2fff) as u32).unwrap_or('x')),
                _ => out.push_str("word"),
            }
        }
        if self.below(16) == 0 {
            let filler = "x".repeat(1 + self.below(64 * 1024) as usize);
            out.push_str(&filler);
        }
        out
    }

    /// Arbitrary JSON, nested up to `depth` levels.
    pub(crate) fn value(&mut self, depth: u32) -> Value {
        let kinds = if depth == 0 { 5 } else { 7 };
        match self.below(kinds) {
            0 => Value::Null,
            1 => Value::Bool(self.below(2) == 0),
            2 => Value::from(self.next() as i64),
            3 => serde_json::Number::from_f64(self.next() as f64 / 7.0)
                .map(Value::Number)
                .unwrap_or(Value::Null),
            4 => Value::String(self.secret()),
            5 => {
                let len = self.below(5);
                Value::Array((0..len).map(|_| self.value(depth - 1)).collect())
            }
            _ => self.object(depth - 1),
        }
    }

    fn object(&mut self, depth: u32) -> Value {
        let mut map = Map::new();
        for _ in 0..self.below(5) {
            let key = if self.below(2) == 0 {
                self.secret()
            } else {
                format!("k{}", self.below(100))
            };
            map.insert(key, self.value(depth));
        }
        Value::Object(map)
    }

    /// Tool-call arguments: an object with arbitrary keys and values, plus a
    /// canary-bearing `command` (and sometimes `path` or `name`), the fields
    /// scope enforcement reads.
    pub(crate) fn arguments(&mut self) -> Value {
        let mut arguments = match self.object(3) {
            Value::Object(map) => map,
            _ => unreachable!(),
        };
        arguments.insert("command".to_string(), Value::String(self.secret()));
        for key in ["path", "name"] {
            if self.below(2) == 0 {
                arguments.insert(key.to_string(), Value::String(self.secret()));
            }
        }
        Value::Object(arguments)
    }
}
