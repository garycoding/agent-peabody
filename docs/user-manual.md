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

- **The top line:** a dot for each service (graptostream, the model, phonorheos): green when it answers, orange when it does not, with the reason when you point at it. A line below the header repeats any problem.
- **The context:** how many tokens the conversation takes, out of the model's context window.
- **The conversation:** what you said ("You"), the instant reply (in italics), what Peabody answered, when the conversation was summarised, and any problem.
- **Settings** opens the settings.

## Talking to Peabody

Say the spoken name and then your question, in one breath: "Peabody, what is the capital of Portugal?". graptostream removes the name and passes the rest to Peabody. You can also say the name alone and ask within four seconds. If you speak again whilst Peabody is answering, it answers each in turn.

As soon as it hears you, Peabody says a short phrase such as "One moment please...", so that the wait for the model is not silent; on a slow machine the answer can take several seconds. It chooses one of its instant replies at random, never the one it said last. When it starts, it asks phonorheos to make and keep the speech for every instant reply without playing it, so that each plays at once when wanted; if phonorheos has been restarted since, the first use of a phrase takes a moment and it is kept again. The instant reply is not part of the conversation: the model never sees it.

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

Saving the settings reconnects at once.

## When something goes wrong

- **A dot is orange.** Point at it to read why. "not reachable" means the address is wrong or the program is not running; "answered 401 Unauthorized" means the token is missing or wrong.
- **Peabody says "I could not get an answer from the model."** The model server could not be reached or refused the request; the window says why.
- **Peabody says "I have no answer to that."** The model returned an empty answer. With a reasoning model, this can mean that its reasoning used up the space for the answer.
- **Nothing happens when you speak.** Check that graptostream heard you in its own window, and that the line was addressed to Peabody's name. A name the speech engine mishears is better replaced by an ordinary, distinct word.
- **Answers are slow.** A reasoning model writes out its reasoning before the answer; on a small machine that can take minutes.

## Privacy

What you say to Peabody goes to the model server, and the answers to phonorheos. The conversation and the settings, tokens included, are kept in Peabody's data folder, readable by your account alone: `~/Library/Application Support/io.github.garycoding.agent-peabody/` on macOS and `~/.local/share/io.github.garycoding.agent-peabody/` on Linux (the window's last line gives the folder).
