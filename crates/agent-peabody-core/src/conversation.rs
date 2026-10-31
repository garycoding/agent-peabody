// SPDX-FileCopyrightText: 2026 Gary Frattarola <garyf@parkviewlab.ai>
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The conversation Peabody keeps, since the chat endpoint keeps none: the exchanges word
//! for word, and once the conversation fills three quarters of the context window, a
//! summary of the older part in place of it, with the last exchanges kept as they were.

use crate::llm::Message;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// The exchanges (a question and its answer) kept word for word by a compaction.
pub const KEEP_EXCHANGES: usize = 4;

/// The share of the context window at which the conversation is compacted.
pub const COMPACT_AT: f64 = 0.75;

const SUMMARY_PROMPT: &str = "You summarise conversations. Summarise the conversation below \
between a person and Peabody, an assistant, so that Peabody can carry it on without the full \
text: keep every fact, name, number, decision, request and open question, and what each side \
said about them. Write plain prose, in the third person, without a preamble.";

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Conversation {
    /// The summary of what came before `messages`, if the conversation has been compacted.
    pub summary: Option<String>,
    /// User and assistant messages, alternating, oldest first.
    pub messages: Vec<Message>,
    /// The tokens the last request and its reply took, as the server reported them.
    pub tokens: Option<u32>,
}

impl Conversation {
    pub fn load(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        crate::store::write_private(
            path,
            &serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?,
        )
    }

    /// The messages a chat request sends: the system prompt, the summary, the exchanges.
    pub fn request(&self, system_prompt: &str) -> Vec<Message> {
        let mut out = Vec::with_capacity(self.messages.len() + 2);
        if !system_prompt.trim().is_empty() {
            out.push(Message::new("system", system_prompt));
        }
        if let Some(s) = &self.summary {
            out.push(Message::new(
                "system",
                &format!("A summary of the conversation so far:\n{s}"),
            ));
        }
        out.extend(self.messages.iter().cloned());
        out
    }

    /// Whether the conversation has reached three quarters of a window of `context` tokens.
    pub fn needs_compaction(&self, context: u32) -> bool {
        self.tokens
            .is_some_and(|t| f64::from(t) >= COMPACT_AT * f64::from(context))
            && self.compactable() > 0
    }

    /// How many messages a compaction would summarise: all but the last exchanges.
    fn compactable(&self) -> usize {
        self.messages.len().saturating_sub(2 * KEEP_EXCHANGES)
    }

    /// The request that asks for a summary of the older part, the earlier summary included.
    pub fn compaction_request(&self) -> Option<Vec<Message>> {
        let n = self.compactable();
        if n == 0 {
            return None;
        }
        let mut text = String::new();
        if let Some(s) = &self.summary {
            text.push_str(&format!("(A summary of what came before:)\n{s}\n\n"));
        }
        for m in &self.messages[..n] {
            let who = if m.role == "assistant" {
                "Peabody"
            } else {
                "Person"
            };
            text.push_str(&format!("{who}: {}\n\n", m.content));
        }
        Some(vec![
            Message::new("system", SUMMARY_PROMPT),
            Message::new("user", text.trim_end()),
        ])
    }

    /// Puts the summary in place of the older part; the size is unknown until the next reply.
    pub fn compact(&mut self, summary: String) {
        let n = self.compactable();
        self.messages.drain(..n);
        self.summary = Some(summary);
        self.tokens = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_exchanges(n: usize) -> Conversation {
        let mut c = Conversation::default();
        for i in 0..n {
            c.messages.push(Message::new("user", &format!("q{i}")));
            c.messages.push(Message::new("assistant", &format!("a{i}")));
        }
        c
    }

    #[test]
    fn compacts_at_three_quarters_and_keeps_the_last_four_exchanges() {
        let mut c = with_exchanges(6);
        c.tokens = Some(6143);
        assert!(!c.needs_compaction(8192));
        c.tokens = Some(6144);
        assert!(c.needs_compaction(8192));
        let req = c.compaction_request().unwrap();
        assert_eq!(
            req[1].content,
            "Person: q0\n\nPeabody: a0\n\nPerson: q1\n\nPeabody: a1"
        );
        c.compact("They spoke of q0 and q1.".into());
        assert_eq!(c.messages.len(), 8);
        assert_eq!(c.messages[0].content, "q2");
        assert_eq!(c.tokens, None);
        let sent = c.request("Be brief.");
        assert_eq!(sent[0], Message::new("system", "Be brief."));
        assert!(sent[1].content.ends_with("They spoke of q0 and q1."));
        assert_eq!(sent.len(), 10);
        // A second compaction carries the first summary into the new one.
        c.messages.extend(with_exchanges(1).messages);
        assert!(
            c.compaction_request().unwrap()[1]
                .content
                .starts_with("(A summary of what came before:)")
        );
    }

    #[test]
    fn four_exchanges_or_fewer_are_never_compacted() {
        let mut c = with_exchanges(4);
        c.tokens = Some(10_000);
        assert!(!c.needs_compaction(8192));
        assert!(c.compaction_request().is_none());
    }

    #[test]
    fn survives_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("conversation.json");
        let mut c = with_exchanges(2);
        c.summary = Some("s".into());
        c.tokens = Some(42);
        c.save(&path).unwrap();
        assert_eq!(Conversation::load(&path), c);
    }
}
