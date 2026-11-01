<!--
SPDX-FileCopyrightText: 2026 Gary Frattarola <garyf@parkviewlab.ai>
SPDX-License-Identifier: CC-BY-4.0
-->

# Why Peabody is as it is

The README says what Peabody is and how it is used. This file records why: the requirements it was built to, the evidence gathered during the build, and the decisions, each with its date.

## The need

graptostream lets the owner speak to apps by name, and phonorheos lets apps answer aloud. Peabody joins them to a language model, so that the owner can hold a spoken conversation with a model running on the lab's own machines.

## Requirements

Given by the owner in conversation on 2026-10-31.

1. A desktop app for macOS and Ubuntu Desktop, in a public repository in the owner's `garycoding` account, following the ParkviewLab handbook.
2. It hears, through graptostream, the utterances addressed to its name, sends them to a language model, and speaks the answers through phonorheos.
3. A settings dialog holds its name, the phonorheos voice, the connection to graptostream, to phonorheos and to the model server (each local, or over the LAN with a token), and the model.
4. It keeps the conversation, since the chat endpoint keeps none. The context window's size comes from the server or from a setting. When the conversation reaches 75% of the window, Peabody asks the model to summarise the older part and keeps the last few exchanges word for word.
5. There is no way to start a fresh conversation.
6. The model server may be llama.cpp (chimchim's, entered by the owner in the settings) or LM Studio (pacific's, used for testing and for the owner's own setup).
7. The code is `AGPL-3.0-or-later` with no commercial alternative; the documentation `CC-BY-4.0`; the brand marks reserved.

## Evidence

### LM Studio on pacific (2026-10-31)

- `POST /v1/chat/completions` returns `usage` (`prompt_tokens`, `completion_tokens`, `total_tokens`), so the 75% measure works as with llama.cpp.
- LM Studio does not serve llama.cpp's `GET /props`. Its `GET /api/v0/models` gives each model's `state` and, for a loaded model, `loaded_context_length` (8192 for `google/gemma-4-12b:2`, whose `max_context_length` is 262144). A loaded instance can carry a suffix (`:2`), so Peabody matches the model's name with or without it.
- Gemma 4 reasons before it answers. The reply carries the reasoning in `reasoning_content`, apart from `content`, and its tokens are counted in `completion_tokens` and again in `completion_tokens_details.reasoning_tokens`. One 400-word answer took 3787 reasoning tokens and about eight minutes on the MacBook Air.

### The live test (pacific, 2026-10-31)

- Peabody's listen request made graptostream listen in its When asked mode, and Peabody was listed among its readers.
- Speech played through the speaker did not test addressing: the speech engine dropped or misheard the leading "Peabody" in each of five attempts, in three voices, with a video playing in the room ("Hmm. Peabody" was heard as "Hey buddy"). Addressing awaits the owner's own voice; the rest of the chain was tested with a stand-in graptostream that delivers typed utterances.
- With LM Studio and phonorheos: answers were spoken in `bm_lewis`; a follow-up was answered from the kept conversation; the conversation survived a restart; with the context size set to 800, the conversation was summarised at 75%, the last four exchanges were kept, and a question about the first exchange was answered from the summary.

## Decisions

All by the owner on 2026-10-31.

- **D1 Name:** the name setting is the app name Peabody reads from graptostream (`/events?to=<name>`), `peabody` by default; the owner maps the spoken name to it in graptostream. graptostream already matches names loosely and removes them, and has no endpoint through which another app could register a name.
- **D2 Signing:** the macOS build is signed and notarised in CI with the five Apple secrets, once they are added to this repository (a `garycoding` repository does not receive ParkviewLab's organisation secrets). Amended the same day: v0.1.0 ships unsigned.
- **D3 Merges:** the session merges each pull request once its checks have passed; v0.1.0 was to be released without a further review.
- **D4 Persistence:** the conversation is kept in the data folder and survives a restart.
- **D5 Compaction:** the last four exchanges are kept word for word.
- **D6 System prompt:** a default asking for short answers in plain spoken sentences, editable in the settings.
- **D7 Identity:** product name Peabody; binary `agent-peabody`; bundle identifier `io.github.garycoding.agent-peabody`.
- **D8 Queueing:** utterances that arrive whilst Peabody answers are answered in order.
- **D9 Listening:** Peabody holds a graptostream listen request for as long as it runs.
- **D10 Testing:** LM Studio on pacific with `google/gemma-4-12b`; chimchim is not used in building or testing.
- **D11 Reasoning:** only `content` is spoken and kept; an empty answer is spoken as "I have no answer to that." and shown as a problem.

Instant replies, by the owner on 2026-10-31:

- **D12 Instant replies:** on each utterance Peabody says at once one of a list of short phrases, then asks the model and speaks its answer as before, since on a slow machine the answer takes several seconds and the silence leaves the speaker unsure whether Peabody heard. The four default phrases are the owner's. Peabody chooses one at random, never the one said last; the list is editable in the settings; the phrase is not kept in the conversation.
- **D13 Kept speech:** the phrases must play at once, so phonorheos (from v0.3.0) keeps speech when asked: each `POST /speak` carries `speak` and `cache`. Peabody sends every phrase with `cache` set and `speak` unset when it starts, so that phonorheos makes them unheard, and the chosen phrase with both set when it is wanted; a phrase phonorheos has lost by restarting is made then and kept again. There is no periodic refresh and no check for an older phonorheos: the owner runs a matching version.

Thinking, by the owner on 2026-10-31:

- **D14 Thinking off:** a setting, off by default, decides whether a reasoning model may think before it answers; off, every chat request (answers and summaries alike) carries `"reasoning_effort": "none"`. The owner asked for this because answers were slow. Evidence on pacific's LM Studio with `google/gemma-4-12b`, one question asking for one sentence: no field, 19.9 s and 274 reasoning tokens; `reasoning_effort` `"none"`, 2.1 s and none; `"low"`, 18.8 s and 268; `chat_template_kwargs.enable_thinking` false, 16.7 s and 234; `reasoning.effort` `"none"`, 18.6 s and 260. Whether llama.cpp's server honours the field was not checked.

Tools, by the owner on 2026-10-31:

- **D15 Peabody runs the tools:** web search through aqua-roaming (the lab's SearXNG MCP server on trixie) is offered by Peabody itself, as an MCP client over Streamable HTTP, rather than by LM Studio. LM Studio runs MCP tools only through its own `/api/v1/chat`, which needs "Require Authentication" and "Allow calling servers from mcp.json" on, documents no message roles for its input, so it cannot take Peabody's kept conversation, and is LM Studio's alone. Peabody offers the tools in the standard chat request's `tools`, which Gemma 4 in LM Studio answered with a correct `search` call (checked 2026-10-31), and the same request should serve llama.cpp (not checked). Speed did not decide it: by reasoning, not measurement, either route spends the same model and search time.
- Within that ruling: at most four rounds of tool calls a question, then a request with no tools; results cut at 8000 characters (about 2000 tokens), since a fetched page can be 50 000; calls and results not kept in the conversation. The token count after a tool-using answer includes the results, so the conversation is summarised slightly early rather than late.

Tools and controls, by the owner on 2026-10-31:

- **D16 Tools in the instructions:** with tools available, Peabody appends to the system prompt a list of them, each with the description its MCP server gives, and nothing else: no date, no instruction to search; the model decides from the descriptions. Evidence before the ruling (pacific, Gemma 4 12B, the owner's 81-exchange conversation from before tools existed): with no conversation the model searched every time; with that conversation it refused ("I do not have access to real-time weather data") with the list, with the refusals removed, and with both. The owner ruled that the refusals stay and that the conversation is not changed by Peabody.
- **D17 Several servers:** any number of MCP servers, each an address and an optional token; v0.4.0's single server is read as the first. A tool whose name an earlier server already offers is given the server's name in front. One status dot covers them all and names any that fail; the others are still used.
- **D18 Controls:** an On/Off button (off: no listen request, nothing heard; kept across restarts), Clear (the whole conversation, after a second click) and Compact (a summary now, keeping the last four exchanges), and the context window's fullness as a percentage with a bar.

- **D19 Model traffic shown:** every request to the model and every reply is a line in the window, collapsed to a summary and opened by a click, so that what the model was given and what it returned can be checked (asked for after the tool tests, 2026-10-31). Each line's label carries its time; the speaker's label is "Input", not "You".

- **Fresh connections to MCP servers (v0.5.1):** a weather call sent 5 s after the tool list never reached aqua-roaming: uvicorn closes an idle connection after 5 s, and Peabody reused the pooled connection just as it closed ("error sending request"). MCP requests now go on a fresh connection each (about a millisecond on the LAN), and a request that fails to send, or whose connection closes unanswered, is sent once more.

- **Commands do not wait (v0.5.2):** Clear did nothing for minutes while Peabody's question waited behind another client's requests to LM Studio, since commands ran only between answers. A command now abandons the question in hand (an unanswered question is not kept) and runs at once.

- **D20 Hush (v0.6.0):** the owner interrupts Peabody with "<spoken name> hush". graptostream (v0.3.0) decides who is interrupted and sends an `event: hush` to that app, from provisional text, and does not address "<name> hush" utterances; Peabody decides what it means: it withdraws its own phonorheos items, abandons an answer in progress, marks a cut-off answer "(interrupted)", and ignores a hush more than 2 s old (graptostream replays missed events after a reconnection). "Stop" was rejected because it begins real requests ("stop the build"); echo cancellation because Peabody may speak through any machine on the LAN and Peabodies are to hear one another. Probe tests (2026-10-31): stopped within milliseconds of the words being heard, also mid-sentence; the name was heard 1 in 3 times as "Peabody", about 5 in 7 as "Jarvis", 9 in 9 as "Aristotle".
- **Gemma's channel markers stripped (v0.6.0):** an answer began "<|channel>s_thought<channel|>…" and phonorheos read the markers aloud; they are now removed like a `<think>` block.

- **D21 Short tool results kept (owner, 2026-10-31):** a tool result of 500 characters or less is kept in the conversation after the answer it served, as "[<tool> said: …]". Asked the time in Seoul, the model answered "10:45 a.m." from `current_time`, leaving out that it was already Sunday there; asked "what day is it there?", it had only its own answer and said Saturday. Long results (search, fetch) stay out, to spare the context window.

- **Models request carries a version header (v0.7.1):** connecting to Anthropic (2026-10-31), "List models" and the periodic model check failed with 400 "anthropic-version: header is required", although Anthropic's page calls the header optional and chat works without it; the models request now sends `anthropic-version: 2023-06-01`, which LM Studio and llama.cpp ignore. The window's top bar also sits above the list (`z-index`), since the time labels, which have `opacity`, painted over it when the list scrolled under it.

- **D22 Rounds, last answer, thinking (v0.7.2):** one question to read Kipling's "If" took a refused page, a search, another refused page and a page that worked, which used all four tool rounds, and the model's last reply, asked for without tools, came back empty after 8,586 tokens. Tool rounds are now eight; on the last request the model is told, for that request only, that its tool calls are used up and to answer from what it found; an empty reply is explained in the window (the reply's `finish_reason`, shown in the opened "From model" line), and the spoken "I have no answer to that" stays. With thinking off, a request to Anthropic's own address also carries `thinking: {"type": "disabled"}`, because Claude 5 models think by default through its OpenAI-compatible layer, which ignores `reasoning_effort`; if the server answers 400 naming `thinking`, the request is sent again without it.
- **D23 A longest reply (v0.8.0):** on LM Studio with Gemma 4 and a long conversation carried over from Anthropic's Sonnet, two requests produced no answer for four and three minutes (LM Studio logged "Start thinking" several times a second), though Peabody asked for no thinking (`reasoning_effort: "none"`); five replays of the rebuilt request, which LM Studio counted at the same 2,891 prompt tokens, answered in two to three seconds, so the cause is unknown and the loop intermittent. The same history made the model answer without calling the weather tool (eight of eight requests); a cleared conversation called it correctly. Requests had no length limit and Peabody waits fifteen minutes. A setting, "Longest reply (tokens)", is now sent as `max_tokens`: 2048 by default, about three times the longest reply seen (some 650 tokens by character count, an estimate) and ten minutes of speech at 150 words a minute; 0 sets none. A reply cut at it is spoken as far as it goes, with a notice in the window. Not chosen: 1,024, which would end a loop sooner but cut a long poem.

Made during the build, within these rulings:

- The tokens counted towards the window exclude the reasoning's, since the reasoning is not kept; counting them would summarise far too soon (found in the live test).
- When the model cannot be reached, Peabody says "I could not get an answer from the model." and does not keep the unanswered question, so that the exchanges stay paired.
- The voices are phonorheos's 28 Kokoro voices, listed in Peabody, since phonorheos has no endpoint that lists them.
- A graptostream stream silent for 45 s (three missed keep-alives) is reconnected, since a LAN link that drops without closing would otherwise leave Peabody deaf; a chat request is given up after 15 minutes, well beyond the eight minutes a long answer took on pacific (from the code review).
