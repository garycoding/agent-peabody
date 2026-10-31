<!--
SPDX-FileCopyrightText: 2026 Gary Frattarola <garyf@parkviewlab.ai>
SPDX-License-Identifier: CC-BY-4.0
-->

# agent-peabody

Peabody is a desktop app for macOS and Ubuntu Desktop that holds a spoken conversation with a language model. You say its name and a question; [graptostream](https://github.com/ParkviewLab/graptostream) hears it and passes the text to Peabody; Peabody sends it, with the conversation so far, to an OpenAI-compatible server (llama.cpp's `llama-server` or LM Studio); and [phonorheos](https://github.com/ParkviewLab/phonorheos) speaks the answer.

Download: the [Releases](https://github.com/garycoding/agent-peabody/releases) page.  
How to use it: the [user manual](docs/user-manual.md).

## Status

Tried on a MacBook Air (macOS 26) with graptostream, phonorheos and LM Studio (`google/gemma-4-12b`): answers spoken, the conversation kept across questions and across a restart, and compacted when it filled three quarters of the context window. Addressing by the spoken name "Peabody" through graptostream has not yet been tried with a person's voice. The Linux packages are built on Ubuntu 24.04 and have not yet been tried on a desktop. The macOS build is signed ad hoc only: not with a Developer ID, and not notarised.

## Requirements and install

- **graptostream**, which runs on macOS only, on this Mac or on a Mac on the LAN, with a spoken name mapped to Peabody's name (`peabody` by default) in its Spoken names.
- **phonorheos** v0.3.0 or later, on this machine or on the LAN.
- **Optionally, MCP servers** whose tools the model may use, such as [aqua-roaming](https://github.com/garycoding/aqua-roaming)'s web search through SearXNG.
- **A model server** with the OpenAI chat API: llama.cpp's `llama-server` or LM Studio, on this machine or on the LAN.

macOS (Apple Silicon, macOS 15 or later): open `agent-peabody-<version>-macos-arm64.dmg` and drag Peabody to Applications. The build is signed ad hoc only, not with a Developer ID, and is not notarised, so macOS refuses to open it the first time; clear the quarantine flag once:

```bash
xattr -dr com.apple.quarantine /Applications/Peabody.app
```

Ubuntu Desktop (x86_64): install the `.deb`, or run the AppImage.

```bash
sudo apt install ./agent-peabody-<version>-linux-amd64.deb
```

## Using it

On first start, Peabody opens its settings. Give the model server's address and model; graptostream and phonorheos default to this machine. Then say "Peabody, what is two plus two?". The window shows what was heard, the answer, each service's state, and how full the context window is. The [user manual](docs/user-manual.md) covers every setting.

## How it works

- **Hearing:** Peabody holds a graptostream listen request (`POST /listen`, renewed every four minutes, withdrawn on quit), so that graptostream listens whilst Peabody runs, and reads `GET /events?to=<name>&final=true`, resuming from the last event after a reconnection.
- **The model:** each utterance is sent to `POST /v1/chat/completions` with the system prompt, the summary of earlier conversation (if any) and the exchanges since. Only the answer is kept and spoken; a reasoning model's reasoning is not.
- **Speaking:** as soon as an utterance arrives, one of the instant replies ("One moment please..." and the like) goes to phonorheos, which plays its kept speech at once; Peabody asked phonorheos to make and keep that speech, unheard, when it started (`"speak": false, "cache": true`). The answer then goes to phonorheos's `POST /speak` with the chosen voice. Utterances that arrive whilst Peabody is answering are answered in order.
- **Context:** the window's size comes from the settings, else from llama.cpp's `GET /props` or LM Studio's `GET /api/v0/models`. When the conversation reaches 75% of it, measured by the token counts the server returns, Peabody asks the model to summarise all but the last four exchanges. If the server reports the window exceeded, Peabody summarises and tries once more.

## Privacy

What you say to Peabody goes to the model server you configure, and the answer to phonorheos; the [user manual](docs/user-manual.md#privacy) says where the conversation and the settings are kept.

## Building

Rust (a Cargo workspace: `crates/agent-peabody-core`, the pipeline with no window, and `crates/agent-peabody-app`, the Tauri app) with plain HTML in `ui/`; no Node. On Linux, install Tauri's system libraries first (`libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev`, and for the AppImage `patchelf file libfuse2t64`).

```bash
cargo install tauri-cli --version "^2" --locked
cargo test
scripts/build-app
```

`scripts/build-app` writes the installers to `dist/`. The checks before a pull request are in [`docs/CONTRIBUTING.md`](docs/CONTRIBUTING.md).

## Releasing

A release is cut from `main` with a `v*` tag; `release.yml` builds the installers on macOS and Ubuntu and creates the Release. The macOS build is signed and notarised when the repository has the five Apple secrets (`CSC_LINK`, `CSC_KEY_PASSWORD`, `APPLE_API_KEY_B64`, `APPLE_API_KEY_ID`, `APPLE_API_ISSUER`), and signed ad hoc otherwise.

## Licence

The code is `AGPL-3.0-or-later`; the documentation `CC-BY-4.0`; the app icon all rights reserved. See [`LICENSING.md`](LICENSING.md). Why Peabody is built as it is: [`docs/why.md`](docs/why.md).

---
<sub>© 2026 Gary Frattarola · Licensed under [CC-BY-4.0](LICENSES/CC-BY-4.0.txt)</sub>
