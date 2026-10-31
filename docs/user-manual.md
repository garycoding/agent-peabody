<!--
SPDX-FileCopyrightText: 2026 Gary Frattarola <garyf@parkviewlab.ai>
SPDX-License-Identifier: CC-BY-4.0
-->

# Peabody user manual

Peabody lets you talk to a language model aloud. You speak to it through graptostream, which turns your speech into text, and it answers through phonorheos, which turns its answer into speech. This manual covers setting it up and using it; the [README](../README.md) describes how it works.

## Before you start

Peabody needs three other programs, each on this machine or on another machine on your network:

- **graptostream** (macOS only) hears you. In its Settings › Spoken names, map the name you will say, for example "Peabody", to the app name `peabody`.
- **phonorheos** v0.3.0 or later speaks the answers.
- **A model server** answers the questions: LM Studio, or llama.cpp's `llama-server`, serving the OpenAI chat API.

On Ubuntu, graptostream runs on a Mac and Peabody reaches it over the network.

## The window

- **On / Off:** switches Peabody off and on. Off, it withdraws its listen request from graptostream (so graptostream in "When asked" mode stops listening for it) and answers nothing; what you say while it is off is not answered later. It stays as you left it when reopened.
- **The top line:** a dot for each service (graptostream, the model, phonorheos, and the tools when servers are set): green when it answers, orange when it does not, with the reason when you point at it. A line below the header repeats any problem.
- **The context:** how full the model's context window is, as a percentage and a bar; pointing at it gives the tokens. The conversation is summarised on its own at 75%.
- **Compact:** summarises all but the last four exchanges now, without waiting for 75%.
- **Clear:** deletes the whole conversation; it asks for a second click within three seconds.
- Compact and Clear act at once, even while Peabody is waiting for an answer: that question is abandoned and not kept. Off does the same.
- **The conversation:** each line labelled with its time: what Peabody heard ("Input"), the instant reply (in italics), each request to the model ("To model") and its reply ("From model"), each tool call, what Peabody answered, when the conversation was summarised or cleared, and any problem. A request or reply shows a one-line summary ("4 messages, 3 tools"; "answer, 3287 tokens"); click it to see the whole of it: every message sent, with the tools offered, or the reasoning, answer, tool calls and token counts received.
- **Settings** opens the settings.

## Talking to Peabody

Say the spoken name and then your question, in one breath: "Peabody, what is the capital of Portugal?". graptostream removes the name and passes the rest to Peabody. You can also say the name alone and ask within four seconds. If you speak again whilst Peabody is answering, it answers each in turn.

As soon as it hears you, Peabody says a short phrase such as "One moment please...", so that the wait for the model is not silent; on a slow machine the answer can take several seconds. It chooses one of its instant replies at random, never the one it said last. When it starts, it asks phonorheos to make and keep the speech for every instant reply without playing it, so that each plays at once when wanted; if phonorheos has been restarted since, the first use of a phrase takes a moment and it is kept again. The instant reply is not part of the conversation: the model never sees it.

With tools servers set, Peabody lists their tools in the model's instructions, as each server describes them, and offers them with each question. Where two servers offer a tool of the same name, the second is given its server's name in front (`other_search`). When the model asks for one (a web search, say), Peabody calls it, shows the call in the window ("Tool: search: current time in California"), and passes the result back, up to four rounds, after which the model must answer with what it has. Each round adds the tool's time and another turn of the model's, so an answer that needs a search takes some seconds longer. The tool calls and their results are not kept in the conversation; a result longer than 8000 characters is cut. A fourth dot shows whether the servers answer; a server that cannot be reached is named there, and Peabody answers with the tools of the others. A model may still answer from habit that it cannot look things up, especially in a long conversation from before it had tools.

Peabody remembers the conversation, including after it is closed and reopened. There is no way to start afresh: when the conversation grows to three quarters of the model's context window, Peabody asks the model to summarise all but the last four exchanges, and carries on from the summary. The window shows each summary.

## Settings

| Setting | What it is |
|---|---|
| Name | The app name graptostream addresses your speech to (`peabody`). It must match the app name in graptostream's Spoken names. |
| Voice | The phonorheos voice Peabody speaks with. `bm_` and `bf_` voices are British, `am_` and `af_` American. |
| System prompt | The instructions the model is given before the conversation. The default asks for short answers in plain spoken sentences. |
| Instant replies | The phrases said at once when Peabody hears you, one a line. The defaults are "Allow me to think about that...", "One moment please...", "Thinking..." and "Allow me time to prepare a cogent response...". Leave the list empty for none. |
| graptostream address and token | Where graptostream answers: `http://127.0.0.1:35860` on this Mac, or `http://<mac>.local:35860` with a token issued in graptostream's Settings › Network, with serving to other machines switched on there. |
| phonorheos address and token | Where phonorheos answers: `http://127.0.0.1:35861` on this machine, or `http://<machine>.local:35861` with a token issued in phonorheos's Network section, with "Open to the LAN" ticked. |
| Model server address and token | For LM Studio, `http://127.0.0.1:1234` (or `http://<machine>.local:1234` with "Serve on local network" on); for llama.cpp, the address `llama-server` listens on. A token only where the server is set to require one. |
| Model | The model's name as the server gives it. List models asks the server. |
| Context size | Leave empty to use the size the server reports; set it where the server reports none or you want Peabody to summarise sooner. |
| Tools (MCP servers) | Any number of MCP servers whose tools the model may use before it answers, each an address and, where the server requires one, a token; Add server and Remove change the list. aqua-roaming's web search is at `http://trixie.local:35830/mcp`. An empty list means no tools. |
| Let the model think before answering | Off by default. A reasoning model such as Gemma 4 thinks before it answers, which helps with hard questions but takes far longer: on a MacBook Air, a one-sentence answer took 20 seconds with thinking and 2 seconds without. Off, Peabody asks the server to skip the thinking (`"reasoning_effort": "none"`); LM Studio honours this. A server that does not know the request ignores it. |

Saving the settings reconnects at once.

## When something goes wrong

- **A dot is orange.** Point at it to read why. "not reachable" means the address is wrong or the program is not running; "answered 401 Unauthorized" means the token is missing or wrong.
- **Peabody says "I could not get an answer from the model."** The model server could not be reached or refused the request; the window says why.
- **Peabody says "I have no answer to that."** The model returned an empty answer. With a reasoning model, this can mean that its reasoning used up the space for the answer.
- **Nothing happens when you speak.** Check that graptostream heard you in its own window, and that the line was addressed to Peabody's name. A name the speech engine mishears is better replaced by an ordinary, distinct word.
- **Answers are slow.** A reasoning model writes out its reasoning before the answer; on a small machine that can take minutes.

## Privacy

What you say to Peabody goes to the model server, and the answers to phonorheos. The conversation and the settings, tokens included, are kept in Peabody's data folder, readable by your account alone: `~/Library/Application Support/io.github.garycoding.agent-peabody/` on macOS and `~/.local/share/io.github.garycoding.agent-peabody/` on Linux (the window's last line gives the folder).
