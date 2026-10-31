// SPDX-FileCopyrightText: 2026 Gary Frattarola <garyf@parkviewlab.ai>
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What Peabody does: each utterance heard through graptostream goes into the
//! conversation, to the model, and its answer out through phonorheos, one at a time and
//! in the order heard.

use crate::conversation::Conversation;
use crate::graptostream::{self, Link, Listen};
use crate::llm::{self, Message};
use crate::settings::Settings;
use crate::{http, phonorheos};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// Spoken when the model returns an empty answer.
pub const NO_ANSWER: &str = "I have no answer to that.";
/// Spoken when the model cannot be reached or refuses the request.
pub const NO_MODEL: &str = "I could not get an answer from the model.";

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Service {
    Graptostream,
    Phonorheos,
    Llm,
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
        self.context = match s.context_size {
            Some(n) => Some(n),
            None => llm::context_size(&self.c, &s.llm, &s.model).await,
        };
        self.report_context();
    }

    pub async fn answer(&mut self, heard: &str) {
        (self.emit)(Event::Heard { text: heard.into() });
        self.conversation.messages.push(Message::new("user", heard));
        let mut compacted = false;
        let reply = loop {
            let req = self.conversation.request(&self.settings.system_prompt);
            match llm::chat(&self.c, &self.settings.llm, &self.settings.model, &req).await {
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

    /// Summarises all but the last exchanges; whether it did.
    async fn compact(&mut self) -> bool {
        let Some(req) = self.conversation.compaction_request() else {
            return false;
        };
        match llm::chat(&self.c, &self.settings.llm, &self.settings.model, &req).await {
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
        (self.emit)(link_event(service, r));
    }
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
            emit(link_event(
                Service::Llm,
                llm::models(&c, &settings.llm).await.map(|_| ()),
            ));
            tokio::time::sleep(timing.health).await;
        }
    }));
    Running { tasks, listen }
}
