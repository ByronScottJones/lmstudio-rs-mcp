//! End-to-end tests: spawn the real server binary and talk MCP to it over
//! stdio, exactly as a client would.
//!
//! The environment is fully isolated so nothing touches the developer's
//! machine: the backend URL points at a closed port (connection refused,
//! fast and deterministic), `HOME`/`USERPROFILE` point at a scratch
//! directory (feedback storage), and `LMS_PATH` points at a file that
//! doesn't exist (so no real `lms` is ever run).

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// Every tool the server declares. Kept explicit so adding a tool without
/// annotating and testing it fails here.
const ALL_TOOLS: &[&str] = &[
    "health_check",
    "lmstudio_status",
    "lms_cli",
    "list_models",
    "list_loaded_models",
    "get_current_model",
    "get_model_info",
    "load_model",
    "unload_model",
    "chat_completion",
    "text_completion",
    "generate_embeddings",
    "create_response",
    "start_conversation",
    "continue_conversation",
    "run_subagent",
    "feedback_create",
    "feedback_list",
    "feedback_get",
    "feedback_update",
    "feedback_delete",
    "feedback_check_duplicates",
    "feedback_submit",
];

struct Server {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
    next_id: u64,
    scratch: PathBuf,
}

impl Server {
    fn start() -> Self {
        let scratch =
            std::env::temp_dir().join(format!("lmstudio-rs-mcp-e2e-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&scratch).expect("create scratch dir");

        let mut child = Command::new(env!("CARGO_BIN_EXE_lmstudio-rs-mcp"))
            .env("LLM_PROVIDER", "lmstudio")
            .env("LLM_BASE_URL", "http://127.0.0.1:1")
            .env("LMS_PATH", scratch.join("no-such-lms"))
            .env("HOME", &scratch)
            .env("USERPROFILE", &scratch)
            .env_remove("LLM_API_KEY")
            .env_remove("GITHUB_TOKEN")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn server binary");

        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        // A reader thread lets every wait have a timeout instead of
        // hanging the test run if the server stops answering.
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });

        let mut server = Self {
            child,
            stdin,
            lines,
            next_id: 1,
            scratch,
        };
        server.request(
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "e2e-test", "version": "0" }
            }),
        );
        server.send(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
        server
    }

    fn send(&mut self, msg: &Value) {
        writeln!(self.stdin, "{msg}").expect("write to server");
        self.stdin.flush().expect("flush to server");
    }

    /// Send a request and return its `result`, skipping any unrelated
    /// messages (notifications) the server emits in between.
    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        loop {
            let line = self
                .lines
                .recv_timeout(REQUEST_TIMEOUT)
                .unwrap_or_else(|_| panic!("timed out waiting for `{method}` response"));
            let msg: Value = serde_json::from_str(&line).expect("server sent valid JSON");
            if msg["id"] == json!(id) {
                assert!(msg.get("error").is_none(), "`{method}` failed: {msg}");
                return msg["result"].clone();
            }
        }
    }

    /// Call a tool and return its `ToolResult` envelope (`structuredContent`).
    fn call(&mut self, name: &str, arguments: Value) -> Value {
        let result = self.request(
            "tools/call",
            json!({ "name": name, "arguments": arguments }),
        );
        assert_ne!(
            result["isError"],
            json!(true),
            "`{name}` returned a protocol-level error: {result}"
        );
        let envelope = result["structuredContent"].clone();
        assert!(
            envelope["success"].is_boolean() && envelope["message"].is_string(),
            "`{name}` did not return a ToolResult envelope: {result}"
        );
        envelope
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.scratch);
    }
}

fn error_code(envelope: &Value) -> &str {
    envelope["error"]["code"].as_str().unwrap_or("")
}

#[test]
fn every_tool_declares_all_four_annotation_hints_as_booleans() {
    let mut server = Server::start();
    let listed = server.request("tools/list", json!({}));
    let tools = listed["tools"].as_array().expect("tools array");

    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    for expected in ALL_TOOLS {
        assert!(names.contains(expected), "missing tool `{expected}`");
    }
    assert_eq!(names.len(), ALL_TOOLS.len(), "unexpected tools: {names:?}");

    for tool in tools {
        let name = tool["name"].as_str().unwrap();
        for hint in [
            "readOnlyHint",
            "destructiveHint",
            "idempotentHint",
            "openWorldHint",
        ] {
            assert!(
                tool["annotations"][hint].is_boolean(),
                "tool `{name}` must declare `{hint}` as an explicit boolean: {}",
                tool["annotations"]
            );
        }
    }
}

#[test]
fn annotations_match_what_the_tools_actually_do() {
    let mut server = Server::start();
    let listed = server.request("tools/list", json!({}));
    let hint = |tool: &str, hint: &str| -> bool {
        listed["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == tool)
            .unwrap_or_else(|| panic!("no tool `{tool}`"))["annotations"][hint]
            .as_bool()
            .unwrap()
    };

    // Pure reads must not be flagged as mutating.
    for read_only in [
        "health_check",
        "lmstudio_status",
        "list_models",
        "list_loaded_models",
        "get_current_model",
        "get_model_info",
        "feedback_list",
        "feedback_get",
        "feedback_check_duplicates",
    ] {
        assert!(hint(read_only, "readOnlyHint"), "{read_only}");
        assert!(!hint(read_only, "destructiveHint"), "{read_only}");
    }
    // Anything that can stop servers, unload models, run commands, or
    // delete data must be flagged destructive so hosts can warn.
    for destructive in [
        "lms_cli",
        "unload_model",
        "run_subagent",
        "feedback_delete",
        "feedback_submit",
    ] {
        assert!(!hint(destructive, "readOnlyHint"), "{destructive}");
        assert!(hint(destructive, "destructiveHint"), "{destructive}");
    }
    // Only the feedback store tools are purely local.
    assert!(!hint("feedback_create", "openWorldHint"));
    assert!(hint("chat_completion", "openWorldHint"));
}

#[test]
fn provider_tools_report_connection_failures_in_the_envelope() {
    let mut server = Server::start();
    let dir = server.scratch.display().to_string();

    let calls: Vec<(&str, Value)> = vec![
        ("health_check", json!({})),
        ("list_models", json!({})),
        ("list_loaded_models", json!({})),
        ("get_current_model", json!({})),
        ("get_model_info", json!({ "identifier": "x" })),
        ("load_model", json!({ "model": "x" })),
        ("unload_model", json!({ "identifier": "x" })),
        ("chat_completion", json!({ "prompt": "hi", "model": "x" })),
        ("text_completion", json!({ "prompt": "hi", "model": "x" })),
        ("generate_embeddings", json!({ "text": "hi", "model": "x" })),
        (
            "create_response",
            json!({ "input_text": "hi", "model": "x" }),
        ),
        (
            "start_conversation",
            json!({ "system_prompt": "s", "first_message": "m", "model": "x" }),
        ),
        (
            "continue_conversation",
            json!({ "response_id": "r", "message": "m", "model": "x" }),
        ),
        (
            "run_subagent",
            json!({ "task": "t", "working_directory": dir, "model": "x" }),
        ),
    ];
    for (name, args) in calls {
        let envelope = server.call(name, args);
        assert_eq!(
            envelope["success"],
            json!(false),
            "`{name}` has no backend to reach and should fail: {envelope}"
        );
        assert!(envelope["error"].is_object(), "`{name}`: {envelope}");
    }

    // The connectivity check specifically reports the right error code.
    let health = server.call("health_check", json!({}));
    assert_eq!(error_code(&health), "CONNECTION_FAILED");
}

#[test]
fn lmstudio_status_reports_each_finding_independently() {
    let mut server = Server::start();
    let envelope = server.call("lmstudio_status", json!({}));

    // A probe always succeeds; the findings are in `data`.
    assert_eq!(envelope["success"], json!(true), "{envelope}");
    let data = &envelope["data"];
    assert_eq!(data["cli"]["available"], json!(false), "LMS_PATH is bogus");
    assert!(data["cli"]["error"].is_string());
    assert_eq!(data["api"]["checked"], json!(true));
    assert_eq!(data["api"]["available"], json!(false));
    assert_eq!(data["api"]["base_url"], json!("http://127.0.0.1:1"));
    assert!(data["installation"]["installed"].is_boolean());
}

#[test]
fn lms_cli_reports_a_missing_cli_and_validates_input_without_running_anything() {
    let mut server = Server::start();

    let missing = server.call("lms_cli", json!({ "command": "whoami" }));
    assert_eq!(missing["success"], json!(false));
    assert_eq!(error_code(&missing), "CLI_NOT_FOUND");

    // A command that would prompt is rejected before the CLI is even looked for.
    let needs_arg = server.call("lms_cli", json!({ "command": "runtime_select" }));
    assert_eq!(error_code(&needs_arg), "INVALID_INPUT");

    // Commands outside the allowlist are refused at the protocol layer.
    let result = server.request(
        "tools/call",
        json!({ "name": "lms_cli", "arguments": { "command": "runtime_remove" } }),
    );
    assert_eq!(
        result["isError"],
        json!(true),
        "runtime_remove must not be accepted: {result}"
    );
}

#[test]
fn feedback_tools_round_trip_through_the_local_store() {
    let mut server = Server::start();

    let created = server.call(
        "feedback_create",
        json!({ "category": "issue", "title": "A title", "body": "A body" }),
    );
    assert_eq!(created["success"], json!(true), "{created}");
    let id = created["data"]["id"].as_u64().expect("entry id");

    let listed = server.call("feedback_list", json!({}));
    assert_eq!(listed["data"].as_array().unwrap().len(), 1);

    let got = server.call("feedback_get", json!({ "id": id }));
    assert_eq!(got["data"]["title"], json!("A title"));

    let updated = server.call("feedback_update", json!({ "id": id, "title": "New title" }));
    assert_eq!(updated["data"]["title"], json!("New title"));

    let deleted = server.call("feedback_delete", json!({ "id": id }));
    assert_eq!(deleted["success"], json!(true));
    let gone = server.call("feedback_get", json!({ "id": id }));
    assert_eq!(gone["success"], json!(false));
}

#[test]
fn feedback_submit_and_duplicate_check_fail_cleanly_for_an_unknown_entry() {
    // Both look the entry up before any network or browser step, so an
    // unknown id exercises them without leaving the machine.
    let mut server = Server::start();
    for tool in ["feedback_check_duplicates", "feedback_submit"] {
        let envelope = server.call(tool, json!({ "id": 999_999 }));
        assert_eq!(envelope["success"], json!(false), "{tool}: {envelope}");
    }
}
