// SPDX-FileCopyrightText: 2026 Gary Frattarola <garyf@parkviewlab.ai>
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Peabody against small stand-ins for graptostream, the model server and phonorheos.

use agent_peabody_core::graptostream::{self, Link, Listen};
use agent_peabody_core::pipeline::{
    self, Emit, Event, MAX_TOOL_ROUNDS, NO_ANSWER, NO_MODEL, Pipeline, Timing,
};
use agent_peabody_core::settings::{Endpoint, Settings};
use axum::Router;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post, put};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;

async fn serve(app: Router) -> Endpoint {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    Endpoint::new(&url)
}

fn sse(body: String) -> impl IntoResponse {
    ([("content-type", "text/event-stream")], body)
}

fn segment(id: u32, text: &str, to: &str) -> String {
    let data = json!({"id": id, "state": "final", "text": text, "to": to});
    format!("id: {id}\nevent: segment\ndata: {data}\n\n")
}

/// A graptostream whose stream, on each connection, sends the next prepared body and ends;
/// it keeps each connection's `Last-Event-ID` and query.
/// A connection's `Last-Event-ID` and query.
type Seen = (Option<String>, String);

#[derive(Clone, Default)]
struct Grapto {
    bodies: Arc<Mutex<Vec<String>>>,
    seen: Arc<Mutex<Vec<Seen>>>,
    listens: Arc<Mutex<Vec<String>>>,
}

async fn grapto_events(
    State(g): State<Grapto>,
    h: HeaderMap,
    uri: axum::http::Uri,
) -> impl IntoResponse {
    let last = h
        .get("last-event-id")
        .map(|v| v.to_str().unwrap().to_string());
    g.seen
        .lock()
        .unwrap()
        .push((last, uri.query().unwrap_or("").to_string()));
    let mut b = g.bodies.lock().unwrap();
    sse(if b.is_empty() {
        String::new()
    } else {
        b.remove(0)
    })
}

fn grapto_app(g: Grapto) -> Router {
    Router::new()
        .route("/events", get(grapto_events))
        .route(
            "/listen",
            post(|State(g): State<Grapto>| async move {
                g.listens.lock().unwrap().push("POST".into());
                (
                    StatusCode::CREATED,
                    axum::Json(json!({"id": "r1", "client": "peabody"})),
                )
            }),
        )
        .route(
            "/listen/{id}",
            put(
                |State(g): State<Grapto>, Path(id): Path<String>| async move {
                    let mut l = g.listens.lock().unwrap();
                    l.push(format!("PUT {id}"));
                    // The first renewal finds the request lapsed.
                    if l.iter().filter(|s| s.starts_with("PUT")).count() == 1 {
                        StatusCode::NOT_FOUND
                    } else {
                        StatusCode::OK
                    }
                },
            )
            .delete(
                |State(g): State<Grapto>, Path(id): Path<String>| async move {
                    g.listens.lock().unwrap().push(format!("DELETE {id}"));
                    StatusCode::NO_CONTENT
                },
            ),
        )
        .with_state(g)
}

/// A model server whose replies are set in advance; it keeps every request.
#[derive(Clone, Default)]
struct Model {
    replies: Arc<Mutex<Vec<(StatusCode, Value)>>>,
    requests: Arc<Mutex<Vec<Value>>>,
    /// How long each answer takes, as a slow or busy server's would.
    delay: Arc<Mutex<Duration>>,
}

fn reply(content: &str, total: u32) -> (StatusCode, Value) {
    (
        StatusCode::OK,
        json!({"choices": [{"message": {"content": content, "reasoning_content": "thinking"}}],
                           "usage": {"prompt_tokens": total - 5, "completion_tokens": 5, "total_tokens": total}}),
    )
}

fn model_app(m: Model) -> Router {
    Router::new()
        .route(
            "/v1/chat/completions",
            post(
                |State(m): State<Model>, axum::Json(body): axum::Json<Value>| async move {
                    m.requests.lock().unwrap().push(body);
                    let delay = *m.delay.lock().unwrap();
                    tokio::time::sleep(delay).await;
                    let mut r = m.replies.lock().unwrap();
                    let (s, v) = if r.is_empty() {
                        reply("Fine.", 100)
                    } else {
                        r.remove(0)
                    };
                    (s, axum::Json(v))
                },
            ),
        )
        .route(
            "/v1/models",
            get(|| async { axum::Json(json!({"data": [{"id": "m"}]})) }),
        )
        .route(
            "/props",
            get(|| async { axum::Json(json!({"default_generation_settings": {"n_ctx": 1000}})) }),
        )
        .with_state(m)
}

/// A phonorheos that keeps what it is asked to say.
#[derive(Clone, Default)]
struct Voice {
    said: Arc<Mutex<Vec<Value>>>,
    /// The ids withdrawn, and which of them were still there (not yet finished).
    withdrawn: Arc<Mutex<Vec<String>>>,
    finished: Arc<Mutex<Vec<String>>>,
}

fn voice_app(v: Voice) -> Router {
    Router::new()
        .route(
            "/speak",
            post(
                |State(v): State<Voice>, axum::Json(body): axum::Json<Value>| async move {
                    let mut said = v.said.lock().unwrap();
                    said.push(body);
                    let id = format!("i{}", said.len());
                    (
                        StatusCode::CREATED,
                        axum::Json(json!({"id": id, "event": 1, "ahead": 0})),
                    )
                },
            ),
        )
        .route(
            "/speak/{id}",
            axum::routing::delete(
                |State(v): State<Voice>, Path(id): Path<String>| async move {
                    v.withdrawn.lock().unwrap().push(id.clone());
                    if v.finished.lock().unwrap().contains(&id) {
                        StatusCode::NOT_FOUND
                    } else {
                        StatusCode::NO_CONTENT
                    }
                },
            ),
        )
        .route("/health", get(|| async { "ok" }))
        .with_state(v)
}

fn collect() -> (Emit, Arc<Mutex<Vec<Event>>>) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let e = events.clone();
    (Arc::new(move |ev| e.lock().unwrap().push(ev)), events)
}

async fn settings(model: &Model, voice: &Voice) -> Settings {
    Settings {
        llm: serve(model_app(model.clone())).await,
        phonorheos: serve(voice_app(voice.clone())).await,
        model: "m".into(),
        voice: "bf_emma".into(),
        // Off unless a test is about them, so that what is said is only the answers.
        instant_replies: Vec::new(),
        ..Settings::default()
    }
}

async fn wait_for(mut done: impl FnMut() -> bool) {
    for _ in 0..200 {
        if done() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("timed out");
}

#[tokio::test]
async fn hears_only_its_own_name_and_resumes_after_a_reconnection() {
    let g = Grapto::default();
    g.bodies.lock().unwrap().extend([
        segment(1, "what is two plus two?", "peabody") + &segment(2, "hello", "claude"),
        segment(3, "and three?", "Peabody"),
    ]);
    let ep = serve(grapto_app(g.clone())).await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let links = Arc::new(Mutex::new(Vec::new()));
    let l = links.clone();
    let task = tokio::spawn(graptostream::read_utterances(
        agent_peabody_core::http::client(),
        ep,
        "peabody".into(),
        tx,
        || {},
        move |s| l.lock().unwrap().push(s),
    ));
    assert_eq!(rx.recv().await.unwrap(), "what is two plus two?");
    assert_eq!(rx.recv().await.unwrap(), "and three?");
    task.abort();
    let seen = g.seen.lock().unwrap();
    assert_eq!(seen[0].0, None);
    assert_eq!(seen[1].0.as_deref(), Some("2"));
    assert!(seen[0].1.contains("to=peabody") && seen[0].1.contains("final=true"));
    assert_eq!(links.lock().unwrap()[0], Link::Connected);
}

#[tokio::test]
async fn holds_a_listen_request_and_asks_afresh_when_it_lapses() {
    let g = Grapto::default();
    let ep = serve(grapto_app(g.clone())).await;
    let listen = Listen::new(agent_peabody_core::http::client(), ep);
    let task = tokio::spawn(listen.clone().hold(5.0, Duration::from_millis(30)));
    wait_for(|| g.listens.lock().unwrap().len() >= 4).await;
    task.abort();
    listen.withdraw().await;
    let l = g.listens.lock().unwrap();
    assert_eq!(&l[..4], ["POST", "PUT r1", "POST", "PUT r1"]);
    assert_eq!(l.last().unwrap(), "DELETE r1");
}

#[tokio::test]
async fn answers_in_its_voice_and_keeps_the_conversation() {
    let (model, voice) = (Model::default(), Voice::default());
    model
        .replies
        .lock()
        .unwrap()
        .extend([reply("Four.", 120), reply("Seven.", 150)]);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("conversation.json");
    let (emit, events) = collect();
    let mut p = Pipeline::new(settings(&model, &voice).await, path.clone(), emit);
    p.connect().await;
    assert_eq!(p.context, Some(1000));
    p.answer("what is two plus two?").await;
    p.answer("and three more?").await;
    let said = voice.said.lock().unwrap();
    assert_eq!(
        said[0],
        json!({"text": "Four.", "client": "peabody", "voice": "bf_emma", "speak": true, "cache": false})
    );
    assert_eq!(said[1]["text"], "Seven.");
    // The second request carries the first exchange, and no reasoning.
    let second = &model.requests.lock().unwrap()[1];
    assert_eq!(second["model"], "m");
    let texts: Vec<&str> = second["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["content"].as_str().unwrap())
        .collect();
    assert_eq!(
        texts[1..],
        ["what is two plus two?", "Four.", "and three more?"]
    );
    assert!(events.lock().unwrap().contains(&Event::Context {
        tokens: Some(150),
        size: Some(1000)
    }));
    // And it survives a restart.
    let (emit, _) = collect();
    assert_eq!(
        Pipeline::new(Settings::default(), path, emit)
            .conversation
            .messages
            .len(),
        4
    );
}

#[tokio::test]
async fn compacts_at_three_quarters_keeping_the_last_four_exchanges() {
    let (model, voice) = (Model::default(), Voice::default());
    let mut replies: Vec<_> = (0..4).map(|i| reply(&format!("a{i}"), 100 + i)).collect();
    replies.push(reply("a4", 750));
    replies.push(reply("They spoke of q0.", 50));
    model.replies.lock().unwrap().extend(replies);
    let dir = tempfile::tempdir().unwrap();
    let (emit, events) = collect();
    let mut p = Pipeline::new(
        settings(&model, &voice).await,
        dir.path().join("c.json"),
        emit,
    );
    p.connect().await;
    for i in 0..5 {
        p.answer(&format!("q{i}")).await;
    }
    assert_eq!(p.conversation.summary.as_deref(), Some("They spoke of q0."));
    assert_eq!(p.conversation.messages.len(), 8);
    assert_eq!(p.conversation.messages[0].content, "q1");
    assert!(
        events
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(e, Event::Compacted { .. }))
    );
    assert_eq!(voice.said.lock().unwrap().len(), 5);
}

#[tokio::test]
async fn compacts_and_retries_once_when_the_window_is_exceeded() {
    let (model, voice) = (Model::default(), Voice::default());
    let mut replies: Vec<_> = (0..5).map(|i| reply(&format!("a{i}"), 100)).collect();
    replies.push((
        StatusCode::BAD_REQUEST,
        json!({"error": {"message": "the request exceeds the available context size"}}),
    ));
    replies.push(reply("Summary.", 40));
    replies.push(reply("a5", 300));
    model.replies.lock().unwrap().extend(replies);
    let dir = tempfile::tempdir().unwrap();
    let (emit, _) = collect();
    let mut s = settings(&model, &voice).await;
    s.context_size = Some(100_000);
    let mut p = Pipeline::new(s, dir.path().join("c.json"), emit);
    p.connect().await;
    for i in 0..6 {
        p.answer(&format!("q{i}")).await;
    }
    assert_eq!(voice.said.lock().unwrap().last().unwrap()["text"], "a5");
    assert_eq!(p.conversation.summary.as_deref(), Some("Summary."));
}

#[tokio::test]
async fn says_so_when_the_model_gives_nothing_or_cannot_be_reached() {
    let (model, voice) = (Model::default(), Voice::default());
    model.replies.lock().unwrap().push(reply("", 30));
    let dir = tempfile::tempdir().unwrap();
    let (emit, _) = collect();
    let mut s = settings(&model, &voice).await;
    let mut p = Pipeline::new(s.clone(), dir.path().join("c.json"), emit.clone());
    p.answer("hmm?").await;
    assert_eq!(voice.said.lock().unwrap()[0]["text"], NO_ANSWER);
    s.llm = Endpoint::new("http://127.0.0.1:9");
    let mut p = Pipeline::new(s, dir.path().join("d.json"), emit);
    p.answer("anyone there?").await;
    assert_eq!(voice.said.lock().unwrap()[1]["text"], NO_MODEL);
    assert!(p.conversation.messages.is_empty());
}

#[tokio::test]
async fn runs_from_hearing_to_speaking() {
    let (g, model, voice) = (Grapto::default(), Model::default(), Voice::default());
    g.bodies
        .lock()
        .unwrap()
        .push(segment(1, "what is two plus two?", "peabody"));
    model.replies.lock().unwrap().push(reply("Four.", 60));
    let mut s = settings(&model, &voice).await;
    s.graptostream = serve(grapto_app(g.clone())).await;
    let dir = tempfile::tempdir().unwrap();
    let (emit, events) = collect();
    let running = pipeline::start(s, dir.path().join("c.json"), emit, Timing::default());
    wait_for(|| !voice.said.lock().unwrap().is_empty()).await;
    running.stop().await;
    assert_eq!(voice.said.lock().unwrap()[0]["text"], "Four.");
    let ev = events.lock().unwrap();
    assert!(ev.contains(&Event::Heard {
        text: "what is two plus two?".into()
    }));
    assert!(g.listens.lock().unwrap().contains(&"DELETE r1".to_string()));
}

#[tokio::test]
async fn prepares_its_instant_replies_and_says_one_before_each_answer() {
    let (model, voice) = (Model::default(), Voice::default());
    model
        .replies
        .lock()
        .unwrap()
        .extend([reply("Four.", 60), reply("Seven.", 80)]);
    let dir = tempfile::tempdir().unwrap();
    let (emit, events) = collect();
    let s = Settings {
        instant_replies: vec!["Thinking...".into(), "One moment please...".into()],
        ..settings(&model, &voice).await
    };
    let mut p = Pipeline::new(s, dir.path().join("c.json"), emit);
    p.connect().await;
    p.answer("what is two plus two?").await;
    p.answer("and three more?").await;
    let said = voice.said.lock().unwrap();
    let prepare = |t: &str| json!({"text": t, "client": "peabody", "voice": "bf_emma", "speak": false, "cache": true});
    assert_eq!(said[0], prepare("Thinking..."));
    assert_eq!(said[1], prepare("One moment please..."));
    // Each answer is preceded by an instant reply, spoken and kept; never the same twice.
    let (first, second) = (&said[2], &said[4]);
    for instant in [first, second] {
        assert_eq!(
            (instant["speak"].as_bool(), instant["cache"].as_bool()),
            (Some(true), Some(true))
        );
    }
    assert_ne!(first["text"], second["text"]);
    assert_eq!(
        (said[3]["text"].as_str(), said[5]["text"].as_str()),
        (Some("Four."), Some("Seven."))
    );
    assert_eq!(said.len(), 6);
    // The instant reply is shown, but the model never sees it.
    assert!(events.lock().unwrap().contains(&Event::Holding {
        text: first["text"].as_str().unwrap().into()
    }));
    let texts: Vec<String> = p
        .conversation
        .messages
        .iter()
        .map(|m| m.content.clone())
        .collect();
    assert_eq!(
        texts,
        [
            "what is two plus two?",
            "Four.",
            "and three more?",
            "Seven."
        ]
    );
}

#[tokio::test]
async fn asks_the_model_not_to_think_unless_the_setting_allows_it() {
    let (model, voice) = (Model::default(), Voice::default());
    model
        .replies
        .lock()
        .unwrap()
        .extend([reply("Four.", 60), reply("Seven.", 80)]);
    let dir = tempfile::tempdir().unwrap();
    let (emit, _) = collect();
    let s = settings(&model, &voice).await;
    assert!(!s.thinking, "thinking is off by default");
    let mut p = Pipeline::new(s.clone(), dir.path().join("c.json"), emit.clone());
    p.answer("what is two plus two?").await;
    let thinking = Settings {
        thinking: true,
        ..s
    };
    let mut p = Pipeline::new(thinking, dir.path().join("d.json"), emit);
    p.answer("and three more?").await;
    let requests = model.requests.lock().unwrap();
    assert_eq!(requests[0]["reasoning_effort"], "none");
    assert!(requests[1].get("reasoning_effort").is_none());
}

/// A model reply asking for one tool call.
fn tool_call(name: &str, arguments: Value) -> (StatusCode, Value) {
    (
        StatusCode::OK,
        json!({"choices": [{"finish_reason": "tool_calls", "message": {"content": "",
            "tool_calls": [{"type": "function", "id": "c1",
                "function": {"name": name, "arguments": arguments.to_string()}}]}}],
            "usage": {"total_tokens": 90}}),
    )
}

/// An MCP server with one tool, `search`, answering as an event stream after a session
/// is opened; it keeps the calls.
#[derive(Clone, Default)]
struct Tools {
    calls: Arc<Mutex<Vec<Value>>>,
    /// What a tool call returns, where not the default sentence.
    result: Arc<Mutex<Option<String>>>,
}

async fn tools_server(t: &Tools, name: &'static str) -> Endpoint {
    Endpoint::new(&format!(
        "{}/mcp",
        serve(tools_app(t.clone(), name)).await.url
    ))
}

fn tools_app(t: Tools, name: &'static str) -> Router {
    Router::new()
        .route(
            "/mcp",
            post(
                move |State(t): State<Tools>, headers: HeaderMap, axum::Json(body): axum::Json<Value>| async move {
                    let id = body["id"].clone();
                    let result = match body["method"].as_str() {
                        Some("initialize") => json!({"protocolVersion": "2025-06-18", "capabilities": {"tools": {}},
                                                     "serverInfo": {"name": name, "version": "0"}}),
                        Some("tools/list") => {
                            assert_eq!(headers.get("mcp-session-id").unwrap(), "s1");
                            json!({"tools": [{"name": "search", "description": "Web search",
                                "inputSchema": {"type": "object", "properties": {"query": {"type": "string"}}}}]})
                        }
                        Some("tools/call") => {
                            t.calls.lock().unwrap().push(json!({"server": name, "params": body["params"]}));
                            let text = t.result.lock().unwrap().clone()
                                .unwrap_or_else(|| "It is 1:20 pm in California.".into());
                            json!({"content": [{"type": "text", "text": text}]})
                        }
                        _ => return (StatusCode::ACCEPTED, [("mcp-session-id", "s1")], String::new()),
                    };
                    let msg = json!({"jsonrpc": "2.0", "id": id, "result": result});
                    (StatusCode::OK, [("mcp-session-id", "s1")], format!("event: message\ndata: {msg}\n\n"))
                },
            ),
        )
        .with_state(t)
}

#[tokio::test]
async fn calls_the_tools_the_model_asks_for_and_keeps_only_the_answer() {
    let (model, voice, tools) = (Model::default(), Voice::default(), Tools::default());
    model.replies.lock().unwrap().extend([
        tool_call("search", json!({"query": "time in California"})),
        reply("It is twenty past one.", 120),
    ]);
    let dir = tempfile::tempdir().unwrap();
    let (emit, events) = collect();
    let mut s = settings(&model, &voice).await;
    s.tools = vec![tools_server(&tools, "stub").await];
    let mut p = Pipeline::new(s, dir.path().join("c.json"), emit);
    p.answer("what time is it in California?").await;
    assert_eq!(
        voice.said.lock().unwrap()[0]["text"],
        "It is twenty past one."
    );
    assert_eq!(
        tools.calls.lock().unwrap()[0]["params"],
        json!({"name": "search", "arguments": {"query": "time in California"}})
    );
    let requests = model.requests.lock().unwrap();
    assert_eq!(requests[0]["tools"][0]["function"]["name"], "search");
    // The tools are listed in the instructions, as the server describes them.
    let system = requests[0]["messages"][0]["content"].as_str().unwrap();
    assert!(
        system.ends_with("\n\nTools available to you:\n- search: Web search"),
        "{system}"
    );
    // The second request carries the call and its result.
    let second = requests[1]["messages"].as_array().unwrap();
    let last = second.last().unwrap();
    assert_eq!(
        (last["role"].as_str(), last["tool_call_id"].as_str()),
        (Some("tool"), Some("c1"))
    );
    assert_eq!(last["content"], "It is 1:20 pm in California.");
    assert_eq!(second[second.len() - 2]["tool_calls"][0]["id"], "c1");
    // The conversation keeps the question and the answer, with the tool's short result.
    let kept: Vec<&str> = p
        .conversation
        .messages
        .iter()
        .map(|m| m.content.as_str())
        .collect();
    assert_eq!(
        kept,
        [
            "what time is it in California?",
            "It is twenty past one.\n\n[search said: It is 1:20 pm in California.]"
        ]
    );
    assert!(events.lock().unwrap().contains(&Event::Tool {
        text: "search: time in California".into()
    }));
}

#[tokio::test]
async fn stops_offering_tools_after_the_last_round() {
    let (model, voice, tools) = (Model::default(), Voice::default(), Tools::default());
    let asks = (0..MAX_TOOL_ROUNDS).map(|_| tool_call("search", json!({"query": "again"})));
    model.replies.lock().unwrap().extend(asks);
    model.replies.lock().unwrap().push(reply("Enough.", 100));
    let dir = tempfile::tempdir().unwrap();
    let (emit, _) = collect();
    let mut s = settings(&model, &voice).await;
    s.tools = vec![tools_server(&tools, "stub").await];
    let mut p = Pipeline::new(s, dir.path().join("c.json"), emit);
    p.answer("loop?").await;
    let requests = model.requests.lock().unwrap();
    assert_eq!(requests.len(), MAX_TOOL_ROUNDS + 1);
    assert!(
        requests[MAX_TOOL_ROUNDS].get("tools").is_none(),
        "the last request offers none"
    );
    assert_eq!(tools.calls.lock().unwrap().len(), MAX_TOOL_ROUNDS);
    assert_eq!(voice.said.lock().unwrap()[0]["text"], "Enough.");
}

#[tokio::test]
async fn answers_without_tools_when_the_mcp_server_cannot_be_reached() {
    let (model, voice) = (Model::default(), Voice::default());
    model
        .replies
        .lock()
        .unwrap()
        .push(reply("Without tools.", 50));
    let dir = tempfile::tempdir().unwrap();
    let (emit, events) = collect();
    let mut s = settings(&model, &voice).await;
    s.tools = vec![Endpoint::new("http://127.0.0.1:9/mcp")];
    let mut p = Pipeline::new(s, dir.path().join("c.json"), emit);
    p.answer("hello?").await;
    assert!(model.requests.lock().unwrap()[0].get("tools").is_none());
    assert_eq!(voice.said.lock().unwrap()[0]["text"], "Without tools.");
    assert!(events.lock().unwrap().iter().any(|e| matches!(
        e,
        Event::Link {
            service: pipeline::Service::Tools,
            ok: false,
            ..
        }
    )));
}

#[tokio::test]
async fn offers_the_tools_of_every_server_and_tells_same_named_ones_apart() {
    let (model, voice) = (Model::default(), Voice::default());
    let (first, second) = (Tools::default(), Tools::default());
    model.replies.lock().unwrap().extend([
        tool_call("other_search", json!({"query": "x"})),
        reply("Done.", 80),
    ]);
    let dir = tempfile::tempdir().unwrap();
    let (emit, _) = collect();
    let mut s = settings(&model, &voice).await;
    s.tools = vec![
        tools_server(&first, "aqua").await,
        tools_server(&second, "other").await,
    ];
    let mut p = Pipeline::new(s, dir.path().join("c.json"), emit);
    p.answer("search the other one").await;
    let requests = model.requests.lock().unwrap();
    let names: Vec<&str> = requests[0]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["function"]["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["search", "other_search"]);
    // The call goes to the second server, by the tool's own name.
    assert!(first.calls.lock().unwrap().is_empty());
    assert_eq!(second.calls.lock().unwrap()[0]["params"]["name"], "search");
}

#[tokio::test]
async fn clears_and_compacts_at_the_owners_word() {
    let (model, voice) = (Model::default(), Voice::default());
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("c.json");
    let (emit, events) = collect();
    let mut p = Pipeline::new(settings(&model, &voice).await, path.clone(), emit);
    for i in 0..3 {
        p.answer(&format!("q{i}")).await;
    }
    p.compact_now().await;
    assert!(
        events
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(e, Event::Notice { .. })),
        "three exchanges are too few to summarise"
    );
    for i in 3..6 {
        p.answer(&format!("q{i}")).await;
    }
    model
        .replies
        .lock()
        .unwrap()
        .push(reply("They asked q0 and q1.", 40));
    p.compact_now().await;
    assert_eq!(
        p.conversation.summary.as_deref(),
        Some("They asked q0 and q1.")
    );
    assert_eq!(
        p.conversation.messages.len(),
        8,
        "the last four exchanges are kept"
    );
    p.clear();
    assert!(p.conversation.messages.is_empty() && p.conversation.summary.is_none());
    assert!(events.lock().unwrap().contains(&Event::Cleared));
    let (emit, _) = collect();
    assert!(
        Pipeline::new(Settings::default(), path, emit)
            .conversation
            .messages
            .is_empty()
    );
}

#[tokio::test]
async fn switched_off_it_neither_listens_nor_hears() {
    let (g, model, voice) = (Grapto::default(), Model::default(), Voice::default());
    g.bodies
        .lock()
        .unwrap()
        .push(segment(1, "what is two plus two?", "peabody"));
    let mut s = settings(&model, &voice).await;
    s.graptostream = serve(grapto_app(g.clone())).await;
    s.enabled = false;
    let dir = tempfile::tempdir().unwrap();
    let (emit, _) = collect();
    let running = pipeline::start(s, dir.path().join("c.json"), emit, Timing::default());
    tokio::time::sleep(Duration::from_millis(300)).await;
    running.stop().await;
    assert!(
        g.listens.lock().unwrap().is_empty(),
        "no listen request is made"
    );
    assert!(voice.said.lock().unwrap().is_empty());
    assert!(model.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn shows_every_request_to_the_model_and_every_reply() {
    let (model, voice, tools) = (Model::default(), Voice::default(), Tools::default());
    model.replies.lock().unwrap().extend([
        tool_call("search", json!({"query": "weather"})),
        reply("Sunny.", 120),
    ]);
    let dir = tempfile::tempdir().unwrap();
    let (emit, events) = collect();
    let mut s = settings(&model, &voice).await;
    s.tools = vec![tools_server(&tools, "stub").await];
    let mut p = Pipeline::new(s, dir.path().join("c.json"), emit);
    p.answer("weather?").await;
    let ev = events.lock().unwrap();
    let traffic: Vec<(&str, &str, &str)> = ev
        .iter()
        .filter_map(|e| match e {
            Event::ToModel { summary, text } => Some(("to", summary.as_str(), text.as_str())),
            Event::FromModel { summary, text } => Some(("from", summary.as_str(), text.as_str())),
            _ => None,
        })
        .collect();
    let summaries: Vec<(&str, &str)> = traffic.iter().map(|(d, s, _)| (*d, *s)).collect();
    assert_eq!(
        summaries,
        [
            ("to", "2 messages, 1 tool"),
            ("from", "asks for search, 90 tokens"),
            ("to", "4 messages, 1 tool"),
            ("from", "answer, 120 tokens"),
        ]
    );
    // Opened, a request shows every message and what was offered; a reply its parts.
    assert!(traffic[0].2.contains("[user]\nweather?"));
    assert!(
        traffic[0]
            .2
            .contains("Tools available to you:\n- search: Web search")
    );
    assert!(traffic[0].2.ends_with("tools: search"));
    assert!(
        traffic[2]
            .2
            .contains("[tool result c1]\nIt is 1:20 pm in California.")
    );
    assert!(traffic[1].2.contains(
        r#"[asks for]
search({"query":"weather"})"#
    ));
    assert!(
        traffic[3]
            .2
            .starts_with("[reasoning]\nthinking\n\n[answer]\nSunny.")
    );
}

/// A proxy in front of `to` that closes its first connection as soon as a request
/// arrives, as a server does with an idle connection at the moment a client reuses it.
async fn dropping_first_connection(to: Endpoint) -> Endpoint {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let target = to.url.trim_start_matches("http://").to_string();
    tokio::spawn(async move {
        let mut first = true;
        while let Ok((mut inbound, _)) = listener.accept().await {
            if first {
                first = false;
                let mut buf = [0u8; 64];
                let _ = tokio::io::AsyncReadExt::read(&mut inbound, &mut buf).await;
                drop(inbound);
                continue;
            }
            let target = target.clone();
            tokio::spawn(async move {
                let mut outbound = tokio::net::TcpStream::connect(target).await.unwrap();
                let _ = tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await;
            });
        }
    });
    Endpoint::new(&format!("http://{addr}/mcp"))
}

#[tokio::test]
async fn sends_a_tool_request_again_when_its_connection_closes_unanswered() {
    let tools = Tools::default();
    let direct = tools_server(&tools, "stub").await;
    let base = Endpoint::new(direct.url.trim_end_matches("/mcp"));
    let ep = dropping_first_connection(base).await;
    let session = agent_peabody_core::mcp::Session::open(ep)
        .await
        .expect("opened on the second try");
    assert_eq!(session.name, "stub");
    let listed = session.tools().await.unwrap();
    assert_eq!(listed[0]["function"]["name"], "search");
    let (out, failed) = session.call("search", json!({"query": "x"})).await;
    assert!(!failed, "{out}");
}

#[tokio::test]
async fn clear_and_compact_do_not_wait_for_a_slow_answer() {
    let (g, model, voice) = (Grapto::default(), Model::default(), Voice::default());
    g.bodies.lock().unwrap().push(segment(
        1,
        "a question the model is slow to answer",
        "peabody",
    ));
    *model.delay.lock().unwrap() = Duration::from_secs(60);
    let mut s = settings(&model, &voice).await;
    s.graptostream = serve(grapto_app(g.clone())).await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("c.json");
    let (emit, events) = collect();
    let running = pipeline::start(s, path.clone(), emit, Timing::default());
    wait_for(|| !model.requests.lock().unwrap().is_empty()).await;
    running.command(pipeline::Command::Clear);
    wait_for(|| events.lock().unwrap().contains(&Event::Cleared)).await;
    running.stop().await;
    let ev = events.lock().unwrap();
    assert!(ev.contains(&Event::Notice {
        text: "The question in hand was abandoned.".into()
    }));
    assert!(
        voice.said.lock().unwrap().is_empty(),
        "nothing was answered"
    );
    let (emit, _) = collect();
    assert!(
        Pipeline::new(Settings::default(), path, emit)
            .conversation
            .messages
            .is_empty()
    );
}

fn hush_event(id: u32, to: &str, end: std::time::SystemTime) -> String {
    let secs = end
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64();
    // graptostream's ISO form, from the epoch seconds (UTC).
    let t = time_utc(secs);
    let data = json!({"id": id, "kind": "hush", "state": "provisional", "text": "hush", "to": to, "end": t});
    format!("id: {id}\nevent: hush\ndata: {data}\n\n")
}

/// Epoch seconds as "YYYY-MM-DDThh:mm:ss.mmmZ" (civil-from-days).
fn time_utc(secs: f64) -> String {
    let total = secs.floor() as i64;
    let millis = ((secs - secs.floor()) * 1000.0).round() as i64;
    let (days, rem) = (total.div_euclid(86_400), total.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{millis:03}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

#[tokio::test]
async fn a_hush_withdraws_the_answer_and_marks_it_interrupted() {
    let (model, voice) = (Model::default(), Voice::default());
    model
        .replies
        .lock()
        .unwrap()
        .push(reply("A very long story about a duck.", 80));
    let dir = tempfile::tempdir().unwrap();
    let (emit, events) = collect();
    let s = Settings {
        instant_replies: vec!["One moment.".into()],
        ..settings(&model, &voice).await
    };
    let mut p = Pipeline::new(s, dir.path().join("c.json"), emit);
    p.answer("tell me a story").await;
    // i1 is the instant reply (already spoken), i2 the answer (still playing).
    voice.finished.lock().unwrap().push("i1".into());
    p.run(pipeline::Command::Hush).await;
    assert_eq!(*voice.withdrawn.lock().unwrap(), ["i1", "i2"]);
    assert_eq!(
        p.conversation.messages.last().unwrap().content,
        "A very long story about a duck. (interrupted)"
    );
    assert!(events.lock().unwrap().contains(&Event::Hushed {
        text: "Hushed; the answer was cut off.".into()
    }));
    // A second hush has nothing left to withdraw and marks nothing.
    p.run(pipeline::Command::Hush).await;
    assert_eq!(voice.withdrawn.lock().unwrap().len(), 2);
    assert!(
        p.conversation
            .messages
            .last()
            .unwrap()
            .content
            .ends_with("duck. (interrupted)")
    );
}

#[tokio::test]
async fn a_hush_from_graptostream_abandons_a_question_still_waiting_for_the_model() {
    let (g, model, voice) = (Grapto::default(), Model::default(), Voice::default());
    *model.delay.lock().unwrap() = Duration::from_secs(60);
    let mut s = Settings {
        instant_replies: vec!["One moment.".into()],
        ..settings(&model, &voice).await
    };
    s.graptostream = serve(grapto_app(g.clone())).await;
    g.bodies
        .lock()
        .unwrap()
        .push(segment(1, "tell me a long story", "peabody"));
    let dir = tempfile::tempdir().unwrap();
    let (emit, events) = collect();
    let running = pipeline::start(s, dir.path().join("c.json"), emit, Timing::default());
    wait_for(|| !model.requests.lock().unwrap().is_empty()).await;
    // The reader reconnects and the next body carries the hush, said just now.
    g.bodies
        .lock()
        .unwrap()
        .push(hush_event(2, "peabody", std::time::SystemTime::now()));
    wait_for(|| {
        events
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(e, Event::Hushed { .. }))
    })
    .await;
    running.stop().await;
    let ev = events.lock().unwrap();
    assert!(ev.contains(&Event::Notice {
        text: "The question in hand was abandoned.".into()
    }));
    // i1 was the phrase prepared at start-up, i2 the instant reply; no answer was spoken.
    assert_eq!(*voice.withdrawn.lock().unwrap(), ["i2"]);
    assert_eq!(voice.said.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn an_old_hush_replayed_after_a_reconnection_is_ignored() {
    let (g, model, voice) = (Grapto::default(), Model::default(), Voice::default());
    let mut s = settings(&model, &voice).await;
    s.graptostream = serve(grapto_app(g.clone())).await;
    let old = std::time::SystemTime::now() - Duration::from_secs(30);
    g.bodies
        .lock()
        .unwrap()
        .push(hush_event(1, "peabody", old) + &segment(2, "what is two plus two?", "peabody"));
    let dir = tempfile::tempdir().unwrap();
    let (emit, events) = collect();
    let running = pipeline::start(s, dir.path().join("c.json"), emit, Timing::default());
    wait_for(|| !voice.said.lock().unwrap().is_empty()).await;
    running.stop().await;
    assert!(
        !events
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(e, Event::Hushed { .. }))
    );
    assert_eq!(voice.said.lock().unwrap()[0]["text"], "Fine.");
}

#[tokio::test]
async fn a_long_tool_result_is_not_kept_in_the_conversation() {
    let (model, voice, tools) = (Model::default(), Voice::default(), Tools::default());
    *tools.result.lock().unwrap() = Some("x".repeat(pipeline::KEEP_TOOL_RESULT + 1));
    model.replies.lock().unwrap().extend([
        tool_call("search", json!({"query": "everything"})),
        reply("Here is the gist.", 120),
    ]);
    let dir = tempfile::tempdir().unwrap();
    let (emit, _) = collect();
    let mut s = settings(&model, &voice).await;
    s.tools = vec![tools_server(&tools, "stub").await];
    let mut p = Pipeline::new(s, dir.path().join("c.json"), emit);
    p.answer("search for everything").await;
    assert_eq!(p.conversation.messages[1].content, "Here is the gist.");
    // The next question starts afresh: nothing is carried over.
    model.replies.lock().unwrap().push(reply("Fine.", 80));
    p.answer("and?").await;
    assert_eq!(p.conversation.messages[3].content, "Fine.");
}

#[tokio::test]
async fn lists_models_from_a_server_that_wants_a_version_header() {
    // Anthropic's models endpoint answers 400 without `anthropic-version`.
    let app = Router::new().route(
        "/v1/models",
        get(|h: HeaderMap| async move {
            if h.contains_key("anthropic-version") {
                (
                    StatusCode::OK,
                    axum::Json(
                        json!({"data": [{"id": "claude-haiku-5-5"}, {"id": "claude-sonnet-5-5"}]}),
                    ),
                )
            } else {
                (
                    StatusCode::BAD_REQUEST,
                    axum::Json(
                        json!({"error": {"message": "anthropic-version: header is required"}}),
                    ),
                )
            }
        }),
    );
    let ep = serve(app).await;
    let models = agent_peabody_core::llm::models(&agent_peabody_core::http::client(), &ep).await;
    assert_eq!(models.unwrap(), ["claude-haiku-5-5", "claude-sonnet-5-5"]);
}
