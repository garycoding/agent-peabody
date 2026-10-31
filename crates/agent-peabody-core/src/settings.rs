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
        }
    }
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
