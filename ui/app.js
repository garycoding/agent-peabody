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

const WHO = { heard: "Input", holding: "Peabody", tool: "Tool", notice: "Note", cleared: "Cleared", to_model: "To model", from_model: "From model", replied: "Peabody", compacted: "Summary", error: "Problem" };

function lineText(l) {
  if (l.kind === "compacted") return "The conversation was summarised: " + l.summary;
  if (l.kind === "cleared") return "The conversation was cleared.";
  return l.text;
}

function addLine(l) {
  const log = $("log");
  log.querySelector(".empty")?.remove();
  const at = new Date(Number(l.at) * 1000).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit" });
  const label = el("span", { class: "who" }, el("span", { class: "at" }, at), WHO[l.kind] || l.kind);
  let body;
  if (l.kind === "to_model" || l.kind === "from_model") {
    // The whole request or reply, collapsed to its summary until opened.
    const pre = el("pre", {}, l.text);
    body = el("details", { class: "text" }, el("summary", {}, l.summary), pre);
  } else {
    body = el("span", { class: "text" }, lineText(l));
  }
  log.append(el("li", { class: l.kind }, label, body));
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
  const pct = size && tokens ? Math.round((100 * tokens) / size) : size ? 0 : null;
  $("context").textContent = pct == null ? "Context size unknown" : `${pct}% of context`;
  $("context-fill").style.width = `${Math.min(pct ?? 0, 100)}%`;
  $("context-fill").classList.toggle("high", (pct ?? 0) >= 75);
  $("context-box").title = size
    ? `${tokens ?? 0} of ${size} tokens. The conversation is summarised when it reaches 75%.`
    : "The model server has not reported its context size.";
}

function showPower() {
  const on = state.settings.enabled !== false;
  $("power").textContent = on ? "On" : "Off";
  $("power").classList.toggle("off", !on);
  $("power").title = on ? "Switch Peabody off: it stops listening and answering" : "Switch Peabody on";
  document.body.classList.toggle("off", !on);
}

async function load() {
  state = await invoke("state");
  $("log").replaceChildren(el("li", { class: "empty" }, "Say “Peabody,” then what you want to ask."));
  for (const l of state.log) addLine(l);
  $("link-tools").hidden = !(state.settings.tools || []).some((t) => t.url);
  showPower();
  for (const s of ["graptostream", "llm", "phonorheos", "tools"]) showLink(s, state.links[s]);
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
    if (l.kind === "cleared") $("log").replaceChildren();
    addLine(l);
  }
});

$("power").addEventListener("click", async () => {
  await invoke("set_enabled", { on: state.settings.enabled === false });
  await load();
});

$("compact").addEventListener("click", () => invoke("conversation", { action: "compact" }).catch((e) => addLine({ kind: "error", text: String(e), at: Date.now() / 1000 })));

// Clearing deletes the conversation, so it takes a second click within three seconds.
let clearTimer = null;
$("clear").addEventListener("click", async () => {
  const b = $("clear");
  if (!clearTimer) {
    b.textContent = "Clear? Click again";
    b.classList.add("confirm");
    clearTimer = setTimeout(() => { b.textContent = "Clear"; b.classList.remove("confirm"); clearTimer = null; }, 3000);
    return;
  }
  clearTimeout(clearTimer);
  clearTimer = null;
  b.textContent = "Clear";
  b.classList.remove("confirm");
  await invoke("conversation", { action: "clear" }).catch((e) => addLine({ kind: "error", text: String(e), at: Date.now() / 1000 }));
});

function serverRow(ep = { url: "", token: "" }) {
  const url = el("input", { class: "url", placeholder: "http://trixie.local:35830/mcp" });
  const token = el("input", { class: "token", type: "password", autocomplete: "off", placeholder: "Token, if needed" });
  url.value = ep.url || "";
  token.value = ep.token || "";
  const row = el("div", { class: "server" }, url, token);
  const remove = el("button", { type: "button" }, "Remove");
  remove.addEventListener("click", () => row.remove());
  row.append(remove);
  return row;
}

$("add-server").addEventListener("click", () => $("servers").append(serverRow()));

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
  $("servers").replaceChildren(...(state.settings.tools || []).map(serverRow));
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
  s.tools = [...$("servers").querySelectorAll(".server")]
    .map((r) => ({ url: r.querySelector(".url").value.trim(), token: r.querySelector(".token").value.trim() }))
    .filter((t) => t.url);
  try {
    await invoke("save_settings", { settings: s });
    $("settings").close();
    await load();
  } catch (e) {
    $("form-error").textContent = `The settings could not be saved: ${e}`;
  }
});

load();
