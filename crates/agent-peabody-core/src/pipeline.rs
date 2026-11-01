// SPDX-FileCopyrightText: 2026 Gary Frattarola <garyf@parkviewlab.ai>
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What Peabody does: each utterance heard through graptostream goes into the
//! conversation, to the model, and its answer out through phonorheos, one at a time and
//! in the order heard. Where MCP servers are set, their tools (a web search, say) are
//! listed in the model's instructions and offered in the request, and Peabody calls the
//! ones the model asks for before it answers.

use crate::conversation::{Conversation, KEEP_EXCHANGES};
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
/// The longest tool result kept in the conversation with the answer it served, in
/// characters: a time, a weather report or a calculation, but not search results or a
/// fetched page, which would fill a small context window.
pub const KEEP_TOOL_RESULT: usize = 500;
/// Spoken when the model cannot be reached or refuses the request.
pub const NO_MODEL: &str = "I could not get an answer from the model.";

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Service {
    Graptostream,
    Phonorheos,
    Llm,
    /// The MCP servers whose tools the model is offered.
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
    /// A request to the model, as sent: a one-line summary and the whole of it.
    #[serde(rename = "to_model")]
    ToModel {
        summary: String,
        text: String,
    },
    /// The model's reply, as received, or why there was none.
    #[serde(rename = "from_model")]
    FromModel {
        summary: String,
        text: String,
    },
    Replied {
        text: String,
    },
    Compacted {
        summary: String,
    },
    /// The conversation was cleared from the window.
    Cleared,
    /// The owner said "<name> hush".
    Hushed {
        text: String,
    },
    /// Something the owner asked for that needs saying but is not a problem.
    Notice {
        text: String,
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
    /// A session for each MCP server, in the settings' order, opened when first needed
    /// and again after a failure.
    servers: Vec<Option<mcp::Session>>,
    /// The tools offered for the question in hand.
    offered: Vec<Offered>,
    /// What phonorheos was given for the question in hand: each item's id, and whether it
    /// is the answer (rather than the instant reply), so that a hush can withdraw them.
    spoken: Vec<(String, bool)>,
    /// The short tool results of the question in hand, kept with its answer.
    kept: Vec<String>,
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
            servers: Vec::new(),
            offered: Vec::new(),
            spoken: Vec::new(),
            kept: Vec::new(),
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
        // Found again with each question; looked for now so that the window shows whether
        // the servers answer.
        self.offer_tools().await;
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
            if let Err(e) = r {
                self.link(Service::Phonorheos, Err(e));
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
        if let Ok(Some(id)) = &r {
            self.spoken.push((id.clone(), false));
        }
        self.link(Service::Phonorheos, r.map(|_| ()));
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
        // Speech from an earlier answer has been spoken, or hushed, by now.
        self.spoken.clear();
        (self.emit)(Event::Heard { text: heard.into() });
        self.say_instant_reply().await;
        self.conversation.messages.push(Message::new("user", heard));
        // Asked again until known, in case the server was not up when Peabody started.
        if self.context.is_none() {
            self.learn_context().await;
        }
        self.offer_tools().await;
        let prompt = with_tools(&self.settings.system_prompt, &self.offered);
        let mut compacted = false;
        let reply = loop {
            let req = self.conversation.request(&prompt);
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
                // Short tool results stay with the answer, so that a follow-up ("what day
                // is it there?") can draw on what the tool said and the answer left out.
                let kept: String = std::mem::take(&mut self.kept)
                    .iter()
                    .map(|k| format!("\n\n[{k}]"))
                    .collect();
                self.conversation
                    .messages
                    .push(Message::new("assistant", &format!("{text}{kept}")));
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
        if let Ok(Some(id)) = &said {
            self.spoken.push((id.clone(), true));
        }
        self.link(Service::Phonorheos, said.map(|_| ()));
        if let Some(size) = self.context
            && self.conversation.needs_compaction(size)
        {
            self.compact().await;
        }
        self.save();
        self.report_context();
    }

    /// Asks the model, offering the tools found for this question, and calls the tools it
    /// asks for until it answers. The calls and their results go to the model alone: they
    /// are not kept in the conversation.
    async fn ask(&mut self, req: &[Message]) -> Result<llm::Reply, llm::Error> {
        let tools: Vec<Value> = self.offered.iter().map(Offered::as_chat_tool).collect();
        let mut messages: Vec<Value> = req.iter().map(|m| json!(m)).collect();
        self.kept.clear();
        for round in 0..=MAX_TOOL_ROUNDS {
            let offer: &[Value] = if round < MAX_TOOL_ROUNDS { &tools } else { &[] };
            let r = self.chat(&messages, offer, "").await?;
            if r.tool_calls.is_empty() || offer.is_empty() {
                return Ok(r);
            }
            messages.push(
                json!({"role": "assistant", "content": r.content, "tool_calls": r.tool_calls}),
            );
            for call in &r.tool_calls {
                let output = self.call_tool(call).await;
                if output.chars().count() <= KEEP_TOOL_RESULT && !output.starts_with("The tool") {
                    let name = call["function"]["name"].as_str().unwrap_or("tool");
                    self.kept.push(format!("{name} said: {output}"));
                }
                messages
                    .push(json!({"role": "tool", "tool_call_id": call["id"], "content": output}));
            }
        }
        unreachable!("the last round offers no tools, so it returns")
    }

    /// One request to the model, shown in the window as sent and as answered.
    async fn chat(
        &self,
        messages: &[Value],
        tools: &[Value],
        purpose: &str,
    ) -> Result<llm::Reply, llm::Error> {
        (self.emit)(Event::ToModel {
            summary: format!(
                "{} message{}{}{purpose}",
                messages.len(),
                if messages.len() == 1 { "" } else { "s" },
                match tools.len() {
                    0 => String::new(),
                    n => format!(", {n} tool{}", if n == 1 { "" } else { "s" }),
                }
            ),
            text: show_request(messages, tools, &self.settings),
        });
        let r = llm::chat(
            &self.c,
            &self.settings.llm,
            &self.settings.model,
            self.settings.thinking,
            &messages,
            tools,
        )
        .await;
        (self.emit)(match &r {
            Ok(reply) => Event::FromModel {
                summary: reply_summary(reply),
                text: show_reply(reply),
            },
            Err(e) => Event::FromModel {
                summary: "failed".into(),
                text: e.to_string(),
            },
        });
        r
    }

    /// Finds the tools of every MCP server set, opening a session where none is open, and
    /// reports the servers that cannot be reached; those that can are used.
    async fn offer_tools(&mut self) {
        self.offered.clear();
        let eps = self.settings.tools.clone();
        self.servers.resize_with(eps.len(), || None);
        let mut failures = Vec::new();
        for (i, ep) in eps.iter().enumerate() {
            if ep.url.trim().is_empty() {
                continue;
            }
            if self.servers[i].is_none() {
                match mcp::Session::open(ep.clone()).await {
                    Ok(s) => self.servers[i] = Some(s),
                    Err(e) => {
                        failures.push(format!("{}: {e}", ep.url));
                        continue;
                    }
                }
            }
            let session = self.servers[i].as_ref().expect("opened above");
            match session.tools().await {
                Ok(tools) => {
                    for t in tools {
                        let f = &t["function"];
                        let tool = f["name"].as_str().unwrap_or_default().to_string();
                        let name = if self.offered.iter().any(|o| o.name == tool) {
                            format!("{}_{tool}", identifier(&session.name))
                        } else {
                            tool.clone()
                        };
                        self.offered.push(Offered {
                            name,
                            tool,
                            server: i,
                            description: f["description"].as_str().unwrap_or_default().into(),
                            parameters: f["parameters"].clone(),
                        });
                    }
                }
                Err(e) => {
                    failures.push(format!("{}: {e}", session.name));
                    self.servers[i] = None;
                }
            }
        }
        if eps.iter().any(|e| !e.url.trim().is_empty()) {
            self.link(
                Service::Tools,
                if failures.is_empty() {
                    Ok(())
                } else {
                    Err(failures.join("; "))
                },
            );
        }
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
        let Some(o) = self.offered.iter().find(|o| o.name == name) else {
            return format!("There is no tool named {name}.");
        };
        let (server, tool) = (o.server, o.tool.clone());
        let Some(session) = self.servers.get(server).and_then(Option::as_ref) else {
            return "The tool is not available.".into();
        };
        let (out, failed) = session.call(&tool, args).await;
        if failed {
            // The session may have lapsed; it is opened afresh for the next question.
            let server_name = session.name.clone();
            self.servers[server] = None;
            self.link(Service::Tools, Err(format!("{server_name}: {out}")));
        }
        out
    }

    /// Empties the conversation, at the owner's word from the window.
    pub fn clear(&mut self) {
        self.conversation = Conversation::default();
        self.save();
        (self.emit)(Event::Cleared);
        self.report_context();
    }

    /// Summarises all but the last exchanges now, at the owner's word from the window.
    pub async fn compact_now(&mut self) {
        if self.conversation.compaction_request().is_none() {
            (self.emit)(Event::Notice {
                text: format!(
                    "There is nothing to summarise yet: the last {KEEP_EXCHANGES} exchanges are always kept word for word."
                ),
            });
            return;
        }
        if self.compact().await {
            self.report_context();
        }
    }

    /// Drops the question in hand after its answer was interrupted: an unanswered question
    /// is not kept, so that the exchanges stay paired.
    pub fn abandon(&mut self) {
        if self
            .conversation
            .messages
            .last()
            .is_some_and(|m| m.role == "user")
        {
            self.conversation.messages.pop();
        }
        (self.emit)(Event::Notice {
            text: "The question in hand was abandoned.".into(),
        });
    }

    pub async fn run(&mut self, c: Command) {
        match c {
            Command::Clear => self.clear(),
            Command::Compact => self.compact_now().await,
            Command::Hush => self.hush().await,
        }
    }

    /// Stops Peabody's speech at the owner's "<name> hush": its items are withdrawn from
    /// phonorheos (other apps' speech is untouched), and an answer cut off mid-speech is
    /// marked in the conversation, so that the model does not take it as heard.
    pub async fn hush(&mut self) {
        let mut cut = false;
        for (id, is_answer) in std::mem::take(&mut self.spoken) {
            match phonorheos::withdraw(&self.c, &self.settings.phonorheos, &id).await {
                Ok(true) if is_answer => cut = true,
                Ok(_) => {}
                Err(e) => self.link(Service::Phonorheos, Err(e)),
            }
        }
        if cut {
            if let Some(m) = self.conversation.messages.last_mut()
                && m.role == "assistant"
            {
                m.content.push_str(" (interrupted)");
            }
            self.save();
        }
        (self.emit)(Event::Hushed {
            text: if cut {
                "Hushed; the answer was cut off.".into()
            } else {
                "Hushed.".into()
            },
        });
    }

    /// Summarises all but the last exchanges; whether it did.
    async fn compact(&mut self) -> bool {
        let Some(req) = self.conversation.compaction_request() else {
            return false;
        };
        let messages: Vec<Value> = req.iter().map(|m| json!(m)).collect();
        match self.chat(&messages, &[], " for a summary").await {
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

/// A request as the window shows it opened: each message by role, then what was offered.
fn show_request(messages: &[Value], tools: &[Value], s: &Settings) -> String {
    let mut out = Vec::new();
    for m in messages {
        let role = m["role"].as_str().unwrap_or("?");
        let content = m["content"].as_str().unwrap_or_default();
        let mut block = match role {
            "tool" => format!("[tool result {}]", m["tool_call_id"].as_str().unwrap_or("")),
            r => format!("[{r}]"),
        };
        if !content.is_empty() {
            block.push('\n');
            block.push_str(content);
        }
        for c in m["tool_calls"].as_array().into_iter().flatten() {
            block.push_str(&format!("\nasks for {}", show_call(c)));
        }
        out.push(block);
    }
    let names: Vec<&str> = tools
        .iter()
        .filter_map(|t| t["function"]["name"].as_str())
        .collect();
    out.push(format!(
        "[sent with]\nmodel {}; thinking {}; tools: {}",
        if s.model.is_empty() {
            "(server's default)"
        } else {
            &s.model
        },
        if s.thinking { "on" } else { "off" },
        if names.is_empty() {
            "none".to_string()
        } else {
            names.join(", ")
        }
    ));
    out.join("\n\n")
}

fn show_call(c: &Value) -> String {
    let f = &c["function"];
    let args = match &f["arguments"] {
        Value::String(s) => s.clone(),
        v => v.to_string(),
    };
    format!("{}({args})", f["name"].as_str().unwrap_or("?"))
}

fn reply_summary(r: &llm::Reply) -> String {
    let what = if !r.tool_calls.is_empty() {
        let names: Vec<&str> = r
            .tool_calls
            .iter()
            .filter_map(|c| c["function"]["name"].as_str())
            .collect();
        format!("asks for {}", names.join(", "))
    } else if r.content.is_empty() {
        "no answer".to_string()
    } else {
        "answer".to_string()
    };
    match r.usage["total_tokens"].as_u64() {
        Some(t) => format!("{what}, {t} tokens"),
        None => what,
    }
}

/// A reply as the window shows it opened: reasoning, answer, tool calls, token counts.
fn show_reply(r: &llm::Reply) -> String {
    let mut out = Vec::new();
    if !r.reasoning.is_empty() {
        out.push(format!("[reasoning]\n{}", r.reasoning));
    }
    if !r.content.is_empty() {
        out.push(format!("[answer]\n{}", r.content));
    }
    for c in &r.tool_calls {
        out.push(format!("[asks for]\n{}", show_call(c)));
    }
    let u = &r.usage;
    let n = |k: &str| u[k].as_u64().map(|v| v.to_string());
    let mut tokens = vec![];
    if let Some(v) = n("prompt_tokens") {
        tokens.push(format!("prompt {v}"));
    }
    if let Some(v) = n("completion_tokens") {
        tokens.push(format!("completion {v}"));
    }
    if let Some(v) = u["completion_tokens_details"]["reasoning_tokens"].as_u64() {
        tokens.push(format!("of which reasoning {v}"));
    }
    if let Some(v) = n("total_tokens") {
        tokens.push(format!("total {v}"));
    }
    if !tokens.is_empty() {
        out.push(format!("[tokens]\n{}", tokens.join(", ")));
    }
    if out.is_empty() {
        "(empty reply)".into()
    } else {
        out.join("\n\n")
    }
}

/// A tool offered to the model, and where it comes from.
struct Offered {
    /// The name the model sees: the server's own, or prefixed with the server's name
    /// where an earlier server offers a tool of the same name.
    name: String,
    /// The name the server knows it by.
    tool: String,
    server: usize,
    description: String,
    parameters: Value,
}

impl Offered {
    fn as_chat_tool(&self) -> Value {
        json!({"type": "function", "function": {
            "name": self.name, "description": self.description, "parameters": self.parameters}})
    }
}

/// The system prompt with the tools listed after it, as their servers describe them.
fn with_tools(system_prompt: &str, tools: &[Offered]) -> String {
    if tools.is_empty() {
        return system_prompt.to_string();
    }
    let list: Vec<String> = tools
        .iter()
        .map(|t| format!("- {}: {}", t.name, t.description))
        .collect();
    let list = format!("Tools available to you:\n{}", list.join("\n"));
    if system_prompt.trim().is_empty() {
        list
    } else {
        format!("{system_prompt}\n\n{list}")
    }
}

/// A server's name made fit for a tool name: letters, digits and `_`.
fn identifier(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

/// What the window asks of a running Peabody.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Command {
    Clear,
    Compact,
    /// "<name> hush", heard by graptostream.
    Hush,
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
    commands: mpsc::UnboundedSender<Command>,
}

impl Running {
    /// Passes a command from the window to the pipeline, which runs it between answers.
    pub fn command(&self, c: Command) {
        let _ = self.commands.send(c);
    }

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
    let (commands, mut cmds) = mpsc::unbounded_channel::<Command>();
    let mut tasks = Vec::new();

    // Switched off, Peabody holds no listen request and reads nothing from graptostream.
    if settings.enabled {
        tasks.push(tokio::spawn(listen.clone().hold(5.0, timing.renew)));
        let e = emit.clone();
        let hushes = commands.clone();
        tasks.push(tokio::spawn(graptostream::read_utterances(
            c.clone(),
            settings.graptostream.clone(),
            settings.name.clone(),
            tx,
            move || {
                let _ = hushes.send(Command::Hush);
            },
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
    } else {
        drop(tx);
    }

    let mut pipeline = Pipeline::new(settings.clone(), conversation, emit.clone());
    let chat_failed = pipeline.chat_failed.clone();
    tasks.push(tokio::spawn(async move {
        pipeline.connect().await;
        loop {
            tokio::select! {
                Some(heard) = rx.recv() => {
                    // A command from the window does not wait for the answer: the question
                    // in hand is abandoned, so that Clear and Compact act at once even while
                    // the model server is slow or busy with another client.
                    let interrupted = {
                        let answer = pipeline.answer(&heard);
                        tokio::pin!(answer);
                        tokio::select! {
                            _ = &mut answer => None,
                            Some(c) = cmds.recv() => Some(c),
                        }
                    };
                    if let Some(c) = interrupted {
                        pipeline.abandon();
                        pipeline.run(c).await;
                    }
                }
                Some(c) = cmds.recv() => pipeline.run(c).await,
                else => break,
            }
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
    Running {
        tasks,
        listen,
        commands,
    }
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
