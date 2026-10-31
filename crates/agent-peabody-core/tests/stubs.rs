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
}

fn voice_app(v: Voice) -> Router {
    Router::new()
        .route(
            "/speak",
            post(
                |State(v): State<Voice>, axum::Json(body): axum::Json<Value>| async move {
                    v.said.lock().unwrap().push(body);
                    (
                        StatusCode::CREATED,
                        axum::Json(json!({"id": "i", "event": 1, "ahead": 0})),
                    )
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
                            json!({"content": [{"type": "text", "text": "It is 1:20 pm in California."}]})
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
    // The conversation keeps the question and the answer only.
    let kept: Vec<&str> = p
        .conversation
        .messages
        .iter()
        .map(|m| m.content.as_str())
        .collect();
    assert_eq!(
        kept,
        ["what time is it in California?", "It is twenty past one."]
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
