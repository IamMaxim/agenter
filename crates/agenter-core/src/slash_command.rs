use serde::{Deserialize, Serialize};

use crate::{AgentProviderId, SessionInfo};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SlashCommandDefinition {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    pub description: String,
    pub category: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<AgentProviderId>,
    pub target: SlashCommandTarget,
    pub danger_level: SlashCommandDangerLevel,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub arguments: Vec<SlashCommandArgument>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub examples: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SlashCommandArgument {
    pub name: String,
    pub kind: SlashCommandArgumentKind,
    pub required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SlashCommandArgumentKind {
    String,
    Number,
    Enum,
    Rest,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SlashCommandTarget {
    Local,
    Runner,
    Provider,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SlashCommandDangerLevel {
    Safe,
    Confirm,
    Dangerous,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct SlashCommandRequest {
    pub command_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub universal_command_id: Option<crate::CommandId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
    #[serde(default)]
    pub arguments: serde_json::Value,
    pub raw_input: String,
    #[serde(default)]
    pub confirmed: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct SlashCommandResult {
    pub accepted: bool,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<SessionInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_payload: Option<serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn slash_command_definition_uses_stable_shape() {
        let command = SlashCommandDefinition {
            id: "codex.shell".to_owned(),
            name: "shell".to_owned(),
            aliases: vec!["sh".to_owned()],
            description: "Run a provider-native shell command.".to_owned(),
            category: "provider".to_owned(),
            provider_id: Some(AgentProviderId::from(AgentProviderId::CODEX)),
            target: SlashCommandTarget::Provider,
            danger_level: SlashCommandDangerLevel::Dangerous,
            arguments: vec![SlashCommandArgument {
                name: "command".to_owned(),
                kind: SlashCommandArgumentKind::Rest,
                required: true,
                description: Some("Command to run".to_owned()),
                choices: Vec::new(),
            }],
            examples: vec!["/shell pwd".to_owned()],
        };

        let value = serde_json::to_value(&command).expect("serialize command");

        assert_eq!(value["id"], "codex.shell");
        assert_eq!(value["name"], "shell");
        assert_eq!(value["provider_id"], "codex");
        assert_eq!(value["target"], "provider");
        assert_eq!(value["danger_level"], "dangerous");
        assert_eq!(value["arguments"][0]["kind"], "rest");
    }

    #[test]
    fn slash_command_request_and_result_round_trip() {
        let request = SlashCommandRequest {
            command_id: "local.title".to_owned(),
            universal_command_id: None,
            idempotency_key: None,
            arguments: json!({"title": "New title"}),
            raw_input: "/title New title".to_owned(),
            confirmed: false,
        };
        let decoded: SlashCommandRequest =
            serde_json::from_value(serde_json::to_value(&request).unwrap()).unwrap();
        assert_eq!(decoded, request);

        let result = SlashCommandResult {
            accepted: true,
            message: "Session renamed.".to_owned(),
            session: None,
            provider_payload: Some(json!({"ok": true})),
        };
        let decoded: SlashCommandResult =
            serde_json::from_value(serde_json::to_value(&result).unwrap()).unwrap();
        assert_eq!(decoded, result);
    }
}
