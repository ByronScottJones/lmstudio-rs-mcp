//! create_decision and system_one tools, over the decision-model endpoints:
//! OpenAI-compatible `/v1/decisions` (LM Studio, OpenAI) and LM Studio's
//! native `/v1/systemone`.

use crate::client::ApiClient;
use crate::types::{ErrorCode, ToolResult};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct CreateDecisionInput {
    /// Provider profile to use (see `list_profiles`). Defaults to the
    /// server's default profile.
    #[serde(default)]
    pub profile: Option<String>,
    /// Identifier of a loaded decision model (not a regular chat model).
    pub model: String,
    /// The text the questions are asked about.
    pub input: String,
    /// Question objects, passed through as-is. Each has a `type` (e.g.
    /// `predicate`), a `name`, and type-specific fields such as
    /// `instructions`, e.g. `{"type":"predicate","name":"angry","instructions":"Is the customer angry?"}`.
    pub questions: Vec<Value>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct SystemOneInput {
    /// Provider profile to use (see `list_profiles`). Defaults to the
    /// server's default profile.
    #[serde(default)]
    pub profile: Option<String>,
    /// Identifier of a loaded decision model (not a regular chat model).
    pub model: String,
    /// The text the questions are asked about. Images must be inline base64
    /// data URLs, and the model must support image input.
    pub state: String,
    /// Questions keyed by question ID, passed through as-is. Each has a
    /// `type`: `noul` (probability a condition is true), `choice` (pick
    /// from a `criteria` object), or `score` (rate against an ordered
    /// `criteria` array). Every type requires `instructions`.
    pub questions: Map<String, Value>,
}

/// The raw decision response, passed through as-is: its answer shape varies
/// by question type.
#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct DecisionData {
    pub raw: Value,
}

fn unsupported(client: &ApiClient, tool: &str, supported: &str) -> ToolResult<DecisionData> {
    ToolResult::err(
        format!("{} has no endpoint for {tool}", client.provider()),
        ErrorCode::InvalidInput,
        format!("{tool} is only supported by {supported} profiles"),
    )
}

async fn run(client: &ApiClient, endpoint: &str, body: Value) -> ToolResult<DecisionData> {
    match client.decisions(endpoint, body).await {
        Ok(raw) => ToolResult::ok("Decision made", DecisionData { raw }),
        Err(e) => ToolResult::err(
            format!("Failed to get decision: {e}"),
            e.code(),
            e.to_string(),
        ),
    }
}

pub async fn create_decision(
    client: &ApiClient,
    input: CreateDecisionInput,
) -> ToolResult<DecisionData> {
    if !client.provider().supports_decisions() {
        return unsupported(client, "create_decision", "LM Studio and OpenAI");
    }
    let body = serde_json::json!({
        "model": input.model,
        "input": input.input,
        "questions": input.questions,
    });
    run(client, "decisions", body).await
}

pub async fn system_one(client: &ApiClient, input: SystemOneInput) -> ToolResult<DecisionData> {
    if !client.provider().supports_systemone() {
        return unsupported(client, "system_one", "LM Studio");
    }
    let body = serde_json::json!({
        "model": input.model,
        "state": input.state,
        "questions": input.questions,
    });
    run(client, "systemone", body).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_one_input_keeps_questions_keyed_by_id() {
        let input: SystemOneInput = serde_json::from_value(serde_json::json!({
            "model": "m",
            "state": "I was charged twice.",
            "questions": {"angry": {"type": "noul", "instructions": "Is the customer angry?"}}
        }))
        .expect("valid input");
        assert_eq!(input.questions["angry"]["type"], "noul");
    }

    #[test]
    fn create_decision_input_takes_a_question_list() {
        let input: CreateDecisionInput = serde_json::from_value(serde_json::json!({
            "model": "m",
            "input": "I was charged twice.",
            "questions": [{"type": "predicate", "name": "angry", "instructions": "Is the customer angry?"}]
        }))
        .expect("valid input");
        assert_eq!(input.questions[0]["name"], "angry");
    }
}
