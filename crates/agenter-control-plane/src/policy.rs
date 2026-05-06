use agenter_core::{
    ApprovalDecision, ApprovalKind, ApprovalOption, ApprovalOptionKind, ApprovalPolicyMetadata,
    ApprovalPolicyRulePreview, ApprovalRequest, ApprovalRisk, PolicyAction,
};
use agenter_db::models::ApprovalPolicyRule;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct PolicyInput {
    pub approval_kind: ApprovalKind,
    pub subject: Option<String>,
    pub native_request_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct PolicyDecision {
    pub action: PolicyAction,
    pub risk: ApprovalRisk,
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rewritten_request: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Default)]
pub struct PolicyEngine;

impl PolicyEngine {
    #[must_use]
    pub fn evaluate_approval_request(&self, request: &ApprovalRequest) -> PolicyDecision {
        self.evaluate(PolicyInput {
            approval_kind: request.kind.clone(),
            subject: request.subject.clone().or_else(|| request.details.clone()),
            native_request_id: request.native_request_id.clone(),
        })
    }

    #[must_use]
    pub fn evaluate(&self, input: PolicyInput) -> PolicyDecision {
        let subject = input.subject.as_deref().unwrap_or_default();
        match input.approval_kind {
            ApprovalKind::Command if command_looks_networked(subject) => PolicyDecision {
                action: PolicyAction::Ask,
                risk: ApprovalRisk::High,
                reason: "network-capable shell command requires user approval".to_owned(),
                rewritten_request: None,
            },
            ApprovalKind::Command => PolicyDecision {
                action: PolicyAction::Ask,
                risk: ApprovalRisk::Medium,
                reason: "shell command requires user approval".to_owned(),
                rewritten_request: None,
            },
            ApprovalKind::FileChange => PolicyDecision {
                action: PolicyAction::Ask,
                risk: ApprovalRisk::Medium,
                reason: "file change requires user approval".to_owned(),
                rewritten_request: None,
            },
            ApprovalKind::Permission => PolicyDecision {
                action: PolicyAction::Ask,
                risk: ApprovalRisk::Unknown,
                reason: "provider permission request requires user approval".to_owned(),
                rewritten_request: None,
            },
            ApprovalKind::Tool | ApprovalKind::ProviderSpecific => PolicyDecision {
                action: PolicyAction::Ask,
                risk: ApprovalRisk::Unknown,
                reason: "provider tool request requires user approval".to_owned(),
                rewritten_request: None,
            },
        }
    }

    #[must_use]
    pub fn persistent_rule_options(&self, request: &ApprovalRequest) -> Vec<ApprovalOption> {
        rule_previews_for_request(request)
            .into_iter()
            .enumerate()
            .map(|(index, preview)| ApprovalOption {
                option_id: format!("persist_rule:{index}"),
                kind: ApprovalOptionKind::PersistApprovalRule,
                label: preview.label.clone(),
                description: Some(
                    "Remember this approval for this workspace and provider.".to_owned(),
                ),
                scope: Some("workspace_provider".to_owned()),
                native_option_id: Some(preview.decision.canonical_option_id().to_owned()),
                policy_rule: Some(preview),
            })
            .collect()
    }

    #[must_use]
    pub fn matching_rule<'a>(
        &self,
        request: &ApprovalRequest,
        rules: &'a [ApprovalPolicyRule],
    ) -> Option<&'a ApprovalPolicyRule> {
        rules.iter().find(|rule| {
            rule.disabled_at.is_none()
                && rule.kind == request.kind
                && rule_matches_request(&rule.matcher, request)
        })
    }
}

impl From<PolicyDecision> for ApprovalPolicyMetadata {
    fn from(value: PolicyDecision) -> Self {
        Self {
            action: value.action,
            reason: Some(value.reason),
            policy_id: Some("agenter.default.ask".to_owned()),
            rewritten_request: value.rewritten_request,
        }
    }
}

fn command_looks_networked(subject: &str) -> bool {
    let lower = subject.to_ascii_lowercase();
    ["curl ", "wget ", "ssh ", "scp ", "git clone", "npm install"]
        .iter()
        .any(|needle| lower.contains(needle))
}

fn rule_previews_for_request(request: &ApprovalRequest) -> Vec<ApprovalPolicyRulePreview> {
    let mut previews = Vec::new();
    match request.kind {
        ApprovalKind::Command => {
            if let Some(prefix) = command_prefix_from_request(request) {
                let rendered = prefix.join(" ");
                previews.push(ApprovalPolicyRulePreview {
                    kind: ApprovalKind::Command,
                    matcher: json!({ "type": "command_prefix", "prefix": prefix }),
                    decision: ApprovalDecision::AcceptForSession,
                    label: format!("Approve commands starting with `{rendered}`"),
                });
            }
        }
        ApprovalKind::FileChange => {
            previews.push(ApprovalPolicyRulePreview {
                kind: ApprovalKind::FileChange,
                matcher: json!({ "type": "workspace_file_change" }),
                decision: ApprovalDecision::AcceptForSession,
                label: "Approve file changes in this workspace".to_owned(),
            });
        }
        ApprovalKind::Permission | ApprovalKind::Tool | ApprovalKind::ProviderSpecific => {
            if let Some(method) = native_method(request) {
                previews.push(ApprovalPolicyRulePreview {
                    kind: request.kind.clone(),
                    matcher: json!({ "type": "native_method", "method": method }),
                    decision: ApprovalDecision::AcceptForSession,
                    label: "Approve this provider permission type".to_owned(),
                });
            }
        }
    }
    previews
}

fn command_prefix_from_request(request: &ApprovalRequest) -> Option<Vec<String>> {
    let command = request.subject.as_deref().or(request.details.as_deref())?;
    if command
        .chars()
        .any(|c| matches!(c, '\n' | '\r' | ';' | '&' | '|' | '<' | '>' | '$' | '`'))
    {
        return None;
    }
    let parts = command
        .split_whitespace()
        .take(2)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    (!parts.is_empty()).then_some(parts)
}

fn native_method(request: &ApprovalRequest) -> Option<String> {
    request
        .native
        .as_ref()
        .and_then(|native| native.method.as_deref())
        .or(request.native_request_id.as_deref())
        .map(str::to_owned)
}

fn json_array_to_nonempty_strings(value: &Value) -> Option<Vec<String>> {
    let out = value
        .as_array()?
        .iter()
        .map(Value::as_str)
        .collect::<Option<Vec<_>>>()?
        .into_iter()
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    (!out.is_empty()).then_some(out)
}

fn rule_matches_request(matcher: &Value, request: &ApprovalRequest) -> bool {
    match matcher.get("type").and_then(Value::as_str) {
        Some("command_prefix") => {
            let Some(prefix) = matcher
                .get("prefix")
                .and_then(json_array_to_nonempty_strings)
            else {
                return false;
            };
            let command = request
                .subject
                .as_deref()
                .or(request.details.as_deref())
                .unwrap_or_default();
            command_has_prefix(command, &prefix)
        }
        Some("workspace_file_change") => request.kind == ApprovalKind::FileChange,
        Some("native_method") => matcher
            .get("method")
            .and_then(Value::as_str)
            .is_some_and(|method| native_method(request).as_deref() == Some(method)),
        _ => false,
    }
}

fn command_has_prefix(command: &str, prefix: &[String]) -> bool {
    let command_parts = command.split_whitespace().collect::<Vec<_>>();
    command_parts.len() >= prefix.len()
        && command_parts
            .iter()
            .zip(prefix.iter())
            .all(|(actual, expected)| *actual == expected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agenter_core::{ApprovalId, ApprovalStatus, SessionId};

    fn request(kind: ApprovalKind, details: Option<&str>) -> ApprovalRequest {
        ApprovalRequest {
            session_id: SessionId::nil(),
            approval_id: ApprovalId::nil(),
            turn_id: None,
            item_id: None,
            kind,
            title: "approval".to_owned(),
            details: details.map(str::to_owned),
            options: Vec::new(),
            status: ApprovalStatus::Pending,
            risk: None,
            subject: details.map(str::to_owned),
            native_request_id: None,
            native_blocking: false,
            policy: None,
            native: None,
            requested_at: None,
            resolved_at: None,
            resolving_decision: None,
        }
    }

    #[test]
    fn shell_command_defaults_to_ask_with_medium_risk() {
        let decision = PolicyEngine
            .evaluate_approval_request(&request(ApprovalKind::Command, Some("cargo test")));
        assert_eq!(decision.action, PolicyAction::Ask);
        assert_eq!(decision.risk, ApprovalRisk::Medium);
    }

    #[test]
    fn network_command_is_high_risk_ask() {
        let decision = PolicyEngine
            .evaluate_approval_request(&request(ApprovalKind::Command, Some("curl example.test")));
        assert_eq!(decision.action, PolicyAction::Ask);
        assert_eq!(decision.risk, ApprovalRisk::High);
    }

    #[test]
    fn derives_command_prefix_persistent_option() {
        let mut request = request(ApprovalKind::Command, Some("cargo test -p agenter-db"));
        request.subject = Some("cargo test -p agenter-db".to_owned());

        let options = PolicyEngine.persistent_rule_options(&request);

        assert_eq!(options.len(), 1);
        assert_eq!(options[0].kind, ApprovalOptionKind::PersistApprovalRule);
        assert_eq!(
            options[0]
                .policy_rule
                .as_ref()
                .expect("rule preview")
                .matcher,
            json!({ "type": "command_prefix", "prefix": ["cargo", "test"] })
        );
    }

    #[test]
    fn command_prefix_rule_matches_later_command() {
        let mut request = request(ApprovalKind::Command, Some("cargo test -p agenter-core"));
        request.subject = Some("cargo test -p agenter-core".to_owned());

        assert!(super::rule_matches_request(
            &json!({ "type": "command_prefix", "prefix": ["cargo", "test"] }),
            &request
        ));
        assert!(!super::rule_matches_request(
            &json!({ "type": "command_prefix", "prefix": ["cargo", "build"] }),
            &request
        ));
    }
}
