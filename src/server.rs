//! The MCP server: wires each tool implementation in `crate::tools` up to
//! the `rmcp` tool-call dispatch machinery.

use crate::client::ApiClient;
use crate::config::Profiles;
use crate::feedback::store::FeedbackStore;
use crate::profiles::ProfileRegistry;
use crate::tools::{
    chat, embeddings, feedback, health_check, lms, models, profiles, responses, subagent,
};
use crate::types::{ErrorCode, ToolResult};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::{tool, tool_handler, tool_router, Peer, RoleServer, ServerHandler};
use std::future::Future;
use std::sync::Arc;

/// This server's own repo — the default target for `feedback_submit` /
/// `feedback_check_duplicates` when the caller doesn't pass `repo`.
const DEFAULT_FEEDBACK_REPO: &str = "ByronScottJones/lmstudio-rs-mcp";

#[derive(Clone)]
pub struct LmStudioServer {
    profiles: Arc<Profiles>,
    registry: Arc<ProfileRegistry>,
    feedback_store: Arc<FeedbackStore>,
    // Separate from `client`'s HTTP client: this one talks to api.github.com,
    // not LM Studio, and carries no LM Studio base URL/auth token.
    github_http: reqwest::Client,
    // Shared (not just cloned per-call) so every clone of this server sees
    // the same conversations — see PersonaCache's doc comment for why this
    // exists at all.
    personas: Arc<responses::PersonaCache>,
    // Read by the code `#[tool_handler]` generates to dispatch `call_tool`
    // requests; the dead-code lint can't see through that macro expansion.
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
}

impl LmStudioServer {
    pub fn new(profiles: Profiles, feedback_store: FeedbackStore) -> Self {
        Self {
            registry: Arc::new(ProfileRegistry::new(&profiles)),
            profiles: Arc::new(profiles),
            feedback_store: Arc::new(feedback_store),
            github_http: reqwest::Client::new(),
            personas: Arc::new(responses::PersonaCache::new()),
            tool_router: Self::tool_router(),
        }
    }
}

impl LmStudioServer {
    /// Run `f` against the client for `profile` (the default profile when
    /// `None`), and stamp the result with the profile that served it. An
    /// unknown profile is an `INVALID_INPUT` result, not a protocol error.
    async fn on_profile<T, F, Fut>(&self, profile: Option<&str>, f: F) -> Json<ToolResult<T>>
    where
        F: FnOnce(Arc<ApiClient>) -> Fut,
        Fut: Future<Output = ToolResult<T>>,
    {
        match self.registry.resolve(profile) {
            Ok(client) => {
                let name = client.profile().to_string();
                Json(f(client).await.with_profile(name))
            }
            Err(msg) => Json(ToolResult::err(msg.clone(), ErrorCode::InvalidInput, msg)),
        }
    }
}

#[tool_router]
impl LmStudioServer {
    #[tool(
        description = "Check connectivity to a provider profile (LM Studio, Ollama, OpenAI, Anthropic, or an OpenAI-compatible server). Uses the default profile unless `profile` is given — see list_profiles",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = true
        )
    )]
    async fn health_check(
        &self,
        Parameters(input): Parameters<profiles::ProfileInput>,
    ) -> Json<crate::types::ToolResult<health_check::HealthCheckData>> {
        self.on_profile(input.profile.as_deref(), |c| async move {
            health_check::health_check(&c).await
        })
        .await
    }

    #[tool(
        description = "Check the LM Studio environment: whether LM Studio is installed (and its app version where readable), whether the `lms` CLI is available (and its build commit), and whether the LM Studio REST API is reachable. Each is reported independently, so this works even when LM Studio isn't running or isn't installed",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = true
        )
    )]
    async fn lmstudio_status(
        &self,
        Parameters(input): Parameters<profiles::ProfileInput>,
    ) -> Json<crate::types::ToolResult<lms::LmStudioStatusData>> {
        self.on_profile(input.profile.as_deref(), |c| async move {
            lms::lmstudio_status(&c).await
        })
        .await
    }

    #[tool(
        description = "Run an `lms` CLI command for LM Studio features the REST API lacks: server start/stop/status, runtime engine management (ls/select/update/get/survey), LM Link (status/enable/disable/set-device-name/set-preferred-device), model download (`get`), `import`, Hub `clone`, and login state (`whoami`/`logout`). Model list/load/unload have their own tools. Runs without a shell and without stdin, so commands can't prompt: those that would (runtime_select, runtime_get, link_set_device_name, link_set_preferred_device (takes a device identifier), get, import, clone) require an argument in `args`, e.g. runtime_select with [\"--latest\"] or an engine alias from runtime_ls, runtime_update with [\"--all\"] to update every installed extension rather than only the selected ones. `get` and `import` pass -y",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = true
        )
    )]
    async fn lms_cli(
        &self,
        Parameters(input): Parameters<lms::LmsCliInput>,
    ) -> Json<crate::types::ToolResult<lms::LmsCliData>> {
        Json(lms::lms_cli(input).await)
    }

    #[tool(
        description = "List models available from the configured provider — the local library for LM Studio/Ollama, or every model the API key can use for OpenAI/Anthropic",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = true
        )
    )]
    async fn list_models(
        &self,
        Parameters(input): Parameters<profiles::ProfileInput>,
    ) -> Json<crate::types::ToolResult<Vec<models::ModelSummary>>> {
        self.on_profile(input.profile.as_deref(), |c| async move {
            models::list_models(&c).await
        })
        .await
    }

    #[tool(
        description = "List all currently loaded model instances. Only meaningful for LM Studio/Ollama — cloud providers have no loading concept and return an empty list",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = true
        )
    )]
    async fn list_loaded_models(
        &self,
        Parameters(input): Parameters<profiles::ProfileInput>,
    ) -> Json<crate::types::ToolResult<Vec<models::LoadedModelSummary>>> {
        self.on_profile(input.profile.as_deref(), |c| async move {
            models::list_loaded_models(&c).await
        })
        .await
    }

    #[tool(
        description = "Identify the currently loaded model (or models, if more than one is loaded). Only meaningful for LM Studio/Ollama",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = true
        )
    )]
    async fn get_current_model(
        &self,
        Parameters(input): Parameters<profiles::ProfileInput>,
    ) -> Json<crate::types::ToolResult<Vec<models::LoadedModelSummary>>> {
        self.on_profile(input.profile.as_deref(), |c| async move {
            models::get_current_model(&c).await
        })
        .await
    }

    #[tool(
        description = "Get detailed information about a specific loaded model instance. Only meaningful for LM Studio/Ollama",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = true
        )
    )]
    async fn get_model_info(
        &self,
        Parameters(input): Parameters<models::GetModelInfoInput>,
    ) -> Json<crate::types::ToolResult<models::LoadedModelSummary>> {
        let profile = input.profile.clone();
        self.on_profile(profile.as_deref(), |c| async move {
            models::get_model_info(&c, input).await
        })
        .await
    }

    #[tool(
        description = "Load a model into memory. Only supported by LM Studio/Ollama — cloud providers have nothing to load",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = true
        )
    )]
    async fn load_model(
        &self,
        Parameters(input): Parameters<models::LoadModelInput>,
    ) -> Json<crate::types::ToolResult<models::LoadedModelData>> {
        let profile = input.profile.clone();
        self.on_profile(profile.as_deref(), |c| async move {
            models::load_model(&c, input).await
        })
        .await
    }

    #[tool(
        description = "Unload a model instance from memory. Only supported by LM Studio/Ollama",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = true
        )
    )]
    async fn unload_model(
        &self,
        Parameters(input): Parameters<models::UnloadModelInput>,
    ) -> Json<crate::types::ToolResult<()>> {
        let profile = input.profile.clone();
        self.on_profile(profile.as_deref(), |c| async move {
            models::unload_model(&c, input).await
        })
        .await
    }

    #[tool(
        description = "Generate a chat completion from the configured provider's current/named model",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = true
        )
    )]
    async fn chat_completion(
        &self,
        Parameters(input): Parameters<chat::ChatCompletionInput>,
    ) -> Json<crate::types::ToolResult<chat::ChatCompletionData>> {
        let profile = input.profile.clone();
        self.on_profile(profile.as_deref(), |c| async move {
            chat::chat_completion(&c, input).await
        })
        .await
    }

    #[tool(
        description = "Generate a raw text completion (non-chat format) — simpler and faster than chat_completion for single-turn tasks like code completion",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = true
        )
    )]
    async fn text_completion(
        &self,
        Parameters(input): Parameters<chat::TextCompletionInput>,
    ) -> Json<crate::types::ToolResult<chat::TextCompletionData>> {
        let profile = input.profile.clone();
        self.on_profile(profile.as_deref(), |c| async move {
            chat::text_completion(&c, input).await
        })
        .await
    }

    #[tool(
        description = "Generate vector embeddings for text, for semantic search, RAG, and similarity comparisons. Requires an embedding-specific model (or provider — Anthropic has no embeddings endpoint)",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = true
        )
    )]
    async fn generate_embeddings(
        &self,
        Parameters(input): Parameters<embeddings::GenerateEmbeddingsInput>,
    ) -> Json<crate::types::ToolResult<embeddings::EmbeddingsData>> {
        let profile = input.profile.clone();
        self.on_profile(profile.as_deref(), |c| async move {
            embeddings::generate_embeddings(&c, input).await
        })
        .await
    }

    #[tool(
        description = "Create a stateful response via a /v1/responses-style endpoint — conversation context is tracked server-side by response ID, no manual message history needed. Only LM Studio (v0.3.29+) and OpenAI support this; other providers return an error directing you to chat_completion instead",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = true
        )
    )]
    async fn create_response(
        &self,
        Parameters(input): Parameters<responses::CreateResponseInput>,
    ) -> Json<crate::types::ToolResult<responses::ResponseData>> {
        let profile = input.profile.clone();
        self.on_profile(profile.as_deref(), |c| async move {
            responses::create_response(&c, input).await
        })
        .await
    }

    #[tool(
        description = "Start a stateful multi-turn conversation with a persistent system prompt. Returns a response_id to pass to continue_conversation. Only LM Studio (v0.3.29+) and OpenAI support this",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = true
        )
    )]
    async fn start_conversation(
        &self,
        Parameters(input): Parameters<responses::StartConversationInput>,
    ) -> Json<crate::types::ToolResult<responses::ResponseData>> {
        let personas = Arc::clone(&self.personas);
        let profile = input.profile.clone();
        self.on_profile(profile.as_deref(), |c| async move {
            responses::start_conversation(&c, &personas, input).await
        })
        .await
    }

    #[tool(
        description = "Continue a stateful conversation started with start_conversation — the original system prompt stays in effect automatically",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = true
        )
    )]
    async fn continue_conversation(
        &self,
        Parameters(input): Parameters<responses::ContinueConversationInput>,
    ) -> Json<crate::types::ToolResult<responses::ResponseData>> {
        let personas = Arc::clone(&self.personas);
        let profile = input.profile.clone();
        self.on_profile(profile.as_deref(), |c| async move {
            responses::continue_conversation(&c, &personas, input).await
        })
        .await
    }

    #[tool(
        description = "Delegate a task to a model from the configured provider acting as a subagent, with its own sandboxed tools (read_file, list_directory, search_files, and — depending on `capability` — write_file, run_command). Comparable to how Claude Code spawns a subagent: it works autonomously across as many tool calls as it needs and you get back a final report, not the full transcript. Good for offloading well-scoped, lower-level work (investigate a directory, make a specific edit, run and interpret a build/test command) from a smaller/local model instead of doing it yourself.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = true
        )
    )]
    async fn run_subagent(
        &self,
        Parameters(input): Parameters<subagent::RunSubagentInput>,
    ) -> Json<crate::types::ToolResult<crate::subagent::runner::SubagentReport>> {
        let profile = input.profile.clone();
        self.on_profile(profile.as_deref(), |c| async move {
            subagent::run_subagent(&c, input).await
        })
        .await
    }

    #[tool(
        description = "List the configured provider profiles — name, provider, base URL, whether it is the default, whether an API key is set (never the key), and which capabilities its provider supports. Pass a profile's name as `profile` to any other provider tool to run it against that backend; omit `profile` to use the default. Makes no network calls — use health_check to test reachability",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn list_profiles(&self) -> Json<ToolResult<Vec<profiles::ProfileSummary>>> {
        Json(profiles::list_profiles(&self.profiles))
    }

    #[tool(
        description = "Draft a new local feedback entry (issue/error/recommendation) about this server. Stored locally only — nothing leaves this machine until feedback_submit is called.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn feedback_create(
        &self,
        Parameters(input): Parameters<feedback::FeedbackCreateInput>,
    ) -> Json<crate::types::ToolResult<crate::feedback::store::FeedbackEntry>> {
        Json(feedback::feedback_create(&self.feedback_store, input))
    }

    #[tool(
        description = "List all local feedback entries (drafts and submitted).",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn feedback_list(
        &self,
    ) -> Json<crate::types::ToolResult<Vec<crate::feedback::store::FeedbackEntry>>> {
        Json(feedback::feedback_list(&self.feedback_store))
    }

    #[tool(
        description = "Show one local feedback entry in full.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn feedback_get(
        &self,
        Parameters(input): Parameters<feedback::FeedbackIdInput>,
    ) -> Json<crate::types::ToolResult<crate::feedback::store::FeedbackEntry>> {
        Json(feedback::feedback_get(&self.feedback_store, input))
    }

    #[tool(
        description = "Edit a local feedback entry's category, title, and/or body.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn feedback_update(
        &self,
        Parameters(input): Parameters<feedback::FeedbackUpdateInput>,
    ) -> Json<crate::types::ToolResult<crate::feedback::store::FeedbackEntry>> {
        Json(feedback::feedback_update(&self.feedback_store, input))
    }

    #[tool(
        description = "Delete a local feedback entry.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn feedback_delete(
        &self,
        Parameters(input): Parameters<feedback::FeedbackIdInput>,
    ) -> Json<crate::types::ToolResult<()>> {
        Json(feedback::feedback_delete(&self.feedback_store, input))
    }

    #[tool(
        description = "Check a local feedback entry's title against this repo's existing GitHub issues, without submitting anything. feedback_submit runs this same check automatically.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = true
        )
    )]
    async fn feedback_check_duplicates(
        &self,
        Parameters(input): Parameters<feedback::CheckDuplicatesInput>,
    ) -> Json<crate::types::ToolResult<feedback::CheckDuplicatesOutput>> {
        Json(
            feedback::feedback_check_duplicates(
                &self.feedback_store,
                &self.github_http,
                DEFAULT_FEEDBACK_REPO,
                input,
            )
            .await,
        )
    }

    #[tool(
        description = "Submit a local feedback entry to GitHub. Never files the issue directly through the API: first checks for an existing duplicate issue (discarding the local draft without submitting if one matches), then asks the connected client to let a human review — and optionally edit — the title/body via MCP elicitation, then hands the client a pre-filled \"new issue\" page to open via a second elicitation. A human still has to click \"Create\" there. If the client doesn't support elicitation, falls back to opening that page directly in this machine's browser instead.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = true
        )
    )]
    async fn feedback_submit(
        &self,
        Parameters(input): Parameters<feedback::FeedbackSubmitInput>,
        peer: Peer<RoleServer>,
    ) -> Json<crate::types::ToolResult<feedback::FeedbackSubmitOutput>> {
        Json(
            feedback::feedback_submit(
                &self.feedback_store,
                &self.github_http,
                DEFAULT_FEEDBACK_REPO,
                input,
                peer,
            )
            .await,
        )
    }
}

#[tool_handler(
    name = "lmstudio-rs-mcp",
    version = "0.1.0",
    instructions = "Bridge to one or more LLM providers — LM Studio, Ollama, or any OpenAI-compatible server (local), or OpenAI/Anthropic (cloud, require an API key) — configured as named profiles (see list_profiles). Every provider tool takes an optional `profile`; omit it to use the default. Inference (chat, text completion, embeddings, stateful conversations where supported), model management on local providers (list, load, unload), delegating work to a model as a sandboxed subagent (run_subagent), and filing feedback about this server itself as GitHub issues (feedback_*). Run list_profiles to see the backends, then health_check to confirm one is reachable."
)]
impl ServerHandler for LmStudioServer {}
