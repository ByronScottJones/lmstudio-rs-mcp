# Agent Guidance: lmstudio-rs-mcp

This file is always active. Apply its rules to every task, then load only the
topic files required by the task. Do not load every topic file by default.

Read [`ENGINEERING.md`](ENGINEERING.md) before changing anything. It is the
development specification: requirements, tool contracts, design decisions,
verification status, and the spec-driven workflow. Update it in the same
change set whenever behavior, contracts, decisions, or verification status
change.

## Project Essentials

- Rust MCP server bridging MCP clients to LM Studio, Ollama, OpenAI, or
  Anthropic over stdio. Stdout carries only MCP protocol messages; log to
  stderr.
- Every tool returns the `ToolResult` envelope and declares all four
  annotation hints (`readOnlyHint`, `destructiveHint`, `idempotentHint`,
  `openWorldHint`). New tools follow the playbook in `ENGINEERING.md`.
- Run `make check` (format check, clippy with warnings as errors, tests)
  before committing. `make help` lists every target.
- Work on a branch and merge by pull request; do not push to `main`.
- Never hardcode or log credentials, tokens, or environment-specific values.

## Topic Routing

The files below are project-level copies of the shared guidance in
`~/.agents`. Load the matching file before making decisions or edits in that
topic. When the shared guidance changes, refresh the copy and keep this
table in step with the folder.

| Topic | File to load |
| --- | --- |
| Project specification, contracts, decisions, and workflow | [`ENGINEERING.md`](ENGINEERING.md) |
| Git repositories, branches, commits, or pull requests | [`agents/git.md`](agents/git.md) |
| Source code, scripts, or general coding practice | [`agents/coding.md`](agents/coding.md) |
| Rust | [`agents/rust.md`](agents/rust.md) |
| Test strategy or test implementation | [`agents/testing.md`](agents/testing.md) |
| Markdown files or documentation authoring | [`agents/markdown.md`](agents/markdown.md) |
| GNU Makefiles and task runners | [`agents/makefile.md`](agents/makefile.md) |
| Team engineering workflow | [`agents/engineering-workflow.md`](agents/engineering-workflow.md) |
| Security engineering or supply-chain security | [`agents/security.md`](agents/security.md) |
| Application configuration, settings, or secrets | [`agents/configuration.md`](agents/configuration.md) |

Load more than one file when a task spans topics. For example, adding an
`lms_cli` command touches `rust.md`, `security.md`, and `testing.md`.

Language files supplement this file; they do not replace the rules below.

## Guidance Precedence

Apply this file to every task. Load every matching topic file before making
decisions or edits. More specific topic guidance supplements general guidance;
when two files conflict, the more specific file wins unless this file states
otherwise.

---

## General Behavior

- Be concise and direct. Prefer shorter responses when the answer is clear.
- Don't add explanatory preamble before code ("Here is a function that..."). Just write the code.
- Don't apologize or over-explain errors. Diagnose and fix.
- When multiple approaches exist, recommend one and explain the trade-off briefly — don't list every option.
- Never add `TODO` comments unless explicitly asked.
- Never generate placeholder logic (e.g., `// implement later`) — implement it or say you can't.
- Prefer editing existing code over rewriting files wholesale.
- When unsure about intent, ask one targeted question rather than guessing.

---

## Tool Calling

- When calling command line tools, add a comment at the end of the command line explaining the purpose of the command.
  When calling multi-line scripts, including custom executables that you have created, each script should have detailed comments,
  including a comment block at the beginning explaining the purpose of the script, and each line or group of commands should have a comment.
