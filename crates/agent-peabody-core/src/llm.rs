// SPDX-FileCopyrightText: 2026 Gary Frattarola <garyf@parkviewlab.ai>
// SPDX-License-Identifier: AGPL-3.0-or-later

//! An OpenAI-compatible chat server: llama.cpp's `llama-server` or LM Studio.

use crate::http;
use crate::settings::Endpoint;
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
}

impl Message {
    pub fn new(role: &str, content: &str) -> Self {
        Self {
            role: role.into(),
            content: content.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Reply {
    /// The answer alone: a reasoning model's reasoning (`reasoning_content`, or a leading
    /// `<think>` block) is left out.
    pub content: String,
    /// The tokens the request and the kept answer take together, where the server reports
    /// them: the reasoning's tokens are not counted, since the reasoning is not kept.
    pub total_tokens: Option<u32>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    /// The request did not fit the model's context window.
    ContextExceeded(String),
    Other(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::ContextExceeded(s) => write!(
                f,
                "the conversation is longer than the context window ({s})"
            ),
            Error::Other(s) => f.write_str(s),
        }
    }
}

/// Whether an error body says the context window was exceeded; the servers word it
/// differently (llama.cpp: "exceeds the available context size"; LM Studio: "context
/// length", "overflows").
fn is_context_error(body: &str) -> bool {
    let b = body.to_lowercase();
    b.contains("context")
        && [
            "exceed",
            "overflow",
            "too long",
            "context length",
            "context size",
        ]
        .iter()
        .any(|w| b.contains(w))
}

/// Removes a leading `<think>…</think>` block, which some servers leave in the content.
fn strip_thinking(s: &str) -> &str {
    let t = s.trim_start();
    if let Some(rest) = t.strip_prefix("<think>") {
        return rest.split_once("</think>").map_or("", |(_, after)| after);
    }
    s
}

pub fn parse_reply(v: &Value) -> Reply {
    let content = v["choices"][0]["message"]["content"].as_str().unwrap_or("");
    let u = &v["usage"];
    let reasoning = u["completion_tokens_details"]["reasoning_tokens"]
        .as_u64()
        .unwrap_or(0);
    Reply {
        content: strip_thinking(content).trim().to_string(),
        total_tokens: u["total_tokens"]
            .as_u64()
            .or_else(|| Some(u["prompt_tokens"].as_u64()? + u["completion_tokens"].as_u64()?))
            .map(|n| n.saturating_sub(reasoning) as u32),
    }
}

pub async fn chat(
    c: &reqwest::Client,
    ep: &Endpoint,
    model: &str,
    messages: &[Message],
) -> Result<Reply, Error> {
    let mut body = json!({ "messages": messages, "stream": false });
    if !model.is_empty() {
        body["model"] = json!(model);
    }
    let r = http::request(c, Method::POST, ep, "/v1/chat/completions")
        .json(&body)
        .send()
        .await
        .map_err(|e| Error::Other(http::describe(&e)))?;
    let status = r.status();
    let text = r
        .text()
        .await
        .map_err(|e| Error::Other(http::describe(&e)))?;
    if !status.is_success() {
        let short: String = text.chars().take(200).collect();
        return Err(if is_context_error(&text) {
            Error::ContextExceeded(short)
        } else {
            Error::Other(format!("answered {status}: {short}"))
        });
    }
    let v: Value = serde_json::from_str(&text)
        .map_err(|e| Error::Other(format!("an unreadable reply: {e}")))?;
    Ok(parse_reply(&v))
}

/// The models the server offers (`GET /v1/models`).
pub async fn models(c: &reqwest::Client, ep: &Endpoint) -> Result<Vec<String>, String> {
    let r = http::request(c, Method::GET, ep, "/v1/models")
        .send()
        .await
        .map_err(|e| http::describe(&e))?;
    if !r.status().is_success() {
        return Err(format!("answered {}", r.status()));
    }
    let v: Value = r.json().await.map_err(|e| http::describe(&e))?;
    Ok(v["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| m["id"].as_str().map(String::from))
        .collect())
}

/// The context window of `model` as the server reports it: llama.cpp's `GET /props`
/// (`n_ctx`), then LM Studio's `GET /api/v0/models` (`loaded_context_length` of the
/// model, or of a loaded instance of it such as `name:2`).
pub async fn context_size(c: &reqwest::Client, ep: &Endpoint, model: &str) -> Option<u32> {
    let mut rb = http::request(c, Method::GET, ep, "/props");
    if !model.is_empty() {
        rb = rb.query(&[("model", model)]);
    }
    if let Ok(r) = rb.send().await
        && r.status().is_success()
        && let Ok(v) = r.json::<Value>().await
        && let Some(n) = v["default_generation_settings"]["n_ctx"]
            .as_u64()
            .or_else(|| v["n_ctx"].as_u64())
    {
        return Some(n as u32);
    }
    let r = http::request(c, Method::GET, ep, "/api/v0/models")
        .send()
        .await
        .ok()?;
    if !r.status().is_success() {
        return None;
    }
    lm_studio_context(&r.json::<Value>().await.ok()?, model)
}

pub fn lm_studio_context(v: &Value, model: &str) -> Option<u32> {
    let loaded: Vec<&Value> = v["data"]
        .as_array()?
        .iter()
        .filter(|m| m["loaded_context_length"].is_u64())
        .collect();
    let base = |id: &str| id.split_once(':').map_or(id, |(b, _)| b).to_string();
    let pick = if model.is_empty() {
        loaded.first().copied()
    } else {
        loaded
            .iter()
            .find(|m| m["id"] == model)
            .or_else(|| {
                loaded
                    .iter()
                    .find(|m| m["id"].as_str().is_some_and(|id| base(id) == base(model)))
            })
            .copied()
    };
    pick.and_then(|m| m["loaded_context_length"].as_u64())
        .map(|n| n as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_answer_and_leaves_out_the_reasoning() {
        let v = json!({
            "choices": [{"message": {"content": "Four.", "reasoning_content": "two and two"}}],
            "usage": {"prompt_tokens": 19, "completion_tokens": 7, "total_tokens": 26}
        });
        assert_eq!(
            parse_reply(&v),
            Reply {
                content: "Four.".into(),
                total_tokens: Some(26)
            }
        );
        let v = json!({"choices": [{"message": {"content": "<think>hmm</think>\n\nFour."}}],
                       "usage": {"prompt_tokens": 10, "completion_tokens": 5}});
        assert_eq!(
            parse_reply(&v),
            Reply {
                content: "Four.".into(),
                total_tokens: Some(15)
            }
        );
        let v = json!({"choices": [{"message": {"content": "<think>still thinking"}}]});
        assert_eq!(
            parse_reply(&v),
            Reply {
                content: String::new(),
                total_tokens: None
            }
        );
        // The reasoning's tokens are not counted, as LM Studio reports them for Gemma.
        let v = json!({"choices": [{"message": {"content": "Four."}}],
                       "usage": {"prompt_tokens": 569, "completion_tokens": 3797, "total_tokens": 4366,
                                 "completion_tokens_details": {"reasoning_tokens": 3787}}});
        assert_eq!(parse_reply(&v).total_tokens, Some(579));
    }

    #[test]
    fn recognises_context_errors() {
        assert!(is_context_error(
            r#"{"error":{"message":"the request exceeds the available context size, try increasing it"}}"#
        ));
        assert!(is_context_error(
            "Trying to keep the first 9000 tokens when context the overflows"
        ));
        assert!(!is_context_error("model not found"));
    }

    #[test]
    fn finds_lm_studio_loaded_context() {
        let v = json!({"data": [
            {"id": "google/gemma-4-12b:2", "state": "loaded", "loaded_context_length": 8192},
            {"id": "google/gemma-4-12b", "state": "not-loaded", "max_context_length": 262144},
            {"id": "other", "state": "loaded", "loaded_context_length": 4096}
        ]});
        assert_eq!(lm_studio_context(&v, "google/gemma-4-12b:2"), Some(8192));
        assert_eq!(lm_studio_context(&v, "google/gemma-4-12b"), Some(8192));
        assert_eq!(lm_studio_context(&v, "other"), Some(4096));
        assert_eq!(lm_studio_context(&v, "absent"), None);
        assert_eq!(lm_studio_context(&v, ""), Some(8192));
    }
}
