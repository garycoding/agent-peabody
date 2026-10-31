// SPDX-FileCopyrightText: 2026 Gary Frattarola <garyf@parkviewlab.ai>
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What Peabody does: each utterance heard through graptostream goes into the
//! conversation, to the model, and its answer out through phonorheos, one at a time and
//! in the order heard. Where an MCP server is set, its tools (a web search, say) are
//! offered to the model, and Peabody calls the ones it asks for before it answers.

use crate::conversation::Conversation;
use crate::graptostream::{self, Link, Listen};
use crate::llm::{self, Message};
use crate::mcp;
use crate::phonorheos::Say;
use crate::settings::Settings;
use crate::{http, phonorheos};
use serde::Serialize;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// Spoken when the model returns an empty answer.
pub const NO_ANSWER: &str = "I have no answer to that.";
/// The most rounds of tool calls for one question; after them the model is asked to
/// answer with what it has, so that a model that keeps searching cannot loop.
pub const MAX_TOOL_ROUNDS: usize = 4;
/// Spoken when the model cannot be reached or refuses the request.
pub const NO_MODEL: &str = "I could not get an answer from the model.";

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Service {
    Graptostream,
    Phonorheos,
    Llm,
    /// The MCP server whose tools the model is offered.
    Tools,
}

/// What the window is told.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Event {
    /// Whether a service answers, and if not, why.
    Link {
        service: Service,
        ok: bool,
        detail: String,
    },
    /// How full the context window is.
    Context {
        tokens: Option<u32>,
        size: Option<u32>,
    },
    Heard {
        text: String,
    },
    /// The instant reply said while the model is asked.
    Holding {
        text: String,
    },
    /// A tool the model asked for, as it is called.
    Tool {
        text: String,
    },
    Replied {
        text: String,
    },
    Compacted {
        summary: String,
    },
    Error {
        text: String,
    },
}

pub type Emit = Arc<dyn Fn(Event) + Send + Sync>;

/// The conversation and the connections it needs, answering one utterance at a time.
pub struct Pipeline {
    c: reqwest::Client,
    settings: Settings,
    path: PathBuf,
    pub conversation: Conversation,
    /// The context window's size: the setting's, else the server's, else unknown.
    pub context: Option<u32>,
    /// Whether the last chat request failed, so that the health check, which only asks
    /// for the model list, does not show the model server as answering.
    pub chat_failed: Arc<AtomicBool>,
    /// The instant reply said last, which is not said next.
    last_instant: Option<usize>,
    /// The MCP session and its tools, opened when first needed and again after a failure.
    tools: Option<(mcp::Session, Vec<Value>)>,
    emit: Emit,
}

impl Pipeline {
    pub fn new(settings: Settings, path: PathBuf, emit: Emit) -> Self {
        let conversation = Conversation::load(&path);
        Self {
            c: http::client(),
            settings,
            path,
            conversation,
            context: None,
            chat_failed: Arc::default(),
            last_instant: None,
            tools: None,
            emit,
        }
    }

    /// Learns the context window's size and reports the model server's state.
    pub async fn connect(&mut self) {
        let s = &self.settings;
        match llm::models(&self.c, &s.llm).await {
            Ok(_) => self.link(Service::Llm, Ok(())),
            Err(e) => self.link(Service::Llm, Err(e)),
        }
        self.learn_context().await;
        self.report_context();
        self.prepare_instant_replies().await;
    }

    /// Asks phonorheos to make and keep the instant replies' speech, unheard.
    async fn prepare_instant_replies(&self) {
        for text in &self.settings.instant_replies {
            let r = phonorheos::speak(
                &self.c,
                &self.settings.phonorheos,
                text,
                &self.settings.voice,
                Say::PREPARE,
            )
            .await;
            if r.is_err() {
                self.link(Service::Phonorheos, r);
                return;
            }
        }
    }

    /// Says one instant reply, chosen at random but never the one said last.
    async fn say_instant_reply(&mut self) {
        let Some(i) = pick(
            self.settings.instant_replies.len(),
            self.last_instant,
            seed(),
        ) else {
            return;
        };
        self.last_instant = Some(i);
        let text = self.settings.instant_replies[i].clone();
        (self.emit)(Event::Holding { text: text.clone() });
        let r = phonorheos::speak(
            &self.c,
            &self.settings.phonorheos,
            &text,
            &self.settings.voice,
            Say::INSTANT,
        )
        .await;
        self.link(Service::Phonorheos, r);
    }

    /// The context window's size: the setting's, else the server's.
    async fn learn_context(&mut self) {
        let s = &self.settings;
        self.context = match s.context_size {
            Some(n) => Some(n),
            None => llm::context_size(&self.c, &s.llm, &s.model).await,
        };
    }

    pub async fn answer(&mut self, heard: &str) {
        (self.emit)(Event::Heard { text: heard.into() });
        self.say_instant_reply().await;
        self.conversation.messages.push(Message::new("user", heard));
        // Asked again until known, in case the server was not up when Peabody started.
        if self.context.is_none() {
            self.learn_context().await;
        }
        let mut compacted = false;
        let reply = loop {
            let req = self.conversation.request(&self.settings.system_prompt);
            match self.ask(&req).await {
                Ok(r) => break Ok(r),
                Err(llm::Error::ContextExceeded(_)) if !compacted => {
                    compacted = true;
                    if !self.compact().await {
                        break Err("the conversation does not fit the context window".to_string());
                    }
                }
                Err(e) => break Err(e.to_string()),
            }
        };
        let spoken = match reply {
            Ok(r) => {
                self.link(Service::Llm, Ok(()));
                self.conversation.tokens = r.total_tokens;
                let text = if r.content.is_empty() {
                    (self.emit)(Event::Error {
                        text: "The model returned no answer.".into(),
                    });
                    NO_ANSWER.to_string()
                } else {
                    r.content
                };
                self.conversation
                    .messages
                    .push(Message::new("assistant", &text));
                text
            }
            Err(e) => {
                // An unanswered question is not kept, so that the exchanges stay paired.
                self.conversation.messages.pop();
                self.link(Service::Llm, Err(e.clone()));
                (self.emit)(Event::Error {
                    text: format!("The model: {e}"),
                });
                NO_MODEL.to_string()
            }
        };
        (self.emit)(Event::Replied {
            text: spoken.clone(),
        });
        let said = phonorheos::speak(
            &self.c,
            &self.settings.phonorheos,
            &spoken,
            &self.settings.voice,
            Say::ANSWER,
        )
        .await;
        self.link(Service::Phonorheos, said);
        if let Some(size) = self.context
            && self.conversation.needs_compaction(size)
        {
            self.compact().await;
        }
        self.save();
        self.report_context();
    }

    /// Asks the model, offering the MCP server's tools where one is set, and calls the
    /// tools it asks for until it answers. The calls and their results go to the model
    /// alone: they are not kept in the conversation.
    async fn ask(&mut self, req: &[Message]) -> Result<llm::Reply, llm::Error> {
        let tools = self.offer_tools().await;
        let mut messages: Vec<Value> = req.iter().map(|m| json!(m)).collect();
        for round in 0..=MAX_TOOL_ROUNDS {
            let offer: &[Value] = if round < MAX_TOOL_ROUNDS { &tools } else { &[] };
            let r = llm::chat(
                &self.c,
                &self.settings.llm,
                &self.settings.model,
                self.settings.thinking,
                &messages,
                offer,
            )
            .await?;
            if r.tool_calls.is_empty() || offer.is_empty() {
                return Ok(r);
            }
            messages.push(
                json!({"role": "assistant", "content": r.content, "tool_calls": r.tool_calls}),
            );
            for call in &r.tool_calls {
                let output = self.call_tool(call).await;
                messages
                    .push(json!({"role": "tool", "tool_call_id": call["id"], "content": output}));
            }
        }
        unreachable!("the last round offers no tools, so it returns")
    }

    /// The tools to offer: none where no MCP server is set or it cannot be reached.
    async fn offer_tools(&mut self) -> Vec<Value> {
        if self.settings.tools.url.trim().is_empty() {
            return Vec::new();
        }
        if self.tools.is_none() {
            let opened = match mcp::Session::open(self.c.clone(), self.settings.tools.clone()).await
            {
                Ok(s) => s.tools().await.map(|t| (s, t)),
                Err(e) => Err(e),
            };
            match opened {
                Ok(st) => {
                    self.link(Service::Tools, Ok(()));
                    self.tools = Some(st);
                }
                Err(e) => {
                    self.link(Service::Tools, Err(e));
                    return Vec::new();
                }
            }
        }
        self.tools
            .as_ref()
            .map(|(_, t)| t.clone())
            .unwrap_or_default()
    }

    async fn call_tool(&mut self, call: &Value) -> String {
        let f = &call["function"];
        let name = f["name"].as_str().unwrap_or_default();
        // The arguments are a JSON string by the chat API's contract; some servers send
        // an object.
        let args = match &f["arguments"] {
            Value::String(s) => serde_json::from_str(s).unwrap_or_else(|_| json!({})),
            Value::Object(_) => f["arguments"].clone(),
            _ => json!({}),
        };
        (self.emit)(Event::Tool {
            text: describe_call(name, &args),
        });
        let Some((session, _)) = &self.tools else {
            return "The tool is not available.".into();
        };
        let (out, failed) = session.call(name, args).await;
        if failed {
            // The session may have lapsed; it is opened afresh for the next question.
            self.tools = None;
            self.link(Service::Tools, Err(out.clone()));
        }
        out
    }

    /// Summarises all but the last exchanges; whether it did.
    async fn compact(&mut self) -> bool {
        let Some(req) = self.conversation.compaction_request() else {
            return false;
        };
        match llm::chat(
            &self.c,
            &self.settings.llm,
            &self.settings.model,
            self.settings.thinking,
            &req,
            &[],
        )
        .await
        {
            Ok(r) if !r.content.is_empty() => {
                self.conversation.compact(r.content.clone());
                (self.emit)(Event::Compacted { summary: r.content });
                self.save();
                true
            }
            Ok(_) => {
                (self.emit)(Event::Error {
                    text: "The model returned an empty summary.".into(),
                });
                false
            }
            Err(e) => {
                (self.emit)(Event::Error {
                    text: format!("The summary could not be made: {e}"),
                });
                false
            }
        }
    }

    fn save(&self) {
        if let Err(e) = self.conversation.save(&self.path) {
            (self.emit)(Event::Error {
                text: format!("The conversation could not be saved: {e}"),
            });
        }
    }

    fn report_context(&self) {
        (self.emit)(Event::Context {
            tokens: self.conversation.tokens,
            size: self.context,
        });
    }

    fn link(&self, service: Service, r: Result<(), String>) {
        if service == Service::Llm {
            self.chat_failed.store(r.is_err(), Ordering::Relaxed);
        }
        (self.emit)(link_event(service, r));
    }
}

/// A tool call as the window shows it: `search: current time in California`.
fn describe_call(name: &str, args: &Value) -> String {
    let what = match args {
        Value::Object(m) if m.len() == 1 => m.values().next().map(|v| match v {
            Value::String(s) => s.clone(),
            v => v.to_string(),
        }),
        Value::Object(m) => m
            .get("query")
            .or_else(|| m.get("url"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| Some(args.to_string())),
        _ => None,
    };
    match what {
        Some(w) => format!("{name}: {w}"),
        None => name.to_string(),
    }
}

/// The index of the instant reply to say among `n`, never `last` when there is another
/// to choose; `seed` is any random number.
fn pick(n: usize, last: Option<usize>, seed: u64) -> Option<usize> {
    match (n, last) {
        (0, _) => None,
        (1, _) => Some(0),
        (n, Some(l)) if l < n => {
            let i = (seed % (n as u64 - 1)) as usize;
            Some(if i >= l { i + 1 } else { i })
        }
        (n, _) => Some((seed % n as u64) as usize),
    }
}

/// A random number from the standard library's randomly keyed hasher, which serves for
/// choosing a phrase without a dependency for it.
fn seed() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish()
}

fn link_event(service: Service, r: Result<(), String>) -> Event {
    match r {
        Ok(()) => Event::Link {
            service,
            ok: true,
            detail: String::new(),
        },
        Err(detail) => Event::Link {
            service,
            ok: false,
            detail,
        },
    }
}

/// How often things are done; shorter in the tests.
#[derive(Clone, Copy)]
pub struct Timing {
    /// Renewal of the five-minute listen request.
    pub renew: Duration,
    /// The check that phonorheos and the model server answer.
    pub health: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            renew: Duration::from_secs(240),
            health: Duration::from_secs(30),
        }
    }
}

/// Peabody at work; [`Running::stop`] ends it and withdraws the listen request.
pub struct Running {
    tasks: Vec<JoinHandle<()>>,
    listen: Listen,
}

impl Running {
    pub async fn stop(self) {
        for t in &self.tasks {
            t.abort();
        }
        self.listen.withdraw().await;
    }
}

pub fn start(settings: Settings, conversation: PathBuf, emit: Emit, timing: Timing) -> Running {
    let c = http::client();
    let listen = Listen::new(c.clone(), settings.graptostream.clone());
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let mut tasks = vec![tokio::spawn(listen.clone().hold(5.0, timing.renew))];

    let e = emit.clone();
    tasks.push(tokio::spawn(graptostream::read_utterances(
        c.clone(),
        settings.graptostream.clone(),
        settings.name.clone(),
        tx,
        move |l| {
            e(link_event(
                Service::Graptostream,
                match l {
                    Link::Connected => Ok(()),
                    Link::Down(d) => Err(d),
                },
            ))
        },
    )));

    let mut pipeline = Pipeline::new(settings.clone(), conversation, emit.clone());
    let chat_failed = pipeline.chat_failed.clone();
    tasks.push(tokio::spawn(async move {
        pipeline.connect().await;
        while let Some(heard) = rx.recv().await {
            pipeline.answer(&heard).await;
        }
    }));

    tasks.push(tokio::spawn(async move {
        loop {
            emit(link_event(
                Service::Phonorheos,
                phonorheos::health(&c, &settings.phonorheos).await,
            ));
            // A server that lists its models may still refuse the chat; only a failure
            // of the list, or a list after a chat that succeeded, is reported.
            let listed = llm::models(&c, &settings.llm).await.map(|_| ());
            if listed.is_err() || !chat_failed.load(Ordering::Relaxed) {
                emit(link_event(Service::Llm, listed));
            }
            tokio::time::sleep(timing.health).await;
        }
    }));
    Running { tasks, listen }
}

#[cfg(test)]
mod tests {
    use super::pick;

    #[test]
    fn an_instant_reply_is_never_the_one_said_last() {
        assert_eq!(pick(0, None, 7), None);
        assert_eq!(
            pick(1, Some(0), 7),
            Some(0),
            "with one phrase it is said each time"
        );
        for seed in 0..100 {
            for last in 0..4 {
                let i = pick(4, Some(last), seed).unwrap();
                assert!(i < 4 && i != last);
            }
            assert!(pick(4, None, seed).unwrap() < 4);
        }
        // Every other phrase can be chosen.
        let chosen: std::collections::BTreeSet<_> =
            (0..100).map(|s| pick(4, Some(2), s).unwrap()).collect();
        assert_eq!(chosen.into_iter().collect::<Vec<_>>(), [0, 1, 3]);
    }
}
