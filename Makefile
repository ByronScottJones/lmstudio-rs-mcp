# lmstudio-rs-mcp task runner. Run `make` or `make help` to list targets.
# Namespaced targets live in tasks/Makefile.<namespace> and use `/` as the delimiter.

-include tasks/Makefile.*

# Cargo executable; override to use a specific toolchain (e.g. CARGO="cargo +nightly").
CARGO ?= cargo
# Name of the installed binary and crate.
BINARY ?= lmstudio-rs-mcp
# Host target triple. `deps` fetches only this platform's crates so it works
# offline from a warm cache instead of downloading other platforms' crates.
HOST ?= $(shell rustc -vV | sed -n 's/^host: //p')

.DEFAULT_GOAL := help

.PHONY: help default all deps build build/release test check install uninstall clean

## This help screen
help:
	@printf "Available targets:\n\n"
	@awk '/^[a-zA-Z\-_0-9%:\\\/]+/ { \
		helpMessage = match(lastLine, /^## (.*)/); \
		if (helpMessage) { \
			helpCommand = $$1; \
			helpMessage = substr(lastLine, RSTART + 3, RLENGTH); \
			gsub("\\\\", "", helpCommand); \
			gsub(":+$$", "", helpCommand); \
			printf "  \x1b[32;01m%-35s\x1b[0m %s\n", helpCommand, helpMessage; \
		} \
	} \
	{ lastLine = $$0 }' $(MAKEFILE_LIST) | sort -u
	@printf "\n"

## Alias for help
default: help

## Run every quality gate: format check, clippy, and the full test suite
all: check

## Fetch host-platform dependencies using the committed lockfile
deps:
	@command -v $(firstword $(CARGO)) >/dev/null || { echo "cargo not found; install Rust from https://rustup.rs" >&2; exit 1; }
	$(CARGO) fetch --locked --target $(HOST)

## Build a debug binary with all targets
build: deps
	$(CARGO) build --all-targets --locked

## Build the optimized release binary
build/release: deps
	$(CARGO) build --release --locked

## Run unit tests and the stdio integration tests (no LM Studio needed)
test: deps
	$(CARGO) test --locked

## Run the same gates as CI: lint/fmt, lint/clippy, test
check: lint/fmt lint/clippy test

## Install the binary into Cargo's bin directory
install: deps
	$(CARGO) install --path . --locked

## Remove the installed binary
uninstall:
	$(CARGO) uninstall $(BINARY)

## Remove build artifacts
clean:
	$(CARGO) clean
