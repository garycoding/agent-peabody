// SPDX-FileCopyrightText: 2026 Gary Frattarola <garyf@parkviewlab.ai>
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Hearing through graptostream: a listen request held for as long as Peabody runs, and
//! the final utterances addressed to Peabody's name, read from `/events` as Server-Sent
//! Events and resumed from the last event after a reconnection.

use crate::http;
use crate::settings::Endpoint;
use futures_util::StreamExt;
use reqwest::{Method, StatusCode};
use serde::Deserialize;
use serde_json::json;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;

pub const CLIENT: &str = "peabody";

/// One Server-Sent Event: its `id`, `event` and joined `data` lines.
#[derive(Debug, Default, PartialEq)]
pub struct Event {
    pub id: Option<String>,
    pub event: Option<String>,
    pub data: String,
}

/// Cuts a byte stream into events; text that ends mid-event waits for the rest.
#[derive(Default)]
pub struct SseParser {
    buf: String,
    current: Event,
    has_data: bool,
}

impl SseParser {
    pub fn push(&mut self, chunk: &str) -> Vec<Event> {
        self.buf.push_str(chunk);
        let mut out = Vec::new();
        while let Some(n) = self.buf.find('\n') {
            let line: String = self.buf.drain(..=n).collect();
            let line = line.trim_end_matches(['\n', '\r']);
            if line.is_empty() {
                if self.has_data {
                    out.push(std::mem::take(&mut self.current));
                } else {
                    self.current = Event::default();
                }
                self.has_data = false;
            } else if line.starts_with(':') {
                // A comment: graptostream's keep-alive.
            } else {
                let (field, value) = line.split_once(':').unwrap_or((line, ""));
                let value = value.strip_prefix(' ').unwrap_or(value);
                match field {
                    "id" => self.current.id = Some(value.into()),
                    "event" => self.current.event = Some(value.into()),
                    "data" => {
                        if self.has_data {
                            self.current.data.push('\n');
                        }
                        self.current.data.push_str(value);
                        self.has_data = true;
                    }
                    _ => {}
                }
            }
        }
        out
    }
}

#[derive(Debug, Deserialize)]
struct Segment {
    #[serde(default)]
    state: String,
    #[serde(default)]
    text: String,
    #[serde(default)]
    to: Option<String>,
}

/// The utterance an event carries, if it is final text addressed to `name`, with the
/// spoken name already removed by graptostream.
pub fn utterance(ev: &Event, name: &str) -> Option<String> {
    if ev.event.as_deref().is_some_and(|e| e != "segment") {
        return None;
    }
    let seg: Segment = serde_json::from_str(&ev.data).ok()?;
    let text = seg.text.trim();
    (seg.state == "final"
        && seg.to.is_some_and(|t| t.eq_ignore_ascii_case(name))
        && !text.is_empty())
    .then(|| text.to_string())
}

/// The state of a connection, reported to the window.
#[derive(Debug, Clone, PartialEq)]
pub enum Link {
    Connected,
    Down(String),
}

/// Reads Peabody's utterances for ever, reconnecting after a failure (1 s, doubling to
/// 30 s) and resuming from the last event seen.
pub async fn read_utterances(
    c: reqwest::Client,
    ep: Endpoint,
    name: String,
    utterances: mpsc::UnboundedSender<String>,
    link: impl Fn(Link) + Send + 'static,
) {
    let mut last_id: Option<String> = None;
    let mut wait = Duration::from_secs(1);
    loop {
        let mut rb = http::request(&c, Method::GET, &ep, "/events").query(&[
            ("to", name.as_str()),
            ("final", "true"),
            ("client", CLIENT),
        ]);
        if let Some(id) = &last_id {
            rb = rb.header("Last-Event-ID", id);
        }
        match rb.send().await {
            Ok(r) if r.status().is_success() => {
                link(Link::Connected);
                wait = Duration::from_secs(1);
                let mut parser = SseParser::default();
                let mut body = r.bytes_stream();
                let ended = loop {
                    match body.next().await {
                        Some(Ok(bytes)) => {
                            for ev in parser.push(&String::from_utf8_lossy(&bytes)) {
                                if ev.id.is_some() {
                                    last_id.clone_from(&ev.id);
                                }
                                if let Some(u) = utterance(&ev, &name)
                                    && utterances.send(u).is_err()
                                {
                                    return;
                                }
                            }
                        }
                        Some(Err(e)) => break http::describe(&e),
                        None => break "the stream ended".to_string(),
                    }
                };
                link(Link::Down(ended));
            }
            Ok(r) => link(Link::Down(format!("answered {}", r.status()))),
            Err(e) => link(Link::Down(http::describe(&e))),
        }
        tokio::time::sleep(wait).await;
        wait = (wait * 2).min(Duration::from_secs(30));
    }
}

/// The listen request Peabody holds, so that graptostream in "When asked" mode listens
/// whilst Peabody runs.
#[derive(Clone)]
pub struct Listen {
    c: reqwest::Client,
    ep: Endpoint,
    id: Arc<Mutex<Option<String>>>,
}

#[derive(Deserialize)]
struct Created {
    id: serde_json::Value,
}

impl Listen {
    pub fn new(c: reqwest::Client, ep: Endpoint) -> Self {
        Self {
            c,
            ep,
            id: Arc::new(Mutex::new(None)),
        }
    }

    /// Asks for `minutes` of listening and renews the request every `renew` for ever,
    /// asking afresh where it has lapsed and retrying after a failure.
    pub async fn hold(self, minutes: f64, renew: Duration) {
        loop {
            let held = self.id.lock().unwrap().clone();
            let ok = match held {
                Some(id) => {
                    let r = http::request(&self.c, Method::PUT, &self.ep, &format!("/listen/{id}"))
                        .json(&json!({ "minutes": minutes }))
                        .send()
                        .await;
                    match r {
                        Ok(r) if r.status() == StatusCode::NOT_FOUND => {
                            *self.id.lock().unwrap() = None;
                            continue;
                        }
                        Ok(r) => r.status().is_success(),
                        Err(_) => false,
                    }
                }
                None => {
                    let r = http::request(&self.c, Method::POST, &self.ep, "/listen")
                        .json(&json!({ "client": CLIENT, "minutes": minutes }))
                        .send()
                        .await;
                    match r {
                        Ok(r) if r.status().is_success() => match r.json::<Created>().await {
                            Ok(c) => {
                                let id = match c.id {
                                    serde_json::Value::String(s) => s,
                                    other => other.to_string(),
                                };
                                *self.id.lock().unwrap() = Some(id);
                                true
                            }
                            Err(_) => false,
                        },
                        _ => false,
                    }
                }
            };
            tokio::time::sleep(if ok {
                renew
            } else {
                renew.min(Duration::from_secs(10))
            })
            .await;
        }
    }

    /// Withdraws the request, if one is held.
    pub async fn withdraw(&self) {
        let held = self.id.lock().unwrap().take();
        if let Some(id) = held {
            let _ = http::request(&self.c, Method::DELETE, &self.ep, &format!("/listen/{id}"))
                .send()
                .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_events_split_across_chunks() {
        let mut p = SseParser::default();
        assert!(p.push(": keep-alive\n\nid: 7\nevent: seg").is_empty());
        let evs = p.push("ment\ndata: {\"a\":1}\r\n\r\nid: 8\ndata: x\ndata: y\n\n");
        assert_eq!(
            evs,
            vec![
                Event {
                    id: Some("7".into()),
                    event: Some("segment".into()),
                    data: "{\"a\":1}".into()
                },
                Event {
                    id: Some("8".into()),
                    event: None,
                    data: "x\ny".into()
                },
            ]
        );
    }

    #[test]
    fn takes_only_final_text_addressed_to_the_name() {
        let ev = |data: &str| Event {
            id: None,
            event: Some("segment".into()),
            data: data.into(),
        };
        let fin =
            r#"{"state":"final","text":"what is two plus two?","to":"Peabody","heard":"Peabody"}"#;
        assert_eq!(
            utterance(&ev(fin), "peabody").as_deref(),
            Some("what is two plus two?")
        );
        assert_eq!(
            utterance(
                &ev(r#"{"state":"provisional","text":"what","to":"peabody"}"#),
                "peabody"
            ),
            None
        );
        assert_eq!(
            utterance(
                &ev(r#"{"state":"final","text":"hello","to":"claude"}"#),
                "peabody"
            ),
            None
        );
        assert_eq!(
            utterance(&ev(r#"{"state":"final","text":"hello"}"#), "peabody"),
            None
        );
        assert_eq!(
            utterance(
                &ev(r#"{"state":"final","text":"  ","to":"peabody"}"#),
                "peabody"
            ),
            None
        );
    }
}
