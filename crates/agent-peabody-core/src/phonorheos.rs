// SPDX-FileCopyrightText: 2026 Gary Frattarola <garyf@parkviewlab.ai>
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Speaking through phonorheos (`POST /speak`), which from v0.3.0 can also keep the
//! speech for a text so that it plays at once next time.

use crate::http;
use crate::settings::Endpoint;
use reqwest::Method;
use serde_json::json;

/// The client name phonorheos shows for Peabody's speech (on the LAN, the token's name).
pub const CLIENT: &str = "peabody";

/// What phonorheos is to do with a text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Say {
    /// Play it now.
    pub speak: bool,
    /// Keep its speech, so that the same text plays at once next time.
    pub cache: bool,
}

impl Say {
    /// An answer: spoken, not kept.
    pub const ANSWER: Say = Say {
        speak: true,
        cache: false,
    };
    /// An instant reply made ready at start-up: kept, not spoken.
    pub const PREPARE: Say = Say {
        speak: false,
        cache: true,
    };
    /// An instant reply when wanted: spoken, and kept again if phonorheos has lost it.
    pub const INSTANT: Say = Say {
        speak: true,
        cache: true,
    };
}

/// Queues `text` in `voice`; returns once phonorheos has queued it, not when it is spoken.
pub async fn speak(
    c: &reqwest::Client,
    ep: &Endpoint,
    text: &str,
    voice: &str,
    say: Say,
) -> Result<(), String> {
    let mut body =
        json!({ "text": text, "client": CLIENT, "speak": say.speak, "cache": say.cache });
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
