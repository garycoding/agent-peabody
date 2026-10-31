// SPDX-FileCopyrightText: 2026 Gary Frattarola <garyf@parkviewlab.ai>
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The app: runs the pipeline with the saved settings, restarts it when they change, and
//! gives the window its state and what happens.

use agent_peabody_core::llm;
use agent_peabody_core::pipeline::{self, Event, Running, Service, Timing};
use agent_peabody_core::settings::{Endpoint, Settings};
use agent_peabody_core::{http, voices};
use serde::Serialize;
use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::{Emitter, Manager, RunEvent};

/// The lines of the conversation the window shows; the conversation itself is in its file.
const LOG: usize = 200;

#[derive(Clone, Serialize)]
struct Line {
    at: String,
    #[serde(flatten)]
    event: Event,
}

#[derive(Clone, Serialize)]
struct LinkState {
    ok: bool,
    detail: String,
}

#[derive(Default)]
struct Shared {
    log: VecDeque<Line>,
    links: BTreeMap<String, LinkState>,
    tokens: Option<u32>,
    size: Option<u32>,
}

struct App {
    dir: PathBuf,
    settings: Mutex<Settings>,
    shared: Arc<Mutex<Shared>>,
    running: tokio::sync::Mutex<Option<Running>>,
}

impl App {
    fn settings_path(&self) -> PathBuf {
        self.dir.join("settings.json")
    }

    fn conversation_path(&self) -> PathBuf {
        self.dir.join("conversation.json")
    }

    /// Stops the pipeline, if it runs, and starts it with the current settings.
    async fn restart(&self, handle: tauri::AppHandle) {
        let mut running = self.running.lock().await;
        if let Some(r) = running.take() {
            r.stop().await;
        }
        {
            let mut s = self.shared.lock().unwrap();
            s.links.clear();
            s.size = None;
        }
        let shared = self.shared.clone();
        let emit = Arc::new(move |ev: Event| {
            let line = Line {
                at: now(),
                event: ev.clone(),
            };
            {
                let mut s = shared.lock().unwrap();
                match &ev {
                    Event::Link {
                        service,
                        ok,
                        detail,
                    } => {
                        s.links.insert(
                            service_name(*service).into(),
                            LinkState {
                                ok: *ok,
                                detail: detail.clone(),
                            },
                        );
                    }
                    Event::Context { tokens, size } => {
                        s.tokens = *tokens;
                        s.size = *size;
                    }
                    _ => {
                        s.log.push_back(line.clone());
                        if s.log.len() > LOG {
                            s.log.pop_front();
                        }
                    }
                }
            }
            let _ = handle.emit("peabody", line);
        });
        let settings = self.settings.lock().unwrap().clone();
        *running = Some(pipeline::start(
            settings,
            self.conversation_path(),
            emit,
            Timing::default(),
        ));
    }
}

fn service_name(s: Service) -> &'static str {
    match s {
        Service::Graptostream => "graptostream",
        Service::Phonorheos => "phonorheos",
        Service::Llm => "llm",
    }
}

/// The time of day, for the window's lines.
fn now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    secs.to_string()
}

#[derive(Serialize)]
struct WindowState {
    version: String,
    data: String,
    settings: Settings,
    voices: Vec<&'static str>,
    log: Vec<Line>,
    links: BTreeMap<String, LinkState>,
    tokens: Option<u32>,
    size: Option<u32>,
}

#[tauri::command]
fn state(app: tauri::State<'_, App>, handle: tauri::AppHandle) -> WindowState {
    let s = app.shared.lock().unwrap();
    WindowState {
        version: handle.package_info().version.to_string(),
        data: app.dir.display().to_string(),
        settings: app.settings.lock().unwrap().clone(),
        voices: voices::VOICES.to_vec(),
        log: s.log.iter().cloned().collect(),
        links: s.links.clone(),
        tokens: s.tokens,
        size: s.size,
    }
}

#[tauri::command]
async fn save_settings(
    settings: Settings,
    app: tauri::State<'_, App>,
    handle: tauri::AppHandle,
) -> Result<(), String> {
    settings
        .save(&app.settings_path())
        .map_err(|e| e.to_string())?;
    *app.settings.lock().unwrap() = settings;
    app.restart(handle).await;
    Ok(())
}

/// The models a server offers, for the settings dialog, before its settings are saved.
#[tauri::command]
async fn models(url: String, token: String) -> Result<Vec<String>, String> {
    llm::models(&http::client(), &Endpoint { url, token }).await
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
                let _ = w.set_focus();
            }
        }))
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            let settings = Settings::load(&dir.join("settings.json"));
            let state = App {
                dir,
                settings: Mutex::new(settings),
                shared: Arc::default(),
                running: tokio::sync::Mutex::new(None),
            };
            app.manage(state);
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let app = handle.state::<App>();
                app.restart(handle.clone()).await;
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![state, save_settings, models])
        .build(tauri::generate_context!())
        .expect("the app could not be built")
        .run(|handle, event| {
            if let RunEvent::Exit = event {
                // Withdraw the listen request, so that graptostream stops listening for Peabody.
                let app = handle.state::<App>();
                tauri::async_runtime::block_on(async {
                    if let Some(r) = app.running.lock().await.take() {
                        r.stop().await;
                    }
                });
            }
        });
}
