// SPDX-FileCopyrightText: 2026 Gary Frattarola <garyf@parkviewlab.ai>
// SPDX-License-Identifier: AGPL-3.0-or-later
"use strict";

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const $ = (id) => document.getElementById(id);

let state = null;

function el(tag, attrs, ...children) {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs || {})) e.setAttribute(k, v);
  for (const c of children) e.append(c);
  return e;
}

const WHO = { heard: "You", holding: "Peabody", replied: "Peabody", compacted: "Summary", error: "Problem" };

function lineText(l) {
  if (l.kind === "compacted") return "The conversation was summarised to fit the context window: " + l.summary;
  return l.text;
}

function addLine(l) {
  const log = $("log");
  log.querySelector(".empty")?.remove();
  const at = new Date(Number(l.at) * 1000).toLocaleTimeString();
  log.append(el("li", { class: l.kind, title: at }, el("span", { class: "who" }, WHO[l.kind] || l.kind), el("span", { class: "text" }, lineText(l))));
  while (log.children.length > 200) log.firstElementChild.remove();
  window.scrollTo(0, document.body.scrollHeight);
}

function showLink(service, link) {
  const e = $("link-" + service);
  e.classList.toggle("ok", !!link && link.ok);
  e.classList.toggle("down", !!link && !link.ok);
  e.title = !link ? "Not yet checked" : link.ok ? "Answers" : link.detail;
  const problems = Object.entries(state.links).filter(([, l]) => !l.ok).map(([s, l]) => `${s === "llm" ? "The model server" : s}: ${l.detail}`);
  $("problems").hidden = !problems.length;
  $("problems").textContent = problems.join(" · ");
}

function showContext() {
  const { tokens, size } = state;
  $("context").textContent = size
    ? `${tokens ?? "?"} of ${size} tokens` + (tokens ? ` (${Math.round((100 * tokens) / size)}%)` : "")
    : "Context size unknown";
  $("context").title = "The conversation is summarised when it reaches 75% of the context window.";
}

async function load() {
  state = await invoke("state");
  $("log").replaceChildren(el("li", { class: "empty" }, "Say “Peabody,” then what you want to ask."));
  for (const l of state.log) addLine(l);
  for (const s of ["graptostream", "llm", "phonorheos"]) showLink(s, state.links[s]);
  showContext();
  $("about").textContent = `Peabody ${state.version} · data in ${state.data}`;
  const voice = $("voice");
  if (!voice.options.length) for (const v of state.voices) voice.append(el("option", { value: v }, v));
  if (!state.settings.llm.url) openSettings();
}

listen("peabody", ({ payload: l }) => {
  if (!state) return;
  if (l.kind === "link") {
    state.links[l.service] = { ok: l.ok, detail: l.detail };
    showLink(l.service, state.links[l.service]);
  } else if (l.kind === "context") {
    state.tokens = l.tokens;
    state.size = l.size;
    showContext();
  } else {
    addLine(l);
  }
});

// The settings dialog: a field named "a.b" is the setting a.b.
function fields() {
  return [...$("form").elements].filter((e) => e.name);
}

function openSettings() {
  for (const f of fields()) {
    const v = f.name.split(".").reduce((o, k) => o?.[k], state.settings);
    if (f.type === "checkbox") f.checked = Boolean(v);
    else f.value = Array.isArray(v) ? v.join("\n") : v ?? "";
  }
  $("context-note").textContent = state.settings.context_size == null && state.size
    ? `The server reports ${state.size} tokens. Leave empty to use it.`
    : "Leave empty to use the size the server reports.";
  $("form-error").textContent = "";
  $("models-note").textContent = "";
  $("settings").showModal();
}

$("open-settings").addEventListener("click", openSettings);
$("cancel").addEventListener("click", () => $("settings").close());

$("fetch-models").addEventListener("click", async () => {
  const form = $("form").elements;
  $("models-note").textContent = "Asking the server…";
  try {
    const models = await invoke("models", { url: form["llm.url"].value.trim(), token: form["llm.token"].value.trim() });
    $("models").replaceChildren(...models.map((m) => el("option", { value: m })));
    $("models-note").textContent = models.length ? `The server offers: ${models.join(", ")}` : "The server lists no models.";
  } catch (e) {
    $("models-note").textContent = `The server could not be asked: ${e}`;
  }
});

$("form").addEventListener("submit", async (ev) => {
  ev.preventDefault();
  const s = structuredClone(state.settings);
  for (const f of fields()) {
    const path = f.name.split(".");
    const last = path.pop();
    const target = path.reduce((o, k) => o[k], s);
    if (f.type === "checkbox") {
      target[last] = f.checked;
      continue;
    }
    const v = f.value.trim();
    target[last] =
      f.name === "context_size" ? (v ? Number(v) : null)
      : f.name === "system_prompt" ? f.value
      : f.name === "instant_replies" ? v.split("\n").map((p) => p.trim()).filter(Boolean)
      : v;
  }
  try {
    await invoke("save_settings", { settings: s });
    $("settings").close();
    await load();
  } catch (e) {
    $("form-error").textContent = `The settings could not be saved: ${e}`;
  }
});

load();
