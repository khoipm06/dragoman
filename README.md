# Dragoman

> Dragoman: A local OpenAI-compatible bridge written in Rust. Routes requests from coding agents (Cline, Aider, OpenHands, Codex) to Gemini Web (`gemini.google.com`) to query the Gemini 4 Argon model with a Google AI Pro subscription.

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Rust](https://img.shields.io/badge/Rust-2024_Edition-orange.svg)](https://www.rust-lang.org/)

---

## Core Features

- Direct Gemini 4 Argon access: Routes queries to Gemini 4 Argon (`Argon4p0`) through web sessions.
- Zero browser runtime: Uses native HTTP/2 direct RPC (`StreamGenerate`). Chrome does not run in the background during normal use.
- Standard OpenAI API compatibility: Implements `/v1/chat/completions`, `/v1/models`, and `/health`.
- Real-time streaming: Server-Sent Events (SSE) deliver chunked token responses as they generate.
- Tool calling translation: Formats tool schemas into instructions and parses `<tool_call>` outputs into standard OpenAI `tool_calls` structures.
- No API tokens required: Works with personal Google AI Pro subscriptions without Google Cloud billing.

---

## Architecture

```text
External Coding Agent (Cline / Aider / OpenHands / Codex)
  │
  │ HTTP POST /v1/chat/completions (OpenAI JSON + Tool Schemas)
  ▼
Dragoman Proxy Server (Rust / Axum on http://127.0.0.1:8080)
  │
  ├── 1. PromptTranslator: Maps chat history and tool schemas into prompts
  ├── 2. GeminiWebClient: Direct HTTP/2 with OpenSSL
  │      └── Loads session tokens from ~/.config/dragoman/session.json
  ▼
Google Gemini Backend (assistant.lamda.BardFrontendService/StreamGenerate)
  │
  ├── Targets internal model: Argon4p0 (Gemini 4 Argon)
  │
  ▼
Real-Time Stream Parser
  │
  └── Emits standard OpenAI SSE chunks to the agent
```

> Browser usage: Chrome runs only once during `dragoman login` to read session cookies and the anti-CSRF token (`SNlM0e`). Dragoman saves these credentials locally and shuts down Chrome.

---

## Quickstart

### 1. Build from Source

Requirements: Rust edition 2024 (version 1.85 or later).

```bash
git clone https://github.com/khoipm06/dragoman.git
cd dragoman
cargo build --release
```

The compiled binary is written to `./target/release/dragoman`.

### 2. Initial Authentication

Run the login command once to link your Google AI Pro account:

```bash
./target/release/dragoman login
```

Steps performed:
1. Dragoman starts Chrome with remote debugging.
2. Log in at [gemini.google.com](https://gemini.google.com).
3. Dragoman reads the session tokens and saves them to `~/.config/dragoman/session.json`.
4. Close Chrome when prompted.

### 3. Verify Direct RPC Connection

Test communication with Gemini 4 Argon:

```bash
./target/release/dragoman test-rpc "State your model identity in one sentence."
```

### 4. Start the Standalone Proxy Server

Launch the proxy server:

```bash
./target/release/dragoman serve --port 8080
```

The server listens on `http://127.0.0.1:8080`.

---

## Agent Configuration

Point your AI coding tool to Dragoman with these settings:

| Setting | Value |
| :--- | :--- |
| **API Base URL** | `http://127.0.0.1:8080/v1` |
| **Model** | `gemini-4-argon` |
| **API Key** | `dragoman-local` (or any string) |

### Example: Non-Streaming Request

```bash
curl http://127.0.0.1:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "gemini-4-argon",
    "messages": [
      {"role": "user", "content": "Explain what a dragoman was in history in 20 words."}
    ],
    "stream": false
  }'
```

### Example: Streaming Request (SSE)

```bash
curl -N http://127.0.0.1:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "gemini-4-argon",
    "messages": [
      {"role": "user", "content": "Write a Rust function that computes Fibonacci numbers."}
    ],
    "stream": true
  }'
```

---

## CLI Reference

```text
Usage: dragoman [COMMAND]

Commands:
  serve     Start the standalone OpenAI-compatible proxy server (no browser needed)
  login     Perform one-time browser login to extract and save session credentials
  test-rpc  Test direct native HTTP RPC without browser
  help      Print this message or the help of the given subcommand(s)

Options:
  -h, --help     Print help
  -V, --version  Print version
```

---

## Project Structure

```text
dragoman/
├── Cargo.toml          # Rust crate manifest
├── LICENSE             # MIT License
├── README.md           # Documentation
└── src/
    ├── main.rs         # CLI entry point (serve, login, test-rpc)
    ├── browser/        # Chrome launcher and CDP client (used only during login)
    │   ├── cdp.rs
    │   ├── launcher.rs
    │   └── mod.rs
    ├── openai/         # OpenAI API data structures and schemas
    │   ├── mod.rs
    │   └── types.rs
    ├── proxy/          # Axum HTTP proxy and tool translator
    │   ├── mod.rs
    │   ├── server.rs
    │   └── translator.rs
    ├── session.rs      # Session credential persistence
    └── web/            # Direct HTTP/2 StreamGenerate RPC client
        ├── client.rs
        └── mod.rs
```

---

## License

This project is licensed under the [MIT License](LICENSE).
