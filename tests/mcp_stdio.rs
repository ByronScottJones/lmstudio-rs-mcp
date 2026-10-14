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
    "list_profiles",
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
        Self::start_with(&[], |_| {})
    }

    /// Start the server with extra environment variables; `prepare` runs
    /// on the scratch directory before launch (to write a profiles file).
    fn start_with(env: &[(&str, &str)], prepare: impl FnOnce(&std::path::Path)) -> Self {
        let scratch =
            std::env::temp_dir().join(format!("lmstudio-rs-mcp-e2e-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&scratch).expect("create scratch dir");
        prepare(&scratch);

        let mut child = Command::new(env!("CARGO_BIN_EXE_lmstudio-rs-mcp"))
            .env("LLM_PROVIDER", "lmstudio")
            .env("LLM_BASE_URL", "http://127.0.0.1:1")
            .env("LMS_PATH", scratch.join("no-such-lms"))
            .env("HOME", &scratch)
            .env("USERPROFILE", &scratch)
            .env_remove("LLM_API_KEY")
            .env_remove("LLM_PROFILES_FILE")
            .env_remove("GITHUB_TOKEN")
            .envs(env.iter().copied())
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
        "list_profiles",
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

#[test]
fn profiles_route_calls_to_different_backends() {
    // Both backends refuse connections; the point is which one is tried.
    let dir =
        std::env::temp_dir().join(format!("lmstudio-rs-mcp-profiles-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("profiles.toml");
    std::fs::write(
        &file,
        "default_profile = \"alpha\"\n\
         [profiles.alpha]\nprovider = \"lmstudio\"\nbase_url = \"http://127.0.0.1:1\"\n\
         [profiles.beta]\nprovider = \"openai-compatible\"\nbase_url = \"http://127.0.0.1:2\"\n",
    )
    .unwrap();
    let mut server = Server::start_with(&[("LLM_PROFILES_FILE", file.to_str().unwrap())], |_| {});

    let listed = server.call("list_profiles", json!({}));
    assert_eq!(listed["success"], json!(true), "{listed}");
    let profiles = listed["data"].as_array().unwrap();
    assert_eq!(profiles.len(), 2, "{listed}");
    assert_eq!(profiles[0]["name"], json!("alpha"));
    assert_eq!(profiles[0]["is_default"], json!(true));
    assert_eq!(profiles[1]["provider"], json!("openai-compatible"));

    // Omitted profile -> default; the result says which profile served it.
    let default = server.call("health_check", json!({}));
    assert_eq!(default["profile"], json!("alpha"), "{default}");
    assert!(default["message"].as_str().unwrap().contains("127.0.0.1:1"));

    let beta = server.call("health_check", json!({ "profile": "beta" }));
    assert_eq!(beta["profile"], json!("beta"), "{beta}");
    assert!(beta["message"].as_str().unwrap().contains("127.0.0.1:2"));

    // Tools with their own inputs accept `profile` too.
    let chat = server.call(
        "chat_completion",
        json!({ "prompt": "hi", "model": "x", "profile": "beta" }),
    );
    assert_eq!(chat["profile"], json!("beta"), "{chat}");

    let unknown = server.call("health_check", json!({ "profile": "nope" }));
    assert_eq!(error_code(&unknown), "INVALID_INPUT", "{unknown}");
    assert!(unknown["message"].as_str().unwrap().contains("alpha, beta"));
}

#[test]
fn an_invalid_profiles_file_stops_the_server_before_serving() {
    let scratch = std::env::temp_dir().join(format!(
        "lmstudio-rs-mcp-badprofiles-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&scratch).unwrap();
    let file = scratch.join("profiles.toml");
    std::fs::write(&file, "[profiles.x]\nprovider = \"bedrock\"\n").unwrap();
    let status = Command::new(env!("CARGO_BIN_EXE_lmstudio-rs-mcp"))
        .env("LLM_PROFILES_FILE", &file)
        .env("HOME", &scratch)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("spawn server binary");
    assert!(!status.success());
}

/// A minimal stand-in for oMLX's model routes. Requires `Bearer test-key`
/// and tracks whether model `m1` is loaded. Returns its base URL.
fn spawn_mock_omlx() -> String {
    use std::io::Read;
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock oMLX");
    let base = format!("http://{}", listener.local_addr().unwrap());
    let loaded = Arc::new(Mutex::new(false));
    std::thread::spawn(move || {
        for stream in listener.incoming().map_while(Result::ok) {
            let loaded = Arc::clone(&loaded);
            std::thread::spawn(move || {
                let mut stream = stream;
                let mut buf = Vec::new();
                let mut chunk = [0u8; 1024];
                while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    match stream.read(&mut chunk) {
                        Ok(0) | Err(_) => return,
                        Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    }
                }
                let head = String::from_utf8_lossy(&buf).to_string();
                let mut lines = head.lines();
                let request_line = lines.next().unwrap_or("").to_string();
                let authorized = head
                    .lines()
                    .any(|l| l.eq_ignore_ascii_case("authorization: Bearer test-key"));
                let (status, body) = if !authorized {
                    (401, json!({ "error": { "message": "bad key" } }))
                } else {
                    let mut state = loaded.lock().unwrap();
                    match request_line.as_str() {
                        l if l.starts_with("GET /v1/models/status ") => (
                            200,
                            json!({ "models": [
                                { "id": "m1", "loaded": *state, "is_loading": false,
                                  "estimated_size": 1000, "model_context_length": 4096,
                                  "model_type": "llm", "config_model_type": "qwen3" },
                                { "id": "m2", "loaded": false, "is_loading": false }
                            ] }),
                        ),
                        l if l.starts_with("POST /v1/models/m1/load ") => {
                            *state = true;
                            (200, json!({ "status": "ok", "model_id": "m1" }))
                        }
                        l if l.starts_with("POST /v1/models/m1/unload ") => {
                            if *state {
                                *state = false;
                                (200, json!({ "status": "ok", "model_id": "m1" }))
                            } else {
                                (
                                    400,
                                    json!({ "error": { "message": "Model not loaded: m1" } }),
                                )
                            }
                        }
                        l if l.contains("/load ") || l.contains("/unload ") => {
                            (404, json!({ "error": { "message": "Model not found" } }))
                        }
                        _ => (404, json!({ "error": { "message": "no route" } })),
                    }
                };
                let body = body.to_string();
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            });
        }
    });
    base
}

#[test]
fn omlx_profile_lists_loads_and_unloads_models() {
    let base = spawn_mock_omlx();
    let dir = std::env::temp_dir().join(format!("lmstudio-rs-mcp-omlx-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("profiles.toml");
    std::fs::write(
        &file,
        format!(
            "[profiles.omlx]\nprovider = \"omlx\"\nbase_url = \"{base}\"\n\
             api_key_env = \"OMLX_TEST_KEY\"\n"
        ),
    )
    .unwrap();
    let mut server = Server::start_with(
        &[
            ("LLM_PROFILES_FILE", file.to_str().unwrap()),
            ("OMLX_TEST_KEY", "test-key"),
        ],
        |_| {},
    );

    let listed = server.call("list_profiles", json!({}));
    assert_eq!(listed["data"][0]["provider"], json!("omlx"), "{listed}");
    assert_eq!(
        listed["data"][0]["capabilities"]["model_management"],
        json!(true)
    );

    // Nothing loaded yet; the library has both models, with detail mapped.
    let loaded = server.call("list_loaded_models", json!({}));
    assert_eq!(loaded["success"], json!(true), "{loaded}");
    assert_eq!(loaded["data"].as_array().unwrap().len(), 0, "{loaded}");
    let library = server.call("list_models", json!({}));
    assert_eq!(library["data"].as_array().unwrap().len(), 2, "{library}");
    assert_eq!(library["data"][0]["architecture"], json!("qwen3"));

    // Load, with a per-load option oMLX cannot apply: it loads and says so.
    let load = server.call(
        "load_model",
        json!({ "model": "m1", "context_length": 8192 }),
    );
    assert_eq!(load["success"], json!(true), "{load}");
    assert_eq!(load["data"]["instance_id"], json!("m1"));
    let status = load["data"]["status"].as_str().unwrap();
    assert!(status.contains("context_length not applied"), "{status}");
    assert!(load["data"]["load_time_seconds"].is_number());

    let loaded = server.call("list_loaded_models", json!({}));
    assert_eq!(loaded["data"][0]["identifier"], json!("m1"), "{loaded}");
    let current = server.call("get_current_model", json!({}));
    assert_eq!(current["data"].as_array().unwrap().len(), 1, "{current}");

    let unload = server.call("unload_model", json!({ "identifier": "m1" }));
    assert_eq!(unload["success"], json!(true), "{unload}");
    let loaded = server.call("list_loaded_models", json!({}));
    assert_eq!(loaded["data"].as_array().unwrap().len(), 0, "{loaded}");

    // Unloading again is MODEL_NOT_LOADED, not an opaque failure.
    let again = server.call("unload_model", json!({ "identifier": "m1" }));
    assert_eq!(error_code(&again), "MODEL_NOT_LOADED", "{again}");

    // An unknown model fails to load with LOAD_FAILED.
    let unknown = server.call("load_model", json!({ "model": "m9" }));
    assert_eq!(error_code(&unknown), "LOAD_FAILED", "{unknown}");
}
