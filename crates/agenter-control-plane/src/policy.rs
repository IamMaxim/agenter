use agenter_core::{
    ApprovalKind, ApprovalPolicyMetadata, ApprovalRequestEvent, ApprovalRisk, PolicyAction,
};
use serde::{Deserialize, Serialize};

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
    pub fn evaluate_approval_request(&self, request: &ApprovalRequestEvent) -> PolicyDecision {
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
            ApprovalKind::Tool | ApprovalKind::ProviderSpecific => PolicyDecision {
                action: PolicyAction::Ask,
                risk: ApprovalRisk::Unknown,
                reason: "provider tool request requires user approval".to_owned(),
                rewritten_request: None,
            },
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use agenter_core::{ApprovalId, SessionId};

    fn request(kind: ApprovalKind, details: Option<&str>) -> ApprovalRequestEvent {
        ApprovalRequestEvent {
            session_id: SessionId::nil(),
            approval_id: ApprovalId::nil(),
            kind,
            title: "approval".to_owned(),
            details: details.map(str::to_owned),
            expires_at: None,
            presentation: None,
            resolution_state: None,
            resolving_decision: None,
            status: None,
            turn_id: None,
            item_id: None,
            options: Vec::new(),
            risk: None,
            subject: details.map(str::to_owned),
            native_request_id: None,
            native_blocking: false,
            policy: None,
            provider_payload: None,
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
}
