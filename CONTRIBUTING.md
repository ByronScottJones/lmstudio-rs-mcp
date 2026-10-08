# Contributing

## Development

```bash
make check                                   # fmt check + clippy + tests
make build/release                           # release binary
```

`make check` runs `cargo fmt --all -- --check`,
`cargo clippy --all-targets -- -D warnings`, and `cargo test`. It should
pass clean before opening a PR — CI runs the same checks on macOS, Windows,
and Linux. Run `make help` for every target.

Read [ENGINEERING.md](ENGINEERING.md) first: it is the development
specification. Specify a behavior change there (requirement, contract,
acceptance criterion) before implementing it, and update it in the same PR.

## Workflow

- Branch from `main`, open a PR against `main`. Direct pushes to `main` are
  not used, even for small changes.
- Keep commits focused; a commit message should explain *why*, not just
  restate the diff.
- If a change touches request/response handling against LM Studio's API,
  verify it against a real running instance where practical — several bugs
  in this project's history were only caught that way (see the git log for
  examples), not by unit tests alone.

## Reporting feedback about this server

If you're using `lmstudio-rs-mcp` as an MCP server and hit a bug or have an
idea, the `feedback_create` / `feedback_submit` tools can draft and file it
as a GitHub issue here directly — see the README's "Feedback" section.
Opening an issue by hand works just as well, of course.
