use agenter_core::{
    AgentProviderId, ApprovalDecision, ApprovalKind, ApprovalMode, ApprovalOption,
    ApprovalOptionKind, ApprovalPolicyMetadata, ApprovalPolicyRulePreview, ApprovalRequest,
    ApprovalRisk, PolicyAction, WorkspaceId,
};
use agenter_db::models::ApprovalPolicyRule;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct PolicyInput {
    pub approval_kind: ApprovalKind,
    pub approval_mode: ApprovalMode,
    pub provider_id: Option<AgentProviderId>,
    pub workspace_id: Option<WorkspaceId>,
    pub subject: Option<String>,
    pub details: Option<String>,
    pub native_request_id: Option<String>,
    pub native_method: Option<String>,
    pub command: Option<Vec<String>>,
    pub cwd: Option<String>,
    pub grant_root: Option<String>,
    pub permission_profile: Option<Value>,
    pub raw_request_payload_hash: Option<String>,
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
        self.evaluate(PolicyInput::from(request))
    }

    #[must_use]
    pub fn evaluate(&self, input: PolicyInput) -> PolicyDecision {
        let subject = input.subject.as_deref().unwrap_or_default();
        if input.approval_mode == ApprovalMode::AllowAllSession {
            return PolicyDecision {
                action: PolicyAction::Allow,
                risk: ApprovalRisk::High,
                reason: "allow_all_session auto-approves every approval for this session"
                    .to_owned(),
                rewritten_request: None,
            };
        }
        if input.approval_mode == ApprovalMode::AllowAllWorkspace {
            return PolicyDecision {
                action: PolicyAction::Allow,
                risk: ApprovalRisk::High,
                reason:
                    "allow_all_workspace auto-approves every approval for this workspace/provider"
                        .to_owned(),
                rewritten_request: None,
            };
        }
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
                description: Some(persistent_rule_description(&preview)),
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
                && (rule.kind == request.kind || rule_matches_all_kinds(&rule.matcher))
                && rule_matches_request(&rule.matcher, request)
        })
    }
}

fn persistent_rule_description(preview: &ApprovalPolicyRulePreview) -> String {
    if rule_matches_all_kinds(&preview.matcher) {
        "Danger: remember allow-all for this workspace/provider. This applies to command, file, permission, tool, and provider-specific approvals."
            .to_owned()
    } else {
        "Remember this approval for this workspace and provider.".to_owned()
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
    let mut previews = vec![ApprovalPolicyRulePreview {
        kind: ApprovalKind::ProviderSpecific,
        matcher: allow_all_matcher(),
        decision: ApprovalDecision::AcceptForSession,
        label: "Danger: allow all operations for this workspace/provider".to_owned(),
    }];
    match request.kind {
        ApprovalKind::Command => {
            if let Some(command) = command_vector_from_request(request) {
                previews.push(ApprovalPolicyRulePreview {
                    kind: ApprovalKind::Command,
                    matcher: json!({
                        "type": "command_exact",
                        "command": command,
                        "cwd": cwd_from_request(request),
                    }),
                    decision: ApprovalDecision::AcceptForSession,
                    label: "Approve this exact command".to_owned(),
                });
            }
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
            if let Some(root) = grant_root_from_request(request) {
                previews.push(ApprovalPolicyRulePreview {
                    kind: ApprovalKind::FileChange,
                    matcher: json!({ "type": "file_change_root", "root": root }),
                    decision: ApprovalDecision::AcceptForSession,
                    label: "Approve file changes under this path".to_owned(),
                });
            }
        }
        ApprovalKind::Permission | ApprovalKind::Tool | ApprovalKind::ProviderSpecific => {
            if let Some(permission) = permission_profile_from_request(request) {
                previews.push(ApprovalPolicyRulePreview {
                    kind: request.kind.clone(),
                    matcher: json!({ "type": "permission_profile", "profile": permission }),
                    decision: ApprovalDecision::AcceptForSession,
                    label: "Approve this provider permission profile".to_owned(),
                });
            }
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
    let command = command_vector_from_request(request)?;
    command_prefix_allowed(&command).then(|| command.into_iter().take(2).collect())
}

fn safe_command_parts_from_request(
    request: &ApprovalRequest,
    max_parts: Option<usize>,
) -> Option<Vec<String>> {
    let command = request.subject.as_deref().or(request.details.as_deref())?;
    if command
        .chars()
        .any(|c| matches!(c, '\n' | '\r' | ';' | '&' | '|' | '<' | '>' | '$' | '`'))
    {
        return None;
    }
    let mut parts = command.split_whitespace();
    let parts: Vec<String> = match max_parts {
        Some(max_parts) => parts.by_ref().take(max_parts).map(str::to_owned).collect(),
        None => parts.map(str::to_owned).collect(),
    };
    (!parts.is_empty()).then_some(parts)
}

fn command_vector_from_request(request: &ApprovalRequest) -> Option<Vec<String>> {
    raw_payload_command_vector(request).or_else(|| safe_command_parts_from_request(request, None))
}

fn raw_payload_command_vector(request: &ApprovalRequest) -> Option<Vec<String>> {
    let payload = raw_payload(request)?;
    for (keys, trusted_argv) in [
        (&["argv", "parsed_argv", "parsedArgv"][..], true),
        (&["parsed_cmd", "parsedCmd", "command"][..], false),
    ] {
        let Some(command) =
            value_at_any_path(payload, keys).and_then(json_array_to_nonempty_strings)
        else {
            continue;
        };
        if !trusted_argv && command.iter().any(|part| shell_control_token(part)) {
            return None;
        }
        return Some(command);
    }
    None
}

fn cwd_from_request(request: &ApprovalRequest) -> Option<String> {
    raw_payload(request)
        .and_then(|payload| value_at_any_path(payload, &["cwd", "workdir"]))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn grant_root_from_request(request: &ApprovalRequest) -> Option<String> {
    raw_payload(request)
        .and_then(|payload| {
            value_at_any_path(
                payload,
                &["grant_root", "grantRoot", "root", "path", "workspaceRoot"],
            )
        })
        .and_then(Value::as_str)
        .or(request.subject.as_deref())
        .or(request.details.as_deref())
        .map(str::to_owned)
}

fn permission_profile_from_request(request: &ApprovalRequest) -> Option<Value> {
    raw_payload(request)
        .and_then(|payload| {
            value_at_any_path(
                payload,
                &[
                    "permission_profile",
                    "permissionProfile",
                    "permission",
                    "permissions",
                    "sandbox",
                    "sandboxMode",
                ],
            )
        })
        .cloned()
}

fn native_method(request: &ApprovalRequest) -> Option<String> {
    request
        .native
        .as_ref()
        .and_then(|native| native.method.as_deref())
        .or(request.native_request_id.as_deref())
        .map(str::to_owned)
}

fn raw_payload(request: &ApprovalRequest) -> Option<&Value> {
    request
        .native
        .as_ref()
        .and_then(|native| native.raw_payload.as_ref())
}

fn value_at_any_path<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a Value> {
    for key in keys {
        if let Some(found) = value.get(*key) {
            return Some(found);
        }
        if let Some(found) = value.get("params").and_then(|params| params.get(*key)) {
            return Some(found);
        }
        if let Some(found) = value.get("request").and_then(|request| request.get(*key)) {
            return Some(found);
        }
    }
    None
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
        Some("allow_all") => true,
        Some("command_exact") => command_exact_matches(matcher, request),
        Some("command_prefix") => {
            let Some(prefix) = matcher
                .get("prefix")
                .and_then(json_array_to_nonempty_strings)
            else {
                return false;
            };
            command_vector_from_request(request)
                .as_ref()
                .is_some_and(|command| {
                    command_prefix_allowed(command) && command_vector_has_prefix(command, &prefix)
                })
        }
        Some("workspace_file_change") => false,
        Some("file_change_root") => matcher
            .get("root")
            .and_then(Value::as_str)
            .zip(grant_root_from_request(request))
            .is_some_and(|(root, grant_root)| path_is_at_or_under(&grant_root, root)),
        Some("native_method") => matcher
            .get("method")
            .and_then(Value::as_str)
            .is_some_and(|method| native_method(request).as_deref() == Some(method)),
        Some("permission_profile") => matcher.get("profile").is_some_and(|profile| {
            permission_profile_from_request(request).as_ref() == Some(profile)
        }),
        _ => false,
    }
}

fn rule_matches_all_kinds(matcher: &Value) -> bool {
    matcher.get("type").and_then(Value::as_str) == Some("allow_all")
}

fn command_exact_matches(matcher: &Value, request: &ApprovalRequest) -> bool {
    let Some(expected) = matcher
        .get("command")
        .and_then(json_array_to_nonempty_strings)
    else {
        return false;
    };
    if command_vector_from_request(request).as_ref() != Some(&expected) {
        return false;
    }
    match matcher.get("cwd").and_then(Value::as_str) {
        Some(expected_cwd) => cwd_from_request(request).as_deref() == Some(expected_cwd),
        None => true,
    }
}

fn command_vector_has_prefix(command: &[String], prefix: &[String]) -> bool {
    command.len() >= prefix.len()
        && command
            .iter()
            .zip(prefix.iter())
            .all(|(actual, expected)| actual == expected)
}

fn command_prefix_allowed(command: &[String]) -> bool {
    let command = unwrap_env_command(command);
    let Some(program) = command.first().map(|part| part.as_str()) else {
        return false;
    };
    let flag = command.get(1).map(|part| part.as_str());
    let lower_program = program
        .rsplit('/')
        .next()
        .unwrap_or(program)
        .to_ascii_lowercase();
    !matches!(
        (lower_program.as_str(), flag),
        ("sh", Some("-c"))
            | ("bash", Some("-c" | "-lc"))
            | ("zsh", Some("-c" | "-lc"))
            | ("python", Some("-c"))
            | ("python3", Some("-c"))
            | ("node", Some("-e"))
            | ("ruby", Some("-e"))
            | ("perl", Some("-e"))
    )
}

fn unwrap_env_command(command: &[String]) -> &[String] {
    let Some(program) = command.first().map(|part| part.as_str()) else {
        return command;
    };
    let basename = program.rsplit('/').next().unwrap_or(program);
    if basename != "env" {
        return command;
    }
    let mut index = 1;
    while let Some(part) = command.get(index).map(String::as_str) {
        if part.contains('=') || (part.starts_with('-') && part != "-") {
            index += 1;
            continue;
        }
        break;
    }
    &command[index..]
}

fn path_is_at_or_under(path: &str, root: &str) -> bool {
    let Some(path) = normal_absolute_path_segments(path) else {
        return false;
    };
    let Some(root) = normal_absolute_path_segments(root) else {
        return false;
    };
    path.len() >= root.len()
        && path
            .iter()
            .zip(root.iter())
            .all(|(path, root)| path == root)
}

fn normal_absolute_path_segments(path: &str) -> Option<Vec<&str>> {
    if !path.starts_with('/') {
        return None;
    }
    let mut segments = Vec::new();
    for segment in path.split('/').skip(1) {
        if segment.is_empty() || segment == "." || segment == ".." {
            return None;
        }
        segments.push(segment);
    }
    (!segments.is_empty()).then_some(segments)
}

fn shell_control_token(part: &str) -> bool {
    part.chars()
        .any(|c| matches!(c, '\n' | '\r' | ';' | '&' | '|' | '<' | '>' | '$' | '`'))
}

fn allow_all_matcher() -> Value {
    json!({ "type": "allow_all", "applies_to": "all_approval_kinds" })
}

impl From<&ApprovalRequest> for PolicyInput {
    fn from(request: &ApprovalRequest) -> Self {
        let raw_payload_hash = raw_payload(request).and_then(|payload| {
            serde_json::to_vec(payload).ok().map(|bytes| {
                use sha2::{Digest, Sha256};
                format!("{:x}", Sha256::digest(&bytes))
            })
        });
        Self {
            approval_kind: request.kind.clone(),
            approval_mode: ApprovalMode::Ask,
            provider_id: None,
            workspace_id: None,
            subject: request.subject.clone().or_else(|| request.details.clone()),
            details: request.details.clone(),
            native_request_id: request.native_request_id.clone(),
            native_method: native_method(request),
            command: command_vector_from_request(request),
            cwd: cwd_from_request(request),
            grant_root: grant_root_from_request(request),
            permission_profile: permission_profile_from_request(request),
            raw_request_payload_hash: raw_payload_hash,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agenter_core::{ApprovalId, ApprovalStatus, NativeRef, SessionId, UserId, WorkspaceId};
    use chrono::Utc;
    use uuid::Uuid;

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

    fn request_with_native(kind: ApprovalKind, payload: Value) -> ApprovalRequest {
        let mut request = request(kind, None);
        request.native = Some(NativeRef {
            protocol: "codex".to_owned(),
            method: Some("item/permissions/requestApproval".to_owned()),
            kind: None,
            native_id: Some("native-1".to_owned()),
            summary: None,
            hash: None,
            pointer: None,
            raw_payload: Some(payload),
        });
        request
    }

    fn rule(kind: ApprovalKind, matcher: Value) -> ApprovalPolicyRule {
        ApprovalPolicyRule {
            rule_id: Uuid::new_v4(),
            owner_user_id: UserId::nil(),
            workspace_id: WorkspaceId::nil(),
            provider_id: AgentProviderId::from("codex"),
            kind,
            label: "rule".to_owned(),
            matcher,
            decision: ApprovalDecision::AcceptForSession,
            source_approval_id: None,
            created_by_user_id: None,
            disabled_by_user_id: None,
            disabled_at: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn allow_all_session_auto_allows_by_default() {
        let decision = PolicyEngine.evaluate(PolicyInput {
            approval_kind: ApprovalKind::Command,
            approval_mode: ApprovalMode::AllowAllSession,
            provider_id: None,
            workspace_id: None,
            subject: Some("curl example.test".to_owned()),
            details: None,
            native_request_id: None,
            native_method: None,
            command: None,
            cwd: None,
            grant_root: None,
            permission_profile: None,
            raw_request_payload_hash: None,
        });

        assert_eq!(decision.action, PolicyAction::Allow);
        assert_eq!(decision.risk, ApprovalRisk::High);
    }

    #[test]
    fn allow_all_rule_matches_every_approval_kind() {
        let rules = vec![rule(
            ApprovalKind::ProviderSpecific,
            json!({ "type": "allow_all", "applies_to": "all_approval_kinds" }),
        )];

        for kind in [
            ApprovalKind::Command,
            ApprovalKind::FileChange,
            ApprovalKind::Permission,
            ApprovalKind::Tool,
            ApprovalKind::ProviderSpecific,
        ] {
            assert!(PolicyEngine
                .matching_rule(&request(kind, Some("anything")), &rules)
                .is_some());
        }
    }

    #[test]
    fn command_exact_matches_command_vector_and_cwd() {
        let request = request_with_native(
            ApprovalKind::Command,
            json!({ "parsed_cmd": ["cargo", "test"], "cwd": "/repo" }),
        );

        assert!(super::rule_matches_request(
            &json!({ "type": "command_exact", "command": ["cargo", "test"], "cwd": "/repo" }),
            &request
        ));
        assert!(!super::rule_matches_request(
            &json!({ "type": "command_exact", "command": ["cargo", "check"], "cwd": "/repo" }),
            &request
        ));
        assert!(!super::rule_matches_request(
            &json!({ "type": "command_exact", "command": ["cargo", "test"], "cwd": "/other" }),
            &request
        ));
    }

    #[test]
    fn command_exact_fallback_uses_full_safe_subject() {
        let request = request(ApprovalKind::Command, Some("cargo test -p agenter-core"));

        assert!(super::rule_matches_request(
            &json!({ "type": "command_exact", "command": ["cargo", "test", "-p", "agenter-core"] }),
            &request
        ));
        assert!(!super::rule_matches_request(
            &json!({ "type": "command_exact", "command": ["cargo", "test"] }),
            &request
        ));
    }

    #[test]
    fn command_prefix_uses_token_boundaries() {
        let mut request = request(ApprovalKind::Command, Some("cargo test -p agenter-core"));
        request.subject = Some("cargo test -p agenter-core".to_owned());

        assert!(super::rule_matches_request(
            &json!({ "type": "command_prefix", "prefix": ["cargo", "test"] }),
            &request
        ));
        assert!(!super::rule_matches_request(
            &json!({ "type": "command_prefix", "prefix": ["car"] }),
            &request
        ));
    }

    #[test]
    fn command_prefix_rejects_unsafe_shell_subjects() {
        for subject in [
            "cargo test && curl example.test",
            "cargo test -p foo; rm -rf /",
        ] {
            let request = request(ApprovalKind::Command, Some(subject));

            assert!(!super::rule_matches_request(
                &json!({ "type": "command_prefix", "prefix": ["cargo", "test"] }),
                &request
            ));
        }
    }

    #[test]
    fn command_prefix_rejects_generic_raw_vectors_with_shell_control_tokens() {
        let request = request_with_native(
            ApprovalKind::Command,
            json!({ "parsed_cmd": ["cargo", "test", "&&", "curl", "example.test"] }),
        );

        assert!(!super::rule_matches_request(
            &json!({ "type": "command_prefix", "prefix": ["cargo", "test"] }),
            &request
        ));
    }

    #[test]
    fn command_prefix_rejects_shell_interpreter_execution_forms() {
        for command in [
            "sh -c cargo test",
            "bash -c cargo test",
            "bash -lc cargo test",
            "zsh -c cargo test",
            "zsh -lc cargo test",
            "python -c print(1)",
            "python3 -c print(1)",
            "node -e console.log(1)",
            "ruby -e puts 1",
            "perl -e print 1",
            "/usr/bin/env bash -lc cargo test",
            "env sh -c cargo test",
            "env python -c print(1)",
        ] {
            let request = request(ApprovalKind::Command, Some(command));
            let options = PolicyEngine.persistent_rule_options(&request);

            assert!(!options.iter().any(|option| {
                option.policy_rule.as_ref().is_some_and(|preview| {
                    preview.matcher.get("type").and_then(Value::as_str) == Some("command_prefix")
                })
            }));
            assert!(!super::rule_matches_request(
                &json!({ "type": "command_prefix", "prefix": command.split_whitespace().take(2).collect::<Vec<_>>() }),
                &request
            ));
        }
    }

    #[test]
    fn file_change_root_matches_descendant_paths() {
        let request = request_with_native(
            ApprovalKind::FileChange,
            json!({ "grantRoot": "/repo/src/main.rs" }),
        );

        assert!(super::rule_matches_request(
            &json!({ "type": "file_change_root", "root": "/repo/src" }),
            &request
        ));
        assert!(!super::rule_matches_request(
            &json!({ "type": "file_change_root", "root": "/repo/tests" }),
            &request
        ));
    }

    #[test]
    fn file_change_root_rejects_traversal_escape() {
        let request = request_with_native(
            ApprovalKind::FileChange,
            json!({ "grantRoot": "/repo/src/../secrets" }),
        );

        assert!(!super::rule_matches_request(
            &json!({ "type": "file_change_root", "root": "/repo/src" }),
            &request
        ));
        assert!(!super::rule_matches_request(
            &json!({ "type": "file_change_root", "root": "/repo/src/.." }),
            &request
        ));
    }

    #[test]
    fn file_change_does_not_generate_unbounded_workspace_rule_preview() {
        let request = request(ApprovalKind::FileChange, Some("changed files"));

        let options = PolicyEngine.persistent_rule_options(&request);

        assert!(!options.iter().any(|option| {
            option.policy_rule.as_ref().is_some_and(|preview| {
                preview.matcher.get("type").and_then(Value::as_str) == Some("workspace_file_change")
            })
        }));
    }

    #[test]
    fn stale_workspace_file_change_rule_does_not_match() {
        let request = request(ApprovalKind::FileChange, Some("/repo/src"));

        assert!(!super::rule_matches_request(
            &json!({ "type": "workspace_file_change" }),
            &request
        ));
    }

    #[test]
    fn native_method_rule_matches_request_method() {
        let request = request_with_native(ApprovalKind::Permission, json!({}));

        assert!(super::rule_matches_request(
            &json!({ "type": "native_method", "method": "item/permissions/requestApproval" }),
            &request
        ));
    }

    #[test]
    fn permission_profile_rule_matches_stable_profile_json() {
        let request = request_with_native(
            ApprovalKind::Permission,
            json!({ "permissionProfile": { "sandboxMode": "danger-full-access" } }),
        );

        assert!(super::rule_matches_request(
            &json!({
                "type": "permission_profile",
                "profile": { "sandboxMode": "danger-full-access" }
            }),
            &request
        ));
        assert!(!super::rule_matches_request(
            &json!({
                "type": "permission_profile",
                "profile": { "sandboxMode": "workspace-write" }
            }),
            &request
        ));
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

        assert!(options
            .iter()
            .all(|option| option.kind == ApprovalOptionKind::PersistApprovalRule));
        let prefix_option = options
            .iter()
            .find(|option| {
                option.policy_rule.as_ref().is_some_and(|preview| {
                    preview.matcher.get("type").and_then(Value::as_str) == Some("command_prefix")
                })
            })
            .expect("command prefix preview");
        assert_eq!(
            prefix_option
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
