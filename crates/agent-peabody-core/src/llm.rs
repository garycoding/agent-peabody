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
    /// The tools the model asks to have called before it answers, as the server gave
    /// them (`id`, and `function` with `name` and `arguments`); empty for an answer.
    pub tool_calls: Vec<Value>,
    /// The model's reasoning, where the server returns it apart (`reasoning_content`).
    pub reasoning: String,
    /// The server's token counts, as it reported them.
    pub usage: Value,
    /// Why the reply ended (`stop`, `length`, `tool_calls`…), as the server gave it.
    pub finish_reason: String,
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
    // Gemma 4's thinking channel, which LM Studio sometimes leaves in the content
    // ("<|channel>s_thought\n<channel|>Trees are…"); phonorheos read it aloud.
    if let Some(rest) = t.strip_prefix("<|channel>") {
        return rest.split_once("<channel|>").map_or("", |(_, after)| after);
    }
    s
}

pub fn parse_reply(v: &Value) -> Reply {
    let message = &v["choices"][0]["message"];
    let content = message["content"].as_str().unwrap_or("");
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
        tool_calls: message["tool_calls"]
            .as_array()
            .cloned()
            .unwrap_or_default(),
        reasoning: message["reasoning_content"]
            .as_str()
            .unwrap_or_default()
            .trim()
            .to_string(),
        usage: u.clone(),
        finish_reason: v["choices"][0]["finish_reason"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
    }
}

/// How long an answer may take: a reasoning model on a small machine took eight minutes
/// for one long answer, so the limit only catches a server that never answers.
pub const ANSWER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15 * 60);

/// Whether the server is Anthropic's, whose Claude 5 models think by default through its
/// OpenAI-compatible layer, which ignores `reasoning_effort`.
pub fn is_anthropic(ep: &Endpoint) -> bool {
    ep.url
        .split("://")
        .last()
        .and_then(|rest| rest.split('/').next())
        .is_some_and(|host| host == "anthropic.com" || host.ends_with(".anthropic.com"))
}

/// `max_tokens` is the most the reply may use; 0 sets no limit.
pub async fn chat(
    c: &reqwest::Client,
    ep: &Endpoint,
    model: &str,
    think: bool,
    max_tokens: u32,
    messages: &impl Serialize,
    tools: &[Value],
) -> Result<Reply, Error> {
    chat_with(
        c,
        ep,
        model,
        think,
        is_anthropic(ep),
        max_tokens,
        messages,
        tools,
    )
    .await
}

/// As [`chat`], and where `thinking_field` is set with thinking off, also sends
/// `thinking: {"type": "disabled"}`; a server that answers 400 naming `thinking` is asked
/// again without it, so that a model that rejects the field still answers.
#[allow(clippy::too_many_arguments)]
pub async fn chat_with(
    c: &reqwest::Client,
    ep: &Endpoint,
    model: &str,
    think: bool,
    thinking_field: bool,
    max_tokens: u32,
    messages: &impl Serialize,
    tools: &[Value],
) -> Result<Reply, Error> {
    let off = !think && thinking_field;
    match send(c, ep, model, think, off, max_tokens, messages, tools).await {
        Err(Error::Other(e)) if off && e.starts_with("answered 400") && e.contains("thinking") => {
            send(c, ep, model, think, false, max_tokens, messages, tools).await
        }
        r => r,
    }
}

#[allow(clippy::too_many_arguments)]
async fn send(
    c: &reqwest::Client,
    ep: &Endpoint,
    model: &str,
    think: bool,
    thinking_disabled: bool,
    max_tokens: u32,
    messages: &impl Serialize,
    tools: &[Value],
) -> Result<Reply, Error> {
    let mut body = json!({ "messages": messages, "stream": false });
    if max_tokens > 0 {
        body["max_tokens"] = json!(max_tokens);
    }
    if thinking_disabled {
        body["thinking"] = json!({"type": "disabled"});
    }
    if !model.is_empty() {
        body["model"] = json!(model);
    }
    if !tools.is_empty() {
        body["tools"] = json!(tools);
    }
    // LM Studio skips a reasoning model's thinking for "none"; "low" and the chat
    // template's enable_thinking made no difference there (Gemma 4, 2026-10-31).
    if !think {
        body["reasoning_effort"] = json!("none");
    }
    let r = http::request(c, Method::POST, ep, "/v1/chat/completions")
        .timeout(ANSWER_TIMEOUT)
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

/// The models the server offers (`GET /v1/models`). Anthropic's server answers 400,
/// "anthropic-version: header is required", without that header, though its page calls it
/// optional; LM Studio and llama.cpp ignore it. Chat completions need no such header.
pub async fn models(c: &reqwest::Client, ep: &Endpoint) -> Result<Vec<String>, String> {
    let r = http::request(c, Method::GET, ep, "/v1/models")
        .header("anthropic-version", "2023-06-01")
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
    fn only_anthropics_own_address_is_anthropic() {
        let is = |u: &str| is_anthropic(&Endpoint::new(u));
        assert!(is("https://api.anthropic.com"));
        assert!(is("https://api.anthropic.com/"));
        assert!(!is("http://127.0.0.1:1234"));
        assert!(!is("http://pacific.local:1234"));
        assert!(!is("https://anthropic.com.evil.example"));
        assert!(!is("https://notanthropic.com"));
    }

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
                total_tokens: Some(26),
                tool_calls: vec![],
                reasoning: "two and two".into(),
                usage: json!({"prompt_tokens": 19, "completion_tokens": 7, "total_tokens": 26}),
                finish_reason: String::new(),
            }
        );
        let v = json!({"choices": [{"message": {"content": "<think>hmm</think>\n\nFour."}}],
                       "usage": {"prompt_tokens": 10, "completion_tokens": 5}});
        assert_eq!(
            parse_reply(&v),
            Reply {
                content: "Four.".into(),
                total_tokens: Some(15),
                tool_calls: vec![],
                reasoning: String::new(),
                usage: json!({"prompt_tokens": 10, "completion_tokens": 5}),
                finish_reason: String::new(),
            }
        );
        let v = json!({"choices": [{"message": {"content": "<think>still thinking"}}]});
        assert_eq!(
            parse_reply(&v),
            Reply {
                content: String::new(),
                total_tokens: None,
                tool_calls: vec![],
                reasoning: String::new(),
                usage: Value::Null,
                finish_reason: String::new(),
            }
        );
        let v = json!({"choices": [{"message": {"content": "<|channel>s_thought\n<channel|>Trees are old."}}]});
        assert_eq!(parse_reply(&v).content, "Trees are old.");
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
