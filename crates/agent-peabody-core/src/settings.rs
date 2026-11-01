// SPDX-FileCopyrightText: 2026 Gary Frattarola <garyf@parkviewlab.ai>
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The owner's settings, kept as JSON in the data folder.

use serde::{Deserialize, Serialize};
use std::path::Path;

/// Where a service answers, and the bearer token it needs over the LAN (empty locally).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Endpoint {
    pub url: String,
    pub token: String,
}

impl Endpoint {
    pub fn new(url: &str) -> Self {
        Self {
            url: url.into(),
            token: String::new(),
        }
    }

    /// The URL of a path on this endpoint, without a doubled slash.
    pub fn at(&self, path: &str) -> String {
        format!("{}{}", self.url.trim_end_matches('/'), path)
    }
}

/// The system prompt used where the owner has written none: replies are spoken, so they
/// must read well aloud.
pub const DEFAULT_SYSTEM_PROMPT: &str = "You are Peabody, a helpful assistant in a spoken \
conversation. Your replies are read aloud, so answer briefly, in plain spoken sentences, \
with no Markdown, lists, headings, code or emoji.";

/// The longest reply asked of the model, in tokens (2026-10-31): about three times the
/// longest reply seen (Kipling's "If", some 650 tokens), and ten minutes of speech.
pub const DEFAULT_MAX_REPLY_TOKENS: u32 = 2048;

/// The phrases said at once when an utterance arrives, while the model is asked; the
/// owner's own (2026-10-31).
pub const DEFAULT_INSTANT_REPLIES: [&str; 4] = [
    "Allow me to think about that...",
    "One moment please...",
    "Thinking...",
    "Allow me time to prepare a cogent response...",
];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// The app name graptostream addresses Peabody's utterances to (`/events?to=`); the
    /// spoken name is mapped to it in graptostream's own settings.
    pub name: String,
    /// The phonorheos voice Peabody speaks with.
    pub voice: String,
    pub system_prompt: String,
    pub graptostream: Endpoint,
    pub phonorheos: Endpoint,
    /// An OpenAI-compatible server: llama.cpp or LM Studio.
    pub llm: Endpoint,
    pub model: String,
    /// The context window's size in tokens, where the server does not report it or the
    /// owner overrides what it reports.
    pub context_size: Option<u32>,
    /// MCP servers (Streamable HTTP) whose tools the model is offered, such as
    /// aqua-roaming's web search at `http://trixie.local:35830/mcp`. Read also from v0.4.0's
    /// single server.
    #[serde(deserialize_with = "servers")]
    pub tools: Vec<Endpoint>,
    /// Whether Peabody listens and answers; off, it holds no listen request.
    pub enabled: bool,
    /// Whether a reasoning model may think before it answers; off by default, since the
    /// thinking made a one-sentence answer take 19.9 s instead of 2.1 s on pacific.
    pub thinking: bool,
    /// The most tokens the model may use for one reply (`max_tokens`); 0 sets no limit. A
    /// reasoning model's thinking counts toward it where the server counts it.
    pub max_reply_tokens: u32,
    /// Said at once on each utterance, one chosen at random, before the answer; phonorheos
    /// keeps their speech so that they play without delay. None turns this off.
    pub instant_replies: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            name: "peabody".into(),
            voice: "bm_lewis".into(),
            system_prompt: DEFAULT_SYSTEM_PROMPT.into(),
            graptostream: Endpoint::new("http://127.0.0.1:35860"),
            phonorheos: Endpoint::new("http://127.0.0.1:35861"),
            llm: Endpoint::default(),
            model: String::new(),
            context_size: None,
            tools: Vec::new(),
            enabled: true,
            thinking: false,
            max_reply_tokens: DEFAULT_MAX_REPLY_TOKENS,
            instant_replies: DEFAULT_INSTANT_REPLIES.map(String::from).to_vec(),
        }
    }
}

/// The MCP servers, as a list or as v0.4.0's single server (dropped where it has no
/// address).
fn servers<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Endpoint>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Servers {
        List(Vec<Endpoint>),
        One(Endpoint),
    }
    Ok(match Servers::deserialize(d)? {
        Servers::List(l) => l,
        Servers::One(e) if e.url.trim().is_empty() => Vec::new(),
        Servers::One(e) => vec![e],
    })
}

impl Settings {
    /// The saved settings, or the defaults where there are none or they cannot be read.
    pub fn load(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    /// Saves by writing a new file and renaming it over the old, so that a crash never
    /// leaves half a file; private to the user, since it holds the tokens.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        crate::store::write_private(
            path,
            &serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        assert_eq!(Settings::load(&path), Settings::default());
        let s = Settings {
            llm: Endpoint {
                url: "http://pacific.local:1234/".into(),
                token: "t".into(),
            },
            context_size: Some(8192),
            ..Settings::default()
        };
        s.save(&path).unwrap();
        assert_eq!(Settings::load(&path), s);
        // v0.4.0 kept one tools server, which becomes the first of the list.
        std::fs::write(
            &path,
            r#"{"tools": {"url": "http://trixie.local:35830/mcp", "token": ""}, "model": "m"}"#,
        )
        .unwrap();
        let old = Settings::load(&path);
        assert_eq!(old.model, "m", "the rest of the settings are kept");
        assert_eq!(old.tools, [Endpoint::new("http://trixie.local:35830/mcp")]);
        assert!(old.enabled);
        assert_eq!(
            s.llm.at("/v1/models"),
            "http://pacific.local:1234/v1/models"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn a_partial_file_keeps_the_other_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, br#"{"name": "jeeves"}"#).unwrap();
        let s = Settings::load(&path);
        assert_eq!(s.name, "jeeves");
        assert_eq!(s.voice, "bm_lewis");
    }
}
