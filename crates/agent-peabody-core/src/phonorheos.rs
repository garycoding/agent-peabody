// SPDX-FileCopyrightText: 2026 Gary Frattarola <garyf@parkviewlab.ai>
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Speaking through phonorheos (`POST /speak`).

use crate::http;
use crate::settings::Endpoint;
use reqwest::Method;
use serde_json::json;

/// The client name phonorheos shows for Peabody's speech (on the LAN, the token's name).
pub const CLIENT: &str = "peabody";

/// Queues `text` in `voice`; returns once phonorheos has queued it, not when it is spoken.
pub async fn speak(
    c: &reqwest::Client,
    ep: &Endpoint,
    text: &str,
    voice: &str,
) -> Result<(), String> {
    let mut body = json!({ "text": text, "client": CLIENT });
    if !voice.is_empty() {
        body["voice"] = json!(voice);
    }
    let r = http::request(c, Method::POST, ep, "/speak")
        .json(&body)
        .send()
        .await
        .map_err(|e| http::describe(&e))?;
    if r.status().is_success() {
        Ok(())
    } else {
        Err(format!("answered {}", r.status()))
    }
}

/// Whether phonorheos answers (`GET /health`).
pub async fn health(c: &reqwest::Client, ep: &Endpoint) -> Result<(), String> {
    let r = http::request(c, Method::GET, ep, "/health")
        .send()
        .await
        .map_err(|e| http::describe(&e))?;
    if r.status().is_success() {
        Ok(())
    } else {
        Err(format!("answered {}", r.status()))
    }
}
