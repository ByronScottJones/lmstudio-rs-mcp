# lmstudio-rs-mcp Engineering Guide

Handover notes and development specification for the engineer or agent who
continues this project. Read it before making changes, and update it in the
same change set whenever architecture, decisions, contracts, verification
status, or known risks change.

Audience: contributors and coding agents. Agents start from
[AGENTS.md](AGENTS.md), which routes to this file and to the project-level
guidance in [`agents/`](agents/). User-facing documentation lives in
[README.md](README.md); contribution workflow lives in
[CONTRIBUTING.md](CONTRIBUTING.md); vulnerability reporting lives in
[SECURITY.md](SECURITY.md).

## 1. How to Develop in This Repository (Spec-Driven)

Specification comes first. Every behavior change follows the same loop:

1. **Specify.** Write or amend the requirement in
   [section 4](#4-requirements) (or the contract in
   [section 5](#5-contracts)) before touching code. State the observable
   behavior, inputs, outputs, error codes, and an acceptance criterion that
   can be checked by a test.
2. **Test.** Add the test that encodes the acceptance criterion and watch it
   fail. Put it at the narrowest useful layer (see
   [section 10](#10-verification)).
3. **Implement.** Make the smallest change that satisfies the spec. Match
   existing conventions; keep unrelated refactors out of the change.
4. **Verify.** Run `make check` (format, clippy with warnings as errors,
   tests). For changes that touch provider request/response handling, also
   verify against a real running backend; several bugs in this project's
   history were only caught that way.
5. **Record.** Update this file (requirements, contracts, decisions,
   verified table, risks) and the README in the same change set.

### Definition of done

- The requirement or contract exists in this document and the code matches it.
- A test covers the acceptance criterion, including the failure path.
- `make check` passes, or the exact gap is recorded in
  [section 10.3](#103-verification-status).
- Every new or changed tool declares all four annotation hints
  (see [section 5.3](#53-tool-catalog)).
- README, this file, and tool descriptions agree with the code.
- Work is on a branch and merged by pull request; direct pushes to `main` are
  not used.

### Traceability

Requirement IDs (`FR-*`, `NFR-*`) appear in section 4 together with the tests
that prove them. When you add a test, name the requirement it covers in the
requirement's "Verified by" line. A requirement with no test is a known gap
and must be listed in [section 11](#11-risks-gaps-and-future-work).

## 2. What This Is

`lmstudio-rs-mcp` is a Model Context Protocol (MCP) server written in Rust. It
lets an MCP client (Claude Code, Claude Desktop, and others) use a language
model backend through tools:

- **Inference:** chat, raw text completion, embeddings, and stateful
  multi-turn conversations.
- **Model management** (LM Studio and Ollama): list, load, unload, inspect.
- **Delegation:** hand a well-scoped task to a model acting as a sandboxed
  subagent with its own file and shell tools.
- **LM Studio environment:** detect the installation, the `lms` CLI, and the
  REST API; run the `lms` features the REST API lacks.
- **Feedback:** draft and file GitHub issues about this server through a
  human-reviewed flow.

The backend is chosen with `LLM_PROVIDER`: LM Studio (default), Ollama,
OpenAI, or Anthropic. It runs on macOS, Windows, and Linux over stdio.

### Non-goals

- It is not a general MCP gateway or proxy to other MCP servers.
- It does not expose arbitrary `lms` or shell access to the MCP client
  (see [section 8.1](#81-lms-cli-and-environment-tools)); the subagent shell
  is a separate, capability-gated feature.
- It does not persist conversations or model state beyond the feedback file.
- It does not translate every provider feature; unsupported combinations
  return a clear `ToolResult`, not an emulation.

## 3. Tech Stack

| Concern | Choice |
| --- | --- |
| Language | Rust, edition 2021; minimum 1.80 (`std::sync::LazyLock`) |
| MCP SDK | [`rmcp`](https://github.com/modelcontextprotocol/rust-sdk) 3.x (server, macros, stdio transport, schemars, elicitation) |
| Async runtime | `tokio` (multi-thread, process, net, signal, time, io-util) |
| HTTP | `reqwest` 0.12 with `rustls-tls`, no OpenSSL |
| Serialization and schema | `serde`, `serde_json`, `schemars` 1 |
| Logging | `tracing` to stderr (stdout is the MCP protocol stream) |
| Errors | `thiserror` for typed errors, `anyhow` in `main` |
| Misc | `dirs`, `chrono`, `regex`, `uuid`, `futures-util`, `url` |
| Release profile | `opt-level = "z"`, LTO, strip, one codegen unit |

Pin and review dependencies through `Cargo.lock`; `make deps` and every
build use `--locked`.

## 4. Requirements

Each requirement has an acceptance criterion and the tests that prove it.
"Verified by" names unit tests (`module::tests::name`) or integration tests
(`tests/mcp_stdio.rs`). Statements marked **Gap** have no automated test.

### 4.1 Functional requirements

**FR-1 Uniform result envelope.** Every tool returns
`{ success, message, data?, error? }`. On success `error` is absent; on
failure `data` is absent and `error` is `{ code, message }`.

- Acceptance: for every tool, a call returns an object with boolean `success`
  and string `message`, never a protocol-level error for a tool-level failure.
- Verified by: `types::tests::tool_result_ok_serializes_without_error_field`,
  `types::tests::tool_result_err_serializes_without_data_field`, and the
  envelope check applied to every tool call in `tests/mcp_stdio.rs`
  (for example `provider_tools_report_connection_failures_in_the_envelope`).

**FR-2 Provider selection.** `LLM_PROVIDER` selects `lmstudio`, `ollama`,
`openai`, or `anthropic` (aliases: `lm-studio`, `lm_studio`, `open-ai`,
`claude`). Unknown values warn and fall back to `lmstudio`. `openai` and
`anthropic` refuse to start without an API key.

- Verified by: `providers::tests::parses_known_names_case_insensitively`,
  `providers::tests::rejects_unknown_names`,
  `config::tests::openai_without_an_api_key_is_a_hard_error`.

**FR-3 Provider capability gating.** Tools a provider cannot support return a
normal `ToolResult` explaining why (success for reads such as
`list_loaded_models`, an error for mutations such as `load_model`), never a
raw HTTP error. The matrix is in [section 5.4](#54-provider-capability-matrix).

- Verified by: `providers::tests` capability tests (for example
  `anthropic_is_the_only_one_without_embeddings`).
- **Gap:** per-tool gating responses for cloud providers are covered only
  through the capability predicates, not end to end.

**FR-4 Connectivity.** `health_check` reports whether the configured provider
answers, with `CONNECTION_FAILED` when it does not.

- Verified by: `provider_tools_report_connection_failures_in_the_envelope`.

**FR-5 Model management.** For LM Studio and Ollama, the server can list the
library, list loaded instances, identify the current model, describe one
instance, load, and unload. Identifier and library key are distinct: tools
that route by instance use the instance identifier.

- Verified by: `tools::models::tests` (loaded-instance flattening, summary,
  tolerant parsing of future response shapes, auto-detection).

**FR-6 Model auto-detection.** When a generation tool omits `model`, the
server uses the single loaded instance's identifier. Zero loaded instances
and more than one loaded instance are `MODEL_NOT_LOADED`; a client failure
keeps its own error code instead of being flattened into `MODEL_NOT_LOADED`.

- Verified by: `tools::models::tests::auto_detect_*`.

**FR-7 Inference.** `chat_completion`, `text_completion`, and
`generate_embeddings` work against supporting providers. Generation is
streamed internally and reassembled into one result (see
[section 8.4](#84-streaming-and-timeouts)). A reasoning model that exhausts
its token budget returns a success carrying `reasoning_content` and an
explanatory message.

- Verified by: `sse::tests`, `client::tests`, `tools::responses::tests`.
- **Gap:** the happy path against a live backend is not in the automated
  suite; see [section 10.3](#103-verification-status).

**FR-8 Stateful conversations.** `create_response`, `start_conversation`, and
`continue_conversation` work on providers with a `previous_response_id`
Responses API (LM Studio, OpenAI). The persona from `start_conversation` is
re-sent on every turn and re-keyed under each new response ID.

- Verified by: `tools::responses::tests`.

**FR-9 Subagent.** `run_subagent` runs a bounded tool-calling loop
(default 15 turns, clamped 1 to 50) over a working-directory sandbox with
capability tiers `read_only` (default), `read_write`, and
`read_write_shell`, and returns one final report.

- Verified by: `subagent::tools`, `subagent::guard`, and `subagent::runner`
  unit tests; the failure path in
  `provider_tools_report_connection_failures_in_the_envelope`.

**FR-10 Feedback.** `feedback_*` tools manage drafts in
`~/.lmstudio-rs-mcp/feedback.json`. `feedback_submit` checks for a duplicate
issue first, never files an issue through the API, and always requires a
human to review and click create.

- Verified by: `feedback::store::tests`, `feedback::github::tests`,
  `feedback_tools_round_trip_through_the_local_store`,
  `feedback_submit_and_duplicate_check_fail_cleanly_for_an_unknown_entry`.
- **Gap:** the elicitation and browser-fallback flow is not automated.

**FR-11 LM Studio status.** `lmstudio_status` reports installation, CLI, and
API availability independently and always succeeds. Installation covers the
app path, the app version where readable (macOS), and `~/.lmstudio`. The CLI
finding includes the path and the build commit (the CLI has no semantic
version). The API finding is skipped when the provider is not LM Studio.

- Verified by: `tools::lms::tests`, `lms::tests`,
  `lmstudio_status_reports_each_finding_independently`.

**FR-12 `lms` CLI access.** `lms_cli` runs one command from a fixed allowlist
(see [section 8.1](#81-lms-cli-and-environment-tools)), with extra `args`.
Commands that would otherwise prompt or fail without an argument are
rejected up front. Anything outside the allowlist is rejected at the protocol
layer.

- Verified by: `lms::tests` (argv construction, allowlist, required
  arguments), `lms_cli_reports_a_missing_cli_and_validates_input_without_running_anything`.

**FR-13 Tool annotations.** Every tool declares `readOnlyHint`,
`destructiveHint`, `idempotentHint`, and `openWorldHint` as explicit
booleans that match its behavior.

- Verified by: `every_tool_declares_all_four_annotation_hints_as_booleans`,
  `annotations_match_what_the_tools_actually_do`.

### 4.2 Non-functional requirements

**NFR-1 Stdout is the protocol.** Nothing but MCP messages is written to
stdout. All logging goes to stderr.

- Verified by: every `tests/mcp_stdio.rs` case parses stdout as JSON-RPC.

**NFR-2 Bounded time.** No call waits forever. Defaults are in
[section 5.6](#56-timeouts-and-limits). Streaming calls use an idle timeout
that resets on every chunk plus an outer maximum.

- Verified by: `sse::tests`, `lms::tests::run_kills_and_reports_a_timeout`.

**NFR-3 Bounded memory.** CLI output is read into buffers capped at 64 KiB
per stream while draining the pipe; subagent file and command I/O is capped at
200,000 bytes.

- Verified by: `lms::tests::read_capped_bounds_memory_but_drains_to_eof`,
  `lms::tests::run_truncates_large_output_without_blocking_the_child`.

**NFR-4 No shell for `lms`.** The `lms` binary is spawned with an argument
vector, never through a shell, with stdin closed.

- Verified by: `lms::tests::run_gives_the_child_a_closed_stdin`,
  `lms::tests::command_names_are_snake_case_on_the_wire`.

**NFR-5 Cross-platform.** Builds and tests on macOS (arm64, x86_64), Windows,
and Linux in CI. No OpenSSL dependency.

- Verified by: CI matrix (see [section 10.3](#103-verification-status)).

**NFR-6 Secrets stay out of output.** API keys and tokens are never logged or
returned in tool results.

- **Gap:** enforced by convention and review, not by a test.

**NFR-7 Tests are hermetic.** The integration suite needs no backend,
network, real `lms`, or access to the developer's home directory.

- Verified by: environment isolation in `tests/mcp_stdio.rs`.

## 5. Contracts

### 5.1 Result envelope

```json
{ "success": true, "message": "human readable", "data": {} }
{ "success": false, "message": "human readable",
  "error": { "code": "CONNECTION_FAILED", "message": "detail" } }
```

The envelope convention is carried over from the TypeScript
`lm-studio-mcp-server` project so clients handle success and failure
uniformly.

### 5.2 Error codes

Serialized in `SCREAMING_SNAKE_CASE`. New codes go in `src/types.rs`.

| Code | Meaning |
| --- | --- |
| `MODEL_NOT_FOUND` | HTTP 404 from the backend, or an unknown model |
| `MODEL_NOT_LOADED` | No loaded instance, several loaded when one was needed, or an unknown identifier |
| `CONNECTION_FAILED` | Backend unreachable or the stream closed early |
| `UNAUTHORIZED` | HTTP 401 or 403 |
| `INVALID_INPUT` | Bad arguments, or an operation the provider does not support |
| `CLI_NOT_FOUND` | The `lms` CLI could not be located |
| `LOAD_FAILED` | `load_model` failed |
| `UNLOAD_FAILED` | Defined but not currently returned; `unload_model` maps failures to `MODEL_NOT_LOADED` or the client error code |
| `TIMEOUT` | A bounded wait elapsed |
| `UNKNOWN` | Anything else |

### 5.3 Tool catalog

Columns under Hints are R (read-only), D (destructive), I (idempotent),
O (open world: may reach a network service). A dash means false. A leading
`*` marks a required input. The test
`annotations_match_what_the_tools_actually_do` enforces the intent behind
this table; update both together.

| Tool | Hints R D I O | Inputs | Providers |
| --- | --- | --- | --- |
| `health_check` | R - I O | none | all |
| `lmstudio_status` | R - I O | none | all (API check LM Studio only) |
| `lms_cli` | - D - O | `*command`, `args`, `timeout_seconds` | LM Studio host |
| `list_models` | R - I O | none | all |
| `list_loaded_models` | R - I O | none | LM Studio, Ollama |
| `get_current_model` | R - I O | none | LM Studio, Ollama |
| `get_model_info` | R - I O | `*identifier` | LM Studio, Ollama |
| `load_model` | - - - O | `*model`, `context_length`, `flash_attention`, `eval_batch_size` | LM Studio, Ollama |
| `unload_model` | - D - O | `*identifier` | LM Studio, Ollama |
| `chat_completion` | R - - O | `*prompt`, `system_prompt`, `model`, `temperature`, `max_tokens` | all |
| `text_completion` | R - - O | `*prompt`, `model`, `temperature`, `max_tokens`, `stop_sequences` | all but Anthropic |
| `generate_embeddings` | R - - O | `*text`, `model` | all but Anthropic |
| `create_response` | - - - O | `*input_text`, `previous_response_id`, `reasoning_effort`, `temperature`, `max_output_tokens`, `model` | LM Studio, OpenAI |
| `start_conversation` | - - - O | `*system_prompt`, `*first_message`, `temperature`, `max_output_tokens`, `model` | LM Studio, OpenAI |
| `continue_conversation` | - - - O | `*response_id`, `*message`, `temperature`, `max_output_tokens`, `model` | LM Studio, OpenAI |
| `run_subagent` | - D - O | `*task`, `*working_directory`, `capability`, `max_turns`, `model`, `system_prompt`, `temperature` | all |
| `feedback_create` | - - - - | `*category`, `*title`, `*body` | local |
| `feedback_list` | R - I - | none | local |
| `feedback_get` | R - I - | `*id` | local |
| `feedback_update` | - - I - | `*id`, `category`, `title`, `body` | local |
| `feedback_delete` | - D I - | `*id` | local |
| `feedback_check_duplicates` | R - I O | `*id`, `repo`, `token` | GitHub |
| `feedback_submit` | - D - O | `*id`, `repo`, `token` | GitHub, browser |

Annotation rationale where it is a judgment call:

- Generation and embeddings are marked read-only because they do not change
  the environment, even though LM Studio can load a model on demand.
- `unload_model`, `run_subagent`, `feedback_delete`, and `feedback_submit`
  are destructive; `feedback_submit` can discard a local draft when it finds
  a duplicate issue.
- `lms_cli` is destructive as a whole because the allowlist includes
  commands that stop the server or log out; individual commands are
  classified by `LmsCommand::is_mutating`.
- Only the feedback store tools are closed-world.

### 5.4 Provider capability matrix

Source of truth: `src/providers.rs`.

| Capability | LM Studio | Ollama | OpenAI | Anthropic |
| --- | --- | --- | --- | --- |
| Default base URL | `http://127.0.0.1:1234` | `http://127.0.0.1:11434` | `https://api.openai.com` | `https://api.anthropic.com` |
| API key | optional | optional | required | required |
| Wire format | OpenAI-compatible | OpenAI-compatible (native for models) | OpenAI-compatible | Anthropic `/v1/messages` |
| Model management | yes | yes | no | no |
| `/v1/responses` with `previous_response_id` | yes | no | yes | no |
| Embeddings | yes | yes | yes | no |
| Raw text completion | yes | yes | yes | no |

Anthropic authenticates with `x-api-key` plus a required `anthropic-version`
header rather than a bearer token.

### 5.5 Configuration

Read from the environment at connect time, so a client that sets variables in
its MCP launch config always wins.

| Variable | Default | Purpose |
| --- | --- | --- |
| `LLM_PROVIDER` | `lmstudio` | Backend selection (FR-2) |
| `LLM_BASE_URL` | provider default | `scheme://host:port`, no trailing slash, no `/v1` suffix |
| `LLM_API_KEY` | none | Bearer or API key; required for OpenAI and Anthropic |
| `LMSTUDIO_BASE_URL`, `LMSTUDIO_HOST`, `LMSTUDIO_PORT`, `LMSTUDIO_API_TOKEN` | none | Legacy LM Studio variables, still honored |
| `LMS_PATH` | auto-detect | Full path to the `lms` binary; an invalid path is an error, never a silent fallback |
| `GITHUB_TOKEN` | none | Duplicate check for a private repo, after the `token` argument and before `gh auth token` |
| `RUST_LOG` | `info` | `tracing` filter, written to stderr |

Add a new variable to `src/config.rs`, the table above, and the README
together. Never hardcode credentials or environment-specific values.

### 5.6 Timeouts and limits

| Limit | Value | Where |
| --- | --- | --- |
| Ordinary API call | 30 s | `DEFAULT_TIMEOUT` |
| Model load | 300 s | `LOAD_MODEL_TIMEOUT` |
| Embeddings call | 180 s | `INFERENCE_TIMEOUT` |
| Stream idle (reset per chunk) | 60 s | `STREAM_IDLE_TIMEOUT` |
| Stream outer maximum | 900 s | `STREAM_MAX_DURATION` |
| `lms version` probe | 15 s | `tools/lms.rs` |
| `lms_cli` default / maximum | 120 s / 3600 s | `tools/lms.rs` |
| `lms_cli` extra arguments | at most 32, no NUL | `lms::validate_extra_args` |
| CLI output per stream | 64 KiB | `lms.rs` |
| Subagent file I/O and command output | 200,000 bytes | `subagent/tools.rs` |
| Subagent command timeout | 60 s | `subagent/tools.rs` |
| Subagent turns | default 15, clamp 1 to 50 | `tools/subagent.rs` |
| Elicitation wait | 120 s | `tools/feedback.rs` |

## 6. Architecture

### 6.1 File map

| Path | Purpose |
| --- | --- |
| `src/main.rs` | Entry point: logging to stderr, config, stdio transport, signal shutdown |
| `src/server.rs` | One `#[tool]` method per tool, with annotations; wires to `tools::*` |
| `src/client.rs` | HTTP client over every provider, plus Ollama and Anthropic translation |
| `src/providers.rs` | `Provider` enum and capability matrix |
| `src/config.rs` | Environment configuration |
| `src/types.rs` | `ToolResult`, `ErrorCode`, `ClientError`, timeout constants |
| `src/sse.rs` | Server-Sent Events reader with idle and maximum timeouts |
| `src/lms.rs` | Locate and run `lms`, install detection, `LmsCommand` allowlist |
| `src/tools/health_check.rs` | `health_check` |
| `src/tools/lms.rs` | `lmstudio_status`, `lms_cli` |
| `src/tools/models.rs` | Model listing and management, auto-detection |
| `src/tools/chat.rs` | `chat_completion`, `text_completion` |
| `src/tools/embeddings.rs` | `generate_embeddings` |
| `src/tools/responses.rs` | Stateful conversations and the persona cache |
| `src/tools/subagent.rs` | `run_subagent` MCP wrapper |
| `src/tools/feedback.rs` | `feedback_*` MCP wrapper and the elicitation flow |
| `src/subagent/tools.rs` | Sandboxed `read_file`, `list_directory`, `search_files`, `write_file`, `run_command` |
| `src/subagent/guard.rs` | Non-bypassable high-risk command pattern guard |
| `src/subagent/runner.rs` | The tool-calling loop and final report |
| `src/feedback/store.rs` | Local JSON CRUD for feedback entries |
| `src/feedback/github.rs` | Duplicate-issue check and token resolution |
| `tests/mcp_stdio.rs` | End-to-end tests over the real binary |
| `AGENTS.md` | Always-active agent guidance for this project: essentials, topic routing, general rules |
| `agents/*.md` | Project-level copies of the shared guidance in `~/.agents` (git, coding, rust, testing, markdown, makefile, engineering-workflow, security, configuration) |
| `Makefile`, `tasks/Makefile.lint` | Task runner (see [section 9](#9-build-run-and-validate)) |
| `.github/workflows/ci.yml` | CI: format, clippy, release build, tests on four targets |
| `.github/` templates | Issue and pull request templates |
| `Test_Run_Results.md` | Record of a manual test run against LM Studio |
| `assets/`, `.well-known/` | README imagery and a publisher verification file |

### 6.2 Data flow

1. The MCP client launches the binary and speaks JSON-RPC over stdio.
2. `server.rs` deserializes tool arguments into the input structs in
   `tools/*`, whose `JsonSchema` derivations publish the input schemas.
3. A tool function in `tools/*` calls `ApiClient` (HTTP providers), the
   subagent runner, the feedback store, or `lms.rs` (local CLI).
4. Provider differences are absorbed in `client.rs`, so tools see one shape.
5. The tool returns `ToolResult<T>`, which `server.rs` wraps in `Json` so the
   client receives `structuredContent` plus text.

### 6.3 Module rules

- Tool functions take typed inputs and return `ToolResult<T>`; they never
  panic on bad input and never return a protocol error for a tool failure.
- Provider-specific behavior stays in `client.rs` and `providers.rs`.
- Everything that spawns a process goes through one runner per concern
  (`lms::run` for `lms`; the subagent runner for `run_command`).
- `.unwrap()` and `.expect()` are for tests and `main` only.

## 7. Key Decisions and Rationale

| Decision | Why | Rejected alternatives |
| --- | --- | --- |
| Rust with the official `rmcp` SDK | Single static binary, easy cross-platform distribution | TypeScript or Python bridge (the originals); heavier runtime |
| `rustls`, no OpenSSL | Cross-compiles cleanly; no system dependency | `native-tls` |
| Uniform `ToolResult` envelope | One success and failure handling path for every client | Per-tool shapes; protocol errors for tool failures |
| Translate Ollama and Anthropic into LM Studio's shapes | Tools stay provider-agnostic | Per-provider tool variants |
| Always stream generation internally | Idle timeout instead of total timeout; slow reasoning models are not killed, hung connections are | A single long total timeout |
| Reasoning-budget exhaustion is a success with the trace | The caller can see why and retry with a larger budget | An opaque error |
| Tolerant response parsing (`#[serde(default)]`, `extra` catch-all) | LM Studio's `/api/v1` is newer and less documented; new fields must not break parsing | Strict structs |
| `feedback_submit` never files via the API | A human reviews and clicks create; avoids unreviewed outbound posts | Direct issue creation |
| Subagent guard is code, not prompt | A small local model cannot be trusted to follow prompt rules alone | Prompt-only restrictions |
| `lms_cli` uses a closed enum allowlist | The caller cannot reach commands we did not choose | Passing any subcommand or arbitrary arguments through |
| No shell, stdin closed for `lms` | No shell injection; a command that wants a prompt fails fast | `sh -c`; inheriting stdin |
| `runtime remove` excluded | Destructive and irreversible | Including it behind a confirmation |
| `chat`, `log stream`, `dev`, `push` excluded | Interactive, unbounded, or publishing outward | Timed `log stream` (possible future work) |
| `ls`, `ps`, `load`, `unload` not in `lms_cli` | The REST API tools already cover them | Duplicating them |
| Required-argument check for prompting commands | The CLI otherwise opens an interactive picker that cannot be answered | Letting the CLI fail on its own |
| CLI version reported as a build commit | `lms` prints no semantic version | Parsing the app version on every platform |
| `lmstudio_status` never fails as a whole | A missing CLI must not hide a working API, and the reverse | Failing fast on the first problem |
| `LMS_PATH` that does not exist is an error | A misconfiguration must not silently run a different binary | Falling through to `PATH` |
| All four annotation hints explicit | Hosts and directories may reject tools missing any hint | Relying on protocol defaults |
| `make deps` fetches host-platform crates only | Works offline from a warm cache; avoids downloading other platforms' crates | Plain `cargo fetch` |

## 8. Detailed Specifications

### 8.1 `lms` CLI and environment tools

**Locating the CLI** (`lms::locate_lms`): `LMS_PATH` if set (must be an
executable file, no fallback), then each `PATH` entry, then
`~/.lmstudio/bin`. On Unix a candidate must have an execute bit, so a stale
non-executable file earlier on `PATH` is skipped.

**Running it** (`lms::run`): argument vector only, `NO_COLOR=1`, stdin closed,
stdout and stderr drained concurrently into capped buffers, ANSI sequences
stripped, child killed on timeout or when the future is dropped.

**Allowlist** (`LmsCommand`, serialized `snake_case`):

| `command` | Runs | Mutating | Needs `args` |
| --- | --- | --- | --- |
| `server_start` | `lms server start` | yes | no |
| `server_stop` | `lms server stop` | yes | no |
| `server_status` | `lms server status --json` | no | no |
| `runtime_ls` | `lms runtime ls` | no | no |
| `runtime_select` | `lms runtime select` | yes | yes (alias or `--latest`) |
| `runtime_update` | `lms runtime update` | yes | no (`--all` for every extension) |
| `runtime_get` | `lms runtime get` | yes | yes (extension name) |
| `runtime_survey` | `lms runtime survey` | no | no |
| `link_status` | `lms link status` | no | no |
| `link_enable` | `lms link enable` | yes | no |
| `link_disable` | `lms link disable` | yes | no |
| `link_set_device_name` | `lms link set-device-name` | yes | yes (name) |
| `link_set_preferred_device` | `lms link set-preferred-device` | yes | yes (device identifier) |
| `get` | `lms get -y` | yes | yes (model name) |
| `import` | `lms import -y` | yes | yes (file path) |
| `clone` | `lms clone` | yes | yes (`owner/name`) |
| `whoami` | `lms whoami` | no | no |
| `logout` | `lms logout` | yes | no |

Results include `command_line` (display form with quoting that keeps
argument boundaries visible; never meant to be executed), `argv` (exact
arguments, one element each), `exit_code`, `stdout`, `stderr`, `timed_out`,
and `truncated`.

**Observed CLI behavior** this design relies on (LM Studio 0.4.25+1, CLI
commit `69d945a`, macOS):

- `lms runtime select` with no argument errors; it needs an alias or
  `--latest`.
- `lms runtime get` with no argument opens an interactive picker; with a name
  it is non-interactive and reports an already-installed extension normally.
- `lms link set-device-name` requires a name; `lms link set-preferred-device`
  opens a picker without an argument and accepts a device identifier.
- `lms runtime update` checks only selected extensions unless `--all` is given.
- `lms server status --json` prints `{"running":bool,"port":n}`.
- Nested `--help` (for example `lms server start --help`) prints the top-level
  help, so subcommand help cannot be scraped reliably.

**Status probe** (`lmstudio_status`): installation, CLI, and API are checked
independently; the CLI check runs `lms version` and extracts
`CLI commit: <hash>`; the app version is read from the macOS app bundle
`Info.plist`. Windows app detection looks under
`%LOCALAPPDATA%\Programs\LM Studio`; Linux relies on `~/.lmstudio` and the CLI.

### 8.2 Subagent

Capability tiers add tools cumulatively: `read_only` (read, list, search),
`read_write` (adds write), `read_write_shell` (adds `run_command`). Every path
is resolved against the working directory and rejected if it escapes through
`../`, an absolute path, or a symlink. `run_command` runs with the working
directory as `cwd`, a 60 s timeout, and capped output, behind the guard in
`subagent/guard.rs`, which blocks privilege escalation, recursive forced
delete, disk and partition tools, raw block-device writes,
`shutdown`/`reboot`, fork bombs, piping a download into a shell, and
force-pushing history (`--force-with-lease` is allowed). The guard is
pattern-matching on the command string, not a shell parser; it is defense in
depth beside the sandbox and capability tier, not a guarantee.

### 8.3 Feedback

Drafts persist to `~/.lmstudio-rs-mcp/feedback.json`. `feedback_submit` order:
look up the entry, resolve a token (argument, `GITHUB_TOKEN`, then
`gh auth token`), check for a duplicate (a match deletes the draft and does
not file), then elicit human review and hand the client a pre-filled "new
issue" page through a second URL-mode elicitation; without elicitation
support it opens that page in the local browser. A missing token skips the
duplicate check gracefully.

### 8.4 Streaming and timeouts

Generation tools request `"stream": true` and reassemble the result in
`client.rs`, for both OpenAI-shaped and Anthropic-shaped streams. The idle
timeout resets on every chunk; the 900 s maximum is an outer safety net. A
connection that closes without a completion signal is reported as
`CONNECTION_FAILED`, never as a truncated success.

## 9. Build, Run, and Validate

`make` lists every target. The Makefile follows the project's GNU Makefile
conventions: namespaced targets with `/`, modular files under `tasks/`,
`?=` defaults, and `##` comments feeding the help screen.

| Command | Purpose |
| --- | --- |
| `make help` | List targets |
| `make deps` | Fetch host-platform dependencies with the lockfile |
| `make build` / `make build/release` | Debug or release build |
| `make test` | Unit and stdio integration tests |
| `make lint/fmt` / `make lint/fix` | Check or apply `rustfmt` |
| `make lint/clippy` | Clippy with warnings as errors |
| `make check` (alias `make all`) | Format check, clippy, tests (the CI gates) |
| `make install` / `make uninstall` | Install into or remove from Cargo's bin directory |
| `make clean` | Remove build artifacts |

Equivalent raw commands:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo build --release
```

Run it against LM Studio by registering the binary with an MCP client; the
README has Claude Code and Claude Desktop examples and the environment
variables. Run `health_check` first, then `lmstudio_status` to diagnose.

## 10. Verification

### 10.1 Test layers

- **Unit tests** live beside the code under `#[cfg(test)]`. Use them for pure
  logic: parsing, allowlists, selection rules, translation.
- **Integration tests** in `tests/mcp_stdio.rs` spawn the real binary and
  speak MCP over stdio. They isolate the environment: `LLM_BASE_URL` points at
  a closed port, `HOME` and `USERPROFILE` at a scratch directory, and
  `LMS_PATH` at a missing file. They assert the full tool list, the four
  annotation hints on every tool, the envelope for every provider tool, the
  feedback round trip, and `lms_cli` validation.
- Match the existing test framework; do not introduce a second one.
- A test that can only pass is not a test: when adding an assertion, confirm it
  can fail (the annotation test was checked by removing one hint).

### 10.2 Environment notes

- Three of the four `sse::tests` bind `127.0.0.1:0` for a throwaway local
  server. Where local port binding is blocked (for example a restrictive sandbox), they fail
  with `Operation not permitted`; that is an environment limit, not a defect.
- `cargo fetch` without `--target` downloads crates for other platforms and
  needs registry access; `make deps` avoids that.

### 10.3 Verification status

Honest record of what was exercised, how, and when. Update on every change
that affects it.

| Area | How | When | Result |
| --- | --- | --- | --- |
| Format, clippy, unit tests (130) | `make check` | 2026-10-08, macOS arm64 | Pass, including the `sse` tests (three of which bind a local port) |
| Integration tests (7) | `make check` (runs `cargo test`) | 2026-10-08, macOS arm64 | Pass |
| CI (format, clippy, release build, tests) | GitHub Actions on macOS arm64 and x86_64, Windows, Linux | PR #5 (`lms` tools) build | Pass on all four targets; later PRs were not re-checked for this document |
| `lmstudio_status`, `lms_cli` `server_status`, `runtime_survey` | Called over stdio against a real LM Studio 0.4.25+1 install | 2026-10-02, macOS | Correct output |
| Argument behavior of `runtime select/get/update`, `link set-*`, `server start/stop` | Observed by the maintainer running the CLI by hand | 2026-10-02 | Matches the design |

**Not exercised by automated tests or by the author:** `lms_cli` running real
`server_start`, `server_stop`, `runtime_update`, `link_enable`,
`link_disable`, `get`, `import`, `clone`, and `logout` through the tool;
the `lms runtime remove` path (intentionally unsupported); the provider tools'
success paths against a live backend; Windows and Linux behavior of CLI
discovery and app detection beyond CI; Ollama, OpenAI, and Anthropic against
real services in this change history.

## 11. Risks, Gaps, and Future Work

Risks:

- **CLI drift.** `lms` output and flags can change between releases; the
  allowlist, required-argument rules, and the `CLI commit:` parse depend on
  observed behavior. Re-verify after LM Studio upgrades.
- **API drift.** LM Studio's `/api/v1` is newer and less documented; tolerant
  parsing reduces, but does not remove, the risk.
- **Destructive commands without server-side confirmation.** `server_stop`,
  `link_disable`, and `logout` run without a prompt from this server; the MCP
  client's permission prompt, driven by the annotations, is the guard.
- **Guard is heuristic.** The subagent command guard cannot stop every
  obfuscated command; keep capability tiers tight.
- **App version is macOS only.** Other platforms report `null`.
- **Feedback file location.** Drafts are stored at
  `~/.lmstudio-rs-mcp/feedback.json`, a dotfile in the home directory. The
  configuration guidance prefers the operating system's application-data
  directory (for example `~/Library/Application Support/<app>/` on macOS).
  Moving it is a breaking change for existing drafts, so it needs a
  migration or a documented upgrade path.
- **Guidance copies can go stale.** `agents/*.md` are snapshots of
  `~/.agents`; refresh them when the shared guidance changes.

Gaps:

- No mock HTTP backend, so provider happy paths are untested end to end.
- No automated test for the elicitation and browser-fallback flow.
- No automated check that secrets never reach logs or results (NFR-6).
- No Linux desktop-app detection in `lmstudio_status`.

Possible future work:

- A small mock HTTP server for happy-path integration tests.
- A time-limited `log stream` wrapper.
- Per-command confirmation for destructive `lms_cli` commands via MCP
  elicitation.

## 12. Change Playbooks

### Add a tool

1. Add the requirement and the catalog row ([sections 4 and 5.3](#4-requirements)).
2. Add the input and output types and the function in `src/tools/`; return
   `ToolResult<T>`.
3. Register it in `src/server.rs` with a description and all four annotation
   hints.
4. Add it to `ALL_TOOLS` and the relevant cases in `tests/mcp_stdio.rs`, plus
   unit tests for the logic.
5. Update the README tool table and this file; run `make check`.

### Add a provider

1. Extend `Provider` and every capability predicate in `src/providers.rs`.
2. Add configuration defaults in `src/config.rs`.
3. Translate requests and responses in `src/client.rs`, keeping quirks there.
4. Update [section 5.4](#54-provider-capability-matrix), the README, and the
   capability tests; verify against the real service.

### Add an `lms` command

1. Observe the real command, including bare invocation, required arguments,
   and whether it prompts. Record the behavior in
   [section 8.1](#81-lms-cli-and-environment-tools).
2. Add a variant to `LmsCommand` with `base_args`, `is_mutating`, and
   `min_args`.
3. Decide it against the exclusion rules (destructive, interactive,
   unbounded, publishing, or already covered by the REST API).
4. Add unit tests for argv, mutation class, and required arguments; update the
   tool description and README.

### Change a contract

Change the specification and the catalog first, state whether the change is
breaking, and keep input and output contracts stable unless the task is
explicitly a breaking change.
