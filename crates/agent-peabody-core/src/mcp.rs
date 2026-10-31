// SPDX-FileCopyrightText: 2026 Gary Frattarola <garyf@parkviewlab.ai>
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A Model Context Protocol client over Streamable HTTP, for the tools Peabody offers the
//! model (aqua-roaming's `search` and `fetch`, say). Peabody runs the tools itself: it
//! lists them, describes them to the model in the chat request, and calls the ones the
//! model asks for. A reply may come as plain JSON or as Server-Sent Events.

use crate::http;
use crate::settings::Endpoint;
use reqwest::Method;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

const PROTOCOL: &str = "2025-06-18";
/// How long a tool may take: a fetch of a slow page, or a search the server paces.
const CALL_TIMEOUT: Duration = Duration::from_secs(90);
/// The most of a tool's result given to the model, in characters, so that a fetched page
/// does not fill a small context window; about 2000 tokens.
pub const RESULT_LIMIT: usize = 8000;

/// A session with one MCP server.
pub struct Session {
    /// The server's name for itself (`serverInfo.name`), or its address.
    pub name: String,
    c: reqwest::Client,
    ep: Endpoint,
    id: Option<String>,
    next: AtomicU64,
}

impl Session {
    /// Opens a session: `initialize`, then the `initialized` notification.
    pub async fn open(c: reqwest::Client, ep: Endpoint) -> Result<Self, String> {
        let mut s = Self {
            name: ep.url.clone(),
            c,
            ep,
            id: None,
            next: AtomicU64::new(1),
        };
        let (r, id) = s
            .rpc(
                "initialize",
                json!({
                    "protocolVersion": PROTOCOL,
                    "capabilities": {},
                    "clientInfo": {"name": "peabody", "version": env!("CARGO_PKG_VERSION")},
                }),
            )
            .await?;
        s.id = id;
        if let Some(n) = r["serverInfo"]["name"].as_str().filter(|n| !n.is_empty()) {
            s.name = n.to_string();
        }
        s.notify("notifications/initialized").await;
        Ok(s)
    }

    /// The server's tools, described as the chat API's `tools` expects them.
    pub async fn tools(&self) -> Result<Vec<Value>, String> {
        let (r, _) = self.rpc("tools/list", json!({})).await?;
        Ok(r["tools"]
            .as_array()
            .map(|ts| ts.iter().map(as_chat_tool).collect())
            .unwrap_or_default())
    }

    /// Calls a tool: its text, cut to [`RESULT_LIMIT`], or the reason it failed, which the
    /// model is told either way; and whether the server could not be asked at all.
    pub async fn call(&self, name: &str, arguments: Value) -> (String, bool) {
        match self
            .rpc("tools/call", json!({"name": name, "arguments": arguments}))
            .await
        {
            Ok((r, _)) => {
                let text = result_text(&r);
                if r["isError"].as_bool() == Some(true) {
                    (format!("The tool failed: {}", cut(&text)), false)
                } else {
                    (cut(&text), false)
                }
            }
            Err(e) => (format!("The tool could not be called: {e}"), true),
        }
    }

    async fn notify(&self, method: &str) {
        let _ = self
            .post(json!({"jsonrpc": "2.0", "method": method}))
            .send()
            .await;
    }

    fn post(&self, body: Value) -> reqwest::RequestBuilder {
        let mut rb = http::request(&self.c, Method::POST, &self.ep, "")
            .header("accept", "application/json, text/event-stream")
            .header("mcp-protocol-version", PROTOCOL)
            .timeout(CALL_TIMEOUT)
            .json(&body);
        if let Some(id) = &self.id {
            rb = rb.header("mcp-session-id", id);
        }
        rb
    }

    /// One request and its result, with the session id the server gave, if any.
    async fn rpc(&self, method: &str, params: Value) -> Result<(Value, Option<String>), String> {
        let n = self.next.fetch_add(1, Ordering::Relaxed);
        let r = self
            .post(json!({"jsonrpc": "2.0", "id": n, "method": method, "params": params}))
            .send()
            .await
            .map_err(|e| http::describe(&e))?;
        let status = r.status();
        let session = r
            .headers()
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let text = r.text().await.map_err(|e| http::describe(&e))?;
        if !status.is_success() {
            return Err(format!("answered {status}"));
        }
        let v = response(&text, n).ok_or("an unreadable reply")?;
        if let Some(e) = v.get("error") {
            return Err(e["message"].as_str().unwrap_or("an error").to_string());
        }
        Ok((v["result"].clone(), session))
    }
}

/// The JSON-RPC response with id `n`, from a JSON body or from the `data:` lines of an
/// event stream.
fn response(body: &str, n: u64) -> Option<Value> {
    let is_ours = |v: &Value| v["id"].as_u64() == Some(n);
    if let Ok(v) = serde_json::from_str::<Value>(body.trim()) {
        return is_ours(&v).then_some(v);
    }
    body.lines()
        .filter_map(|l| l.strip_prefix("data:"))
        .filter_map(|d| serde_json::from_str::<Value>(d.trim()).ok())
        .find(is_ours)
}

/// An MCP tool as an OpenAI-style function for the chat request.
fn as_chat_tool(t: &Value) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": t["name"],
            "description": t["description"].as_str().unwrap_or(""),
            "parameters": if t["inputSchema"].is_object() { t["inputSchema"].clone() } else { json!({"type": "object"}) },
        }
    })
}

/// The text of a tool's result; an image or other non-text content is named, not given.
fn result_text(r: &Value) -> String {
    r["content"]
        .as_array()
        .map(|cs| {
            cs.iter()
                .map(|c| match c["type"].as_str() {
                    Some("text") => c["text"].as_str().unwrap_or("").to_string(),
                    Some(other) => format!("[{other} content not shown]"),
                    None => String::new(),
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

fn cut(s: &str) -> String {
    match s.char_indices().nth(RESULT_LIMIT) {
        Some((i, _)) => format!("{}\n[cut at {RESULT_LIMIT} characters]", &s[..i]),
        None => s.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reply_is_read_from_json_or_from_an_event_stream() {
        let j = r#"{"jsonrpc":"2.0","id":2,"result":{"tools":[]}}"#;
        assert!(response(j, 2).is_some());
        assert!(response(j, 3).is_none());
        let sse = format!("event: message\ndata: {j}\n\n");
        assert_eq!(response(&sse, 2).unwrap()["result"]["tools"], json!([]));
    }

    #[test]
    fn a_tool_becomes_a_chat_function_and_its_text_is_cut() {
        let t = as_chat_tool(&json!({"name": "search", "description": "Web search",
            "inputSchema": {"type": "object", "properties": {"query": {"type": "string"}}}}));
        assert_eq!(t["function"]["name"], "search");
        assert_eq!(
            t["function"]["parameters"]["properties"]["query"]["type"],
            "string"
        );
        let long = "é".repeat(RESULT_LIMIT + 10);
        assert!(cut(&long).ends_with("[cut at 8000 characters]"));
        assert_eq!(cut("short"), "short");
        let r = json!({"content": [{"type": "text", "text": "a"}, {"type": "image", "data": "…"}]});
        assert_eq!(result_text(&r), "a\n[image content not shown]");
    }
}
