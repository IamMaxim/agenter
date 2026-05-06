#![allow(dead_code)]

use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CodexProviderCommand {
    pub method: &'static str,
    pub category: CodexProviderCommandCategory,
    pub label: &'static str,
    pub schema: CodexProviderCommandSchema,
    pub response_kind: CodexProviderCommandResponseKind,
    pub availability: CodexProviderCommandAvailability,
    pub raw_payload_display: RawPayloadDisplay,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CodexProviderCommandSchema {
    pub params_type: &'static str,
    pub response_type: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CodexProviderCommandCategory {
    ThreadMaintenance,
    Memory,
    SkillsPluginsApps,
    Filesystem,
    AccountDeviceAuth,
    ConfigMcp,
    Realtime,
    Feedback,
    ShellBackgroundTerminals,
    ExternalAgentConfig,
    FuzzySearch,
    WindowsSandbox,
    MockExperimental,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CodexProviderCommandResponseKind {
    CommandResult,
    QueryResult,
    SessionMutation,
    ProviderNotification,
    StreamHandle,
    StreamingUpdate,
    LongRunning,
    UnsupportedCompatibility,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CodexProviderCommandAvailability {
    Supported,
    Guarded,
    Experimental,
    PlatformGated,
    Disabled,
    Unsupported,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RawPayloadDisplay {
    Always,
    Never,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CodexClientRequestDisposition {
    pub method: &'static str,
    pub disposition: CodexClientRequestDispositionKind,
    pub reason: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CodexClientRequestDispositionKind {
    Core,
    ProviderCommand,
    AdapterInternal,
    Deferred,
}

impl CodexProviderCommandCategory {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ThreadMaintenance => "thread_maintenance",
            Self::Memory => "memory",
            Self::SkillsPluginsApps => "skills_plugins_apps",
            Self::Filesystem => "filesystem",
            Self::AccountDeviceAuth => "account_device_auth",
            Self::ConfigMcp => "config_mcp",
            Self::Realtime => "realtime",
            Self::Feedback => "feedback",
            Self::ShellBackgroundTerminals => "shell_background_terminals",
            Self::ExternalAgentConfig => "external_agent_config",
            Self::FuzzySearch => "fuzzy_search",
            Self::WindowsSandbox => "windows_sandbox",
            Self::MockExperimental => "mock_experimental",
        }
    }
}

const fn schema(
    params_type: &'static str,
    response_type: &'static str,
) -> CodexProviderCommandSchema {
    CodexProviderCommandSchema {
        params_type,
        response_type,
    }
}

const fn command(
    method: &'static str,
    category: CodexProviderCommandCategory,
    label: &'static str,
    schema: CodexProviderCommandSchema,
    response_kind: CodexProviderCommandResponseKind,
    availability: CodexProviderCommandAvailability,
) -> CodexProviderCommand {
    CodexProviderCommand {
        method,
        category,
        label,
        schema,
        response_kind,
        availability,
        raw_payload_display: RawPayloadDisplay::Always,
    }
}

const fn disposition(
    method: &'static str,
    disposition: CodexClientRequestDispositionKind,
    reason: &'static str,
) -> CodexClientRequestDisposition {
    CodexClientRequestDisposition {
        method,
        disposition,
        reason,
    }
}

pub const CORE_CLIENT_REQUESTS: &[CodexClientRequestDisposition] = &[
    disposition(
        "thread/start",
        CodexClientRequestDispositionKind::Core,
        "UniversalCommand::StartSession owns Codex thread creation.",
    ),
    disposition(
        "thread/resume",
        CodexClientRequestDispositionKind::Core,
        "UniversalCommand::LoadSession owns Codex thread resume.",
    ),
    disposition(
        "thread/fork",
        CodexClientRequestDispositionKind::Core,
        "UniversalCommand::ForkSession owns Codex thread fork.",
    ),
    disposition(
        "turn/start",
        CodexClientRequestDispositionKind::Core,
        "UniversalCommand::StartTurn owns Codex turn creation.",
    ),
    disposition(
        "turn/steer",
        CodexClientRequestDispositionKind::Core,
        "UniversalCommand::SendUserInput owns steering active Codex turns.",
    ),
    disposition(
        "turn/interrupt",
        CodexClientRequestDispositionKind::Core,
        "UniversalCommand::CancelTurn owns Codex turn interruption.",
    ),
];

pub const ADAPTER_INTERNAL_CLIENT_REQUESTS: &[CodexClientRequestDisposition] = &[
    disposition(
        "initialize",
        CodexClientRequestDispositionKind::AdapterInternal,
        "Transport startup and capability discovery only.",
    ),
    disposition(
        "thread/unsubscribe",
        CodexClientRequestDispositionKind::AdapterInternal,
        "Adapter session close/unsubscribe path, not a public chat command by default.",
    ),
    disposition(
        "thread/list",
        CodexClientRequestDispositionKind::AdapterInternal,
        "Adapter discovery/import path.",
    ),
    disposition(
        "thread/loaded/list",
        CodexClientRequestDispositionKind::AdapterInternal,
        "Adapter loaded-session discovery path.",
    ),
    disposition(
        "thread/read",
        CodexClientRequestDispositionKind::AdapterInternal,
        "Adapter history and metadata reconciliation path.",
    ),
    disposition(
        "thread/turns/list",
        CodexClientRequestDispositionKind::AdapterInternal,
        "Adapter paged history import path.",
    ),
];

pub const DEFERRED_CLIENT_REQUESTS: &[CodexClientRequestDisposition] = &[
    disposition(
        "getConversationSummary",
        CodexClientRequestDispositionKind::Deferred,
        "Deprecated v1 compatibility API; no new universal event or provider command should depend on it.",
    ),
    disposition(
        "gitDiffToRemote",
        CodexClientRequestDispositionKind::Deferred,
        "Deprecated v1 compatibility API; use current diff events and explicit provider notifications instead.",
    ),
    disposition(
        "getAuthStatus",
        CodexClientRequestDispositionKind::Deferred,
        "Deprecated v1 compatibility API; account/read is the current provider command surface.",
    ),
];

pub const PROVIDER_COMMANDS: &[CodexProviderCommand] = &[
    command(
        "thread/archive",
        CodexProviderCommandCategory::ThreadMaintenance,
        "Archive thread",
        schema("v2::ThreadArchiveParams", "v2::ThreadArchiveResponse"),
        CodexProviderCommandResponseKind::SessionMutation,
        CodexProviderCommandAvailability::Guarded,
    ),
    command(
        "thread/increment_elicitation",
        CodexProviderCommandCategory::ThreadMaintenance,
        "Increment elicitation hold",
        schema(
            "v2::ThreadIncrementElicitationParams",
            "v2::ThreadIncrementElicitationResponse",
        ),
        CodexProviderCommandResponseKind::ProviderNotification,
        CodexProviderCommandAvailability::Experimental,
    ),
    command(
        "thread/decrement_elicitation",
        CodexProviderCommandCategory::ThreadMaintenance,
        "Decrement elicitation hold",
        schema(
            "v2::ThreadDecrementElicitationParams",
            "v2::ThreadDecrementElicitationResponse",
        ),
        CodexProviderCommandResponseKind::ProviderNotification,
        CodexProviderCommandAvailability::Experimental,
    ),
    command(
        "thread/name/set",
        CodexProviderCommandCategory::ThreadMaintenance,
        "Rename thread",
        schema("v2::ThreadSetNameParams", "v2::ThreadSetNameResponse"),
        CodexProviderCommandResponseKind::SessionMutation,
        CodexProviderCommandAvailability::Supported,
    ),
    command(
        "thread/goal/set",
        CodexProviderCommandCategory::ThreadMaintenance,
        "Set thread goal",
        schema("v2::ThreadGoalSetParams", "v2::ThreadGoalSetResponse"),
        CodexProviderCommandResponseKind::ProviderNotification,
        CodexProviderCommandAvailability::Experimental,
    ),
    command(
        "thread/goal/get",
        CodexProviderCommandCategory::ThreadMaintenance,
        "Read thread goal",
        schema("v2::ThreadGoalGetParams", "v2::ThreadGoalGetResponse"),
        CodexProviderCommandResponseKind::QueryResult,
        CodexProviderCommandAvailability::Experimental,
    ),
    command(
        "thread/goal/clear",
        CodexProviderCommandCategory::ThreadMaintenance,
        "Clear thread goal",
        schema("v2::ThreadGoalClearParams", "v2::ThreadGoalClearResponse"),
        CodexProviderCommandResponseKind::ProviderNotification,
        CodexProviderCommandAvailability::Experimental,
    ),
    command(
        "thread/metadata/update",
        CodexProviderCommandCategory::ThreadMaintenance,
        "Update thread metadata",
        schema(
            "v2::ThreadMetadataUpdateParams",
            "v2::ThreadMetadataUpdateResponse",
        ),
        CodexProviderCommandResponseKind::SessionMutation,
        CodexProviderCommandAvailability::Supported,
    ),
    command(
        "thread/unarchive",
        CodexProviderCommandCategory::ThreadMaintenance,
        "Unarchive thread",
        schema("v2::ThreadUnarchiveParams", "v2::ThreadUnarchiveResponse"),
        CodexProviderCommandResponseKind::SessionMutation,
        CodexProviderCommandAvailability::Guarded,
    ),
    command(
        "thread/compact/start",
        CodexProviderCommandCategory::ThreadMaintenance,
        "Start compaction",
        schema(
            "v2::ThreadCompactStartParams",
            "v2::ThreadCompactStartResponse",
        ),
        CodexProviderCommandResponseKind::LongRunning,
        CodexProviderCommandAvailability::Supported,
    ),
    command(
        "thread/approveGuardianDeniedAction",
        CodexProviderCommandCategory::ThreadMaintenance,
        "Approve guardian denied action",
        schema(
            "v2::ThreadApproveGuardianDeniedActionParams",
            "v2::ThreadApproveGuardianDeniedActionResponse",
        ),
        CodexProviderCommandResponseKind::ProviderNotification,
        CodexProviderCommandAvailability::Guarded,
    ),
    command(
        "thread/rollback",
        CodexProviderCommandCategory::ThreadMaintenance,
        "Rollback thread",
        schema("v2::ThreadRollbackParams", "v2::ThreadRollbackResponse"),
        CodexProviderCommandResponseKind::SessionMutation,
        CodexProviderCommandAvailability::Guarded,
    ),
    command(
        "thread/inject_items",
        CodexProviderCommandCategory::ThreadMaintenance,
        "Inject native items",
        schema(
            "v2::ThreadInjectItemsParams",
            "v2::ThreadInjectItemsResponse",
        ),
        CodexProviderCommandResponseKind::SessionMutation,
        CodexProviderCommandAvailability::Guarded,
    ),
    command(
        "thread/memoryMode/set",
        CodexProviderCommandCategory::Memory,
        "Set memory mode",
        schema(
            "v2::ThreadMemoryModeSetParams",
            "v2::ThreadMemoryModeSetResponse",
        ),
        CodexProviderCommandResponseKind::ProviderNotification,
        CodexProviderCommandAvailability::Experimental,
    ),
    command(
        "memory/reset",
        CodexProviderCommandCategory::Memory,
        "Reset memory",
        schema("undefined", "v2::MemoryResetResponse"),
        CodexProviderCommandResponseKind::CommandResult,
        CodexProviderCommandAvailability::Guarded,
    ),
    command(
        "skills/list",
        CodexProviderCommandCategory::SkillsPluginsApps,
        "List skills",
        schema("v2::SkillsListParams", "v2::SkillsListResponse"),
        CodexProviderCommandResponseKind::QueryResult,
        CodexProviderCommandAvailability::Supported,
    ),
    command(
        "hooks/list",
        CodexProviderCommandCategory::SkillsPluginsApps,
        "List hooks",
        schema("v2::HooksListParams", "v2::HooksListResponse"),
        CodexProviderCommandResponseKind::QueryResult,
        CodexProviderCommandAvailability::Supported,
    ),
    command(
        "marketplace/add",
        CodexProviderCommandCategory::SkillsPluginsApps,
        "Add marketplace item",
        schema("v2::MarketplaceAddParams", "v2::MarketplaceAddResponse"),
        CodexProviderCommandResponseKind::CommandResult,
        CodexProviderCommandAvailability::Guarded,
    ),
    command(
        "marketplace/remove",
        CodexProviderCommandCategory::SkillsPluginsApps,
        "Remove marketplace item",
        schema(
            "v2::MarketplaceRemoveParams",
            "v2::MarketplaceRemoveResponse",
        ),
        CodexProviderCommandResponseKind::CommandResult,
        CodexProviderCommandAvailability::Guarded,
    ),
    command(
        "marketplace/upgrade",
        CodexProviderCommandCategory::SkillsPluginsApps,
        "Upgrade marketplace item",
        schema(
            "v2::MarketplaceUpgradeParams",
            "v2::MarketplaceUpgradeResponse",
        ),
        CodexProviderCommandResponseKind::CommandResult,
        CodexProviderCommandAvailability::Guarded,
    ),
    command(
        "plugin/list",
        CodexProviderCommandCategory::SkillsPluginsApps,
        "List plugins",
        schema("v2::PluginListParams", "v2::PluginListResponse"),
        CodexProviderCommandResponseKind::QueryResult,
        CodexProviderCommandAvailability::Supported,
    ),
    command(
        "plugin/read",
        CodexProviderCommandCategory::SkillsPluginsApps,
        "Read plugin",
        schema("v2::PluginReadParams", "v2::PluginReadResponse"),
        CodexProviderCommandResponseKind::QueryResult,
        CodexProviderCommandAvailability::Supported,
    ),
    command(
        "app/list",
        CodexProviderCommandCategory::SkillsPluginsApps,
        "List apps",
        schema("v2::AppsListParams", "v2::AppsListResponse"),
        CodexProviderCommandResponseKind::QueryResult,
        CodexProviderCommandAvailability::Supported,
    ),
    command(
        "skills/config/write",
        CodexProviderCommandCategory::SkillsPluginsApps,
        "Write skills config",
        schema(
            "v2::SkillsConfigWriteParams",
            "v2::SkillsConfigWriteResponse",
        ),
        CodexProviderCommandResponseKind::CommandResult,
        CodexProviderCommandAvailability::Guarded,
    ),
    command(
        "plugin/install",
        CodexProviderCommandCategory::SkillsPluginsApps,
        "Install plugin",
        schema("v2::PluginInstallParams", "v2::PluginInstallResponse"),
        CodexProviderCommandResponseKind::CommandResult,
        CodexProviderCommandAvailability::Guarded,
    ),
    command(
        "plugin/uninstall",
        CodexProviderCommandCategory::SkillsPluginsApps,
        "Uninstall plugin",
        schema("v2::PluginUninstallParams", "v2::PluginUninstallResponse"),
        CodexProviderCommandResponseKind::CommandResult,
        CodexProviderCommandAvailability::Guarded,
    ),
    command(
        "fs/readFile",
        CodexProviderCommandCategory::Filesystem,
        "Read file",
        schema("v2::FsReadFileParams", "v2::FsReadFileResponse"),
        CodexProviderCommandResponseKind::QueryResult,
        CodexProviderCommandAvailability::Guarded,
    ),
    command(
        "fs/writeFile",
        CodexProviderCommandCategory::Filesystem,
        "Write file",
        schema("v2::FsWriteFileParams", "v2::FsWriteFileResponse"),
        CodexProviderCommandResponseKind::CommandResult,
        CodexProviderCommandAvailability::Guarded,
    ),
    command(
        "fs/createDirectory",
        CodexProviderCommandCategory::Filesystem,
        "Create directory",
        schema(
            "v2::FsCreateDirectoryParams",
            "v2::FsCreateDirectoryResponse",
        ),
        CodexProviderCommandResponseKind::CommandResult,
        CodexProviderCommandAvailability::Guarded,
    ),
    command(
        "fs/getMetadata",
        CodexProviderCommandCategory::Filesystem,
        "Get filesystem metadata",
        schema("v2::FsGetMetadataParams", "v2::FsGetMetadataResponse"),
        CodexProviderCommandResponseKind::QueryResult,
        CodexProviderCommandAvailability::Guarded,
    ),
    command(
        "fs/readDirectory",
        CodexProviderCommandCategory::Filesystem,
        "Read directory",
        schema("v2::FsReadDirectoryParams", "v2::FsReadDirectoryResponse"),
        CodexProviderCommandResponseKind::QueryResult,
        CodexProviderCommandAvailability::Guarded,
    ),
    command(
        "fs/remove",
        CodexProviderCommandCategory::Filesystem,
        "Remove filesystem entry",
        schema("v2::FsRemoveParams", "v2::FsRemoveResponse"),
        CodexProviderCommandResponseKind::CommandResult,
        CodexProviderCommandAvailability::Guarded,
    ),
    command(
        "fs/copy",
        CodexProviderCommandCategory::Filesystem,
        "Copy filesystem entry",
        schema("v2::FsCopyParams", "v2::FsCopyResponse"),
        CodexProviderCommandResponseKind::CommandResult,
        CodexProviderCommandAvailability::Guarded,
    ),
    command(
        "fs/watch",
        CodexProviderCommandCategory::Filesystem,
        "Watch filesystem path",
        schema("v2::FsWatchParams", "v2::FsWatchResponse"),
        CodexProviderCommandResponseKind::StreamHandle,
        CodexProviderCommandAvailability::Guarded,
    ),
    command(
        "fs/unwatch",
        CodexProviderCommandCategory::Filesystem,
        "Stop filesystem watch",
        schema("v2::FsUnwatchParams", "v2::FsUnwatchResponse"),
        CodexProviderCommandResponseKind::CommandResult,
        CodexProviderCommandAvailability::Guarded,
    ),
    command(
        "device/key/create",
        CodexProviderCommandCategory::AccountDeviceAuth,
        "Create device key",
        schema("v2::DeviceKeyCreateParams", "v2::DeviceKeyCreateResponse"),
        CodexProviderCommandResponseKind::CommandResult,
        CodexProviderCommandAvailability::Guarded,
    ),
    command(
        "device/key/public",
        CodexProviderCommandCategory::AccountDeviceAuth,
        "Read public device key",
        schema("v2::DeviceKeyPublicParams", "v2::DeviceKeyPublicResponse"),
        CodexProviderCommandResponseKind::QueryResult,
        CodexProviderCommandAvailability::Supported,
    ),
    command(
        "device/key/sign",
        CodexProviderCommandCategory::AccountDeviceAuth,
        "Sign with device key",
        schema("v2::DeviceKeySignParams", "v2::DeviceKeySignResponse"),
        CodexProviderCommandResponseKind::CommandResult,
        CodexProviderCommandAvailability::Guarded,
    ),
    command(
        "account/login/start",
        CodexProviderCommandCategory::AccountDeviceAuth,
        "Start account login",
        schema("v2::LoginAccountParams", "v2::LoginAccountResponse"),
        CodexProviderCommandResponseKind::LongRunning,
        CodexProviderCommandAvailability::Guarded,
    ),
    command(
        "account/login/cancel",
        CodexProviderCommandCategory::AccountDeviceAuth,
        "Cancel account login",
        schema(
            "v2::CancelLoginAccountParams",
            "v2::CancelLoginAccountResponse",
        ),
        CodexProviderCommandResponseKind::CommandResult,
        CodexProviderCommandAvailability::Supported,
    ),
    command(
        "account/logout",
        CodexProviderCommandCategory::AccountDeviceAuth,
        "Log out account",
        schema("undefined", "v2::LogoutAccountResponse"),
        CodexProviderCommandResponseKind::CommandResult,
        CodexProviderCommandAvailability::Guarded,
    ),
    command(
        "account/rateLimits/read",
        CodexProviderCommandCategory::AccountDeviceAuth,
        "Read account rate limits",
        schema("undefined", "v2::GetAccountRateLimitsResponse"),
        CodexProviderCommandResponseKind::QueryResult,
        CodexProviderCommandAvailability::Supported,
    ),
    command(
        "account/sendAddCreditsNudgeEmail",
        CodexProviderCommandCategory::AccountDeviceAuth,
        "Send add-credits nudge email",
        schema(
            "v2::SendAddCreditsNudgeEmailParams",
            "v2::SendAddCreditsNudgeEmailResponse",
        ),
        CodexProviderCommandResponseKind::CommandResult,
        CodexProviderCommandAvailability::Guarded,
    ),
    command(
        "account/read",
        CodexProviderCommandCategory::AccountDeviceAuth,
        "Read account",
        schema("v2::GetAccountParams", "v2::GetAccountResponse"),
        CodexProviderCommandResponseKind::QueryResult,
        CodexProviderCommandAvailability::Supported,
    ),
    command(
        "config/read",
        CodexProviderCommandCategory::ConfigMcp,
        "Read config",
        schema("v2::ConfigReadParams", "v2::ConfigReadResponse"),
        CodexProviderCommandResponseKind::QueryResult,
        CodexProviderCommandAvailability::Supported,
    ),
    command(
        "config/value/write",
        CodexProviderCommandCategory::ConfigMcp,
        "Write config value",
        schema("v2::ConfigValueWriteParams", "v2::ConfigWriteResponse"),
        CodexProviderCommandResponseKind::CommandResult,
        CodexProviderCommandAvailability::Guarded,
    ),
    command(
        "config/batchWrite",
        CodexProviderCommandCategory::ConfigMcp,
        "Write config values",
        schema("v2::ConfigBatchWriteParams", "v2::ConfigWriteResponse"),
        CodexProviderCommandResponseKind::CommandResult,
        CodexProviderCommandAvailability::Guarded,
    ),
    command(
        "configRequirements/read",
        CodexProviderCommandCategory::ConfigMcp,
        "Read config requirements",
        schema("undefined", "v2::ConfigRequirementsReadResponse"),
        CodexProviderCommandResponseKind::QueryResult,
        CodexProviderCommandAvailability::Supported,
    ),
    command(
        "model/list",
        CodexProviderCommandCategory::ConfigMcp,
        "List models",
        schema("v2::ModelListParams", "v2::ModelListResponse"),
        CodexProviderCommandResponseKind::QueryResult,
        CodexProviderCommandAvailability::Supported,
    ),
    command(
        "modelProvider/capabilities/read",
        CodexProviderCommandCategory::ConfigMcp,
        "Read model provider capabilities",
        schema(
            "v2::ModelProviderCapabilitiesReadParams",
            "v2::ModelProviderCapabilitiesReadResponse",
        ),
        CodexProviderCommandResponseKind::QueryResult,
        CodexProviderCommandAvailability::Supported,
    ),
    command(
        "experimentalFeature/list",
        CodexProviderCommandCategory::ConfigMcp,
        "List experimental features",
        schema(
            "v2::ExperimentalFeatureListParams",
            "v2::ExperimentalFeatureListResponse",
        ),
        CodexProviderCommandResponseKind::QueryResult,
        CodexProviderCommandAvailability::Supported,
    ),
    command(
        "experimentalFeature/enablement/set",
        CodexProviderCommandCategory::ConfigMcp,
        "Set experimental feature enablement",
        schema(
            "v2::ExperimentalFeatureEnablementSetParams",
            "v2::ExperimentalFeatureEnablementSetResponse",
        ),
        CodexProviderCommandResponseKind::CommandResult,
        CodexProviderCommandAvailability::Guarded,
    ),
    command(
        "collaborationMode/list",
        CodexProviderCommandCategory::ConfigMcp,
        "List collaboration modes",
        schema(
            "v2::CollaborationModeListParams",
            "v2::CollaborationModeListResponse",
        ),
        CodexProviderCommandResponseKind::QueryResult,
        CodexProviderCommandAvailability::Experimental,
    ),
    command(
        "mcpServer/oauth/login",
        CodexProviderCommandCategory::ConfigMcp,
        "Start MCP OAuth login",
        schema(
            "v2::McpServerOauthLoginParams",
            "v2::McpServerOauthLoginResponse",
        ),
        CodexProviderCommandResponseKind::LongRunning,
        CodexProviderCommandAvailability::Guarded,
    ),
    command(
        "config/mcpServer/reload",
        CodexProviderCommandCategory::ConfigMcp,
        "Reload MCP server config",
        schema("undefined", "v2::McpServerRefreshResponse"),
        CodexProviderCommandResponseKind::CommandResult,
        CodexProviderCommandAvailability::Supported,
    ),
    command(
        "mcpServerStatus/list",
        CodexProviderCommandCategory::ConfigMcp,
        "List MCP server status",
        schema(
            "v2::ListMcpServerStatusParams",
            "v2::ListMcpServerStatusResponse",
        ),
        CodexProviderCommandResponseKind::QueryResult,
        CodexProviderCommandAvailability::Supported,
    ),
    command(
        "mcpServer/resource/read",
        CodexProviderCommandCategory::ConfigMcp,
        "Read MCP resource",
        schema("v2::McpResourceReadParams", "v2::McpResourceReadResponse"),
        CodexProviderCommandResponseKind::QueryResult,
        CodexProviderCommandAvailability::Supported,
    ),
    command(
        "mcpServer/tool/call",
        CodexProviderCommandCategory::ConfigMcp,
        "Call MCP tool",
        schema(
            "v2::McpServerToolCallParams",
            "v2::McpServerToolCallResponse",
        ),
        CodexProviderCommandResponseKind::ProviderNotification,
        CodexProviderCommandAvailability::Guarded,
    ),
    command(
        "thread/realtime/start",
        CodexProviderCommandCategory::Realtime,
        "Start realtime session",
        schema(
            "v2::ThreadRealtimeStartParams",
            "v2::ThreadRealtimeStartResponse",
        ),
        CodexProviderCommandResponseKind::StreamHandle,
        CodexProviderCommandAvailability::Experimental,
    ),
    command(
        "thread/realtime/appendAudio",
        CodexProviderCommandCategory::Realtime,
        "Append realtime audio",
        schema(
            "v2::ThreadRealtimeAppendAudioParams",
            "v2::ThreadRealtimeAppendAudioResponse",
        ),
        CodexProviderCommandResponseKind::StreamingUpdate,
        CodexProviderCommandAvailability::Experimental,
    ),
    command(
        "thread/realtime/appendText",
        CodexProviderCommandCategory::Realtime,
        "Append realtime text",
        schema(
            "v2::ThreadRealtimeAppendTextParams",
            "v2::ThreadRealtimeAppendTextResponse",
        ),
        CodexProviderCommandResponseKind::StreamingUpdate,
        CodexProviderCommandAvailability::Experimental,
    ),
    command(
        "thread/realtime/stop",
        CodexProviderCommandCategory::Realtime,
        "Stop realtime session",
        schema(
            "v2::ThreadRealtimeStopParams",
            "v2::ThreadRealtimeStopResponse",
        ),
        CodexProviderCommandResponseKind::CommandResult,
        CodexProviderCommandAvailability::Experimental,
    ),
    command(
        "thread/realtime/listVoices",
        CodexProviderCommandCategory::Realtime,
        "List realtime voices",
        schema(
            "v2::ThreadRealtimeListVoicesParams",
            "v2::ThreadRealtimeListVoicesResponse",
        ),
        CodexProviderCommandResponseKind::QueryResult,
        CodexProviderCommandAvailability::Experimental,
    ),
    command(
        "feedback/upload",
        CodexProviderCommandCategory::Feedback,
        "Upload feedback",
        schema("v2::FeedbackUploadParams", "v2::FeedbackUploadResponse"),
        CodexProviderCommandResponseKind::CommandResult,
        CodexProviderCommandAvailability::Guarded,
    ),
    command(
        "thread/shellCommand",
        CodexProviderCommandCategory::ShellBackgroundTerminals,
        "Run thread shell command",
        schema(
            "v2::ThreadShellCommandParams",
            "v2::ThreadShellCommandResponse",
        ),
        CodexProviderCommandResponseKind::LongRunning,
        CodexProviderCommandAvailability::Guarded,
    ),
    command(
        "thread/backgroundTerminals/clean",
        CodexProviderCommandCategory::ShellBackgroundTerminals,
        "Clean background terminals",
        schema(
            "v2::ThreadBackgroundTerminalsCleanParams",
            "v2::ThreadBackgroundTerminalsCleanResponse",
        ),
        CodexProviderCommandResponseKind::CommandResult,
        CodexProviderCommandAvailability::Experimental,
    ),
    command(
        "command/exec",
        CodexProviderCommandCategory::ShellBackgroundTerminals,
        "Execute one-off command",
        schema("v2::CommandExecParams", "v2::CommandExecResponse"),
        CodexProviderCommandResponseKind::StreamHandle,
        CodexProviderCommandAvailability::Guarded,
    ),
    command(
        "command/exec/write",
        CodexProviderCommandCategory::ShellBackgroundTerminals,
        "Write one-off command input",
        schema("v2::CommandExecWriteParams", "v2::CommandExecWriteResponse"),
        CodexProviderCommandResponseKind::StreamingUpdate,
        CodexProviderCommandAvailability::Guarded,
    ),
    command(
        "command/exec/terminate",
        CodexProviderCommandCategory::ShellBackgroundTerminals,
        "Terminate one-off command",
        schema(
            "v2::CommandExecTerminateParams",
            "v2::CommandExecTerminateResponse",
        ),
        CodexProviderCommandResponseKind::CommandResult,
        CodexProviderCommandAvailability::Guarded,
    ),
    command(
        "command/exec/resize",
        CodexProviderCommandCategory::ShellBackgroundTerminals,
        "Resize one-off command",
        schema(
            "v2::CommandExecResizeParams",
            "v2::CommandExecResizeResponse",
        ),
        CodexProviderCommandResponseKind::StreamingUpdate,
        CodexProviderCommandAvailability::Guarded,
    ),
    command(
        "externalAgentConfig/detect",
        CodexProviderCommandCategory::ExternalAgentConfig,
        "Detect external agent config",
        schema(
            "v2::ExternalAgentConfigDetectParams",
            "v2::ExternalAgentConfigDetectResponse",
        ),
        CodexProviderCommandResponseKind::QueryResult,
        CodexProviderCommandAvailability::Supported,
    ),
    command(
        "externalAgentConfig/import",
        CodexProviderCommandCategory::ExternalAgentConfig,
        "Import external agent config",
        schema(
            "v2::ExternalAgentConfigImportParams",
            "v2::ExternalAgentConfigImportResponse",
        ),
        CodexProviderCommandResponseKind::LongRunning,
        CodexProviderCommandAvailability::Guarded,
    ),
    command(
        "fuzzyFileSearch",
        CodexProviderCommandCategory::FuzzySearch,
        "Fuzzy file search",
        schema("FuzzyFileSearchParams", "FuzzyFileSearchResponse"),
        CodexProviderCommandResponseKind::QueryResult,
        CodexProviderCommandAvailability::Guarded,
    ),
    command(
        "fuzzyFileSearch/sessionStart",
        CodexProviderCommandCategory::FuzzySearch,
        "Start fuzzy file search session",
        schema(
            "FuzzyFileSearchSessionStartParams",
            "FuzzyFileSearchSessionStartResponse",
        ),
        CodexProviderCommandResponseKind::StreamHandle,
        CodexProviderCommandAvailability::Experimental,
    ),
    command(
        "fuzzyFileSearch/sessionUpdate",
        CodexProviderCommandCategory::FuzzySearch,
        "Update fuzzy file search session",
        schema(
            "FuzzyFileSearchSessionUpdateParams",
            "FuzzyFileSearchSessionUpdateResponse",
        ),
        CodexProviderCommandResponseKind::StreamingUpdate,
        CodexProviderCommandAvailability::Experimental,
    ),
    command(
        "fuzzyFileSearch/sessionStop",
        CodexProviderCommandCategory::FuzzySearch,
        "Stop fuzzy file search session",
        schema(
            "FuzzyFileSearchSessionStopParams",
            "FuzzyFileSearchSessionStopResponse",
        ),
        CodexProviderCommandResponseKind::CommandResult,
        CodexProviderCommandAvailability::Experimental,
    ),
    command(
        "windowsSandbox/setupStart",
        CodexProviderCommandCategory::WindowsSandbox,
        "Start Windows sandbox setup",
        schema(
            "v2::WindowsSandboxSetupStartParams",
            "v2::WindowsSandboxSetupStartResponse",
        ),
        CodexProviderCommandResponseKind::LongRunning,
        CodexProviderCommandAvailability::PlatformGated,
    ),
    command(
        "mock/experimentalMethod",
        CodexProviderCommandCategory::MockExperimental,
        "Mock experimental method",
        schema(
            "v2::MockExperimentalMethodParams",
            "v2::MockExperimentalMethodResponse",
        ),
        CodexProviderCommandResponseKind::UnsupportedCompatibility,
        CodexProviderCommandAvailability::Disabled,
    ),
    command(
        "review/start",
        CodexProviderCommandCategory::ThreadMaintenance,
        "Start review",
        schema("v2::ReviewStartParams", "v2::ReviewStartResponse"),
        CodexProviderCommandResponseKind::LongRunning,
        CodexProviderCommandAvailability::Supported,
    ),
];

#[must_use]
pub fn provider_command_manifest() -> &'static [CodexProviderCommand] {
    PROVIDER_COMMANDS
}

#[must_use]
pub fn client_request_dispositions() -> Vec<CodexClientRequestDisposition> {
    CORE_CLIENT_REQUESTS
        .iter()
        .chain(ADAPTER_INTERNAL_CLIENT_REQUESTS)
        .chain(DEFERRED_CLIENT_REQUESTS)
        .copied()
        .chain(PROVIDER_COMMANDS.iter().map(|command| {
            disposition(
                command.method,
                CodexClientRequestDispositionKind::ProviderCommand,
                "Exposed through the Codex provider command manifest.",
            )
        }))
        .collect()
}

#[must_use]
pub fn provider_command(method: &str) -> Option<&'static CodexProviderCommand> {
    PROVIDER_COMMANDS
        .iter()
        .find(|command| command.method == method)
}

#[must_use]
pub fn provider_capability_details() -> Vec<agenter_core::ProviderCapabilityDetail> {
    use agenter_core::{ProviderCapabilityDetail, ProviderCapabilityStatus};

    let families = [
        (
            "thread_maintenance",
            CodexProviderCommandCategory::ThreadMaintenance,
        ),
        ("memory_mode", CodexProviderCommandCategory::Memory),
        (
            "skills_plugins_apps",
            CodexProviderCommandCategory::SkillsPluginsApps,
        ),
        ("client_fs", CodexProviderCommandCategory::Filesystem),
        (
            "config_account_device_auth",
            CodexProviderCommandCategory::AccountDeviceAuth,
        ),
        ("config_mcp", CodexProviderCommandCategory::ConfigMcp),
        ("realtime", CodexProviderCommandCategory::Realtime),
        ("feedback", CodexProviderCommandCategory::Feedback),
        (
            "one_off_command",
            CodexProviderCommandCategory::ShellBackgroundTerminals,
        ),
        (
            "external_agent_config",
            CodexProviderCommandCategory::ExternalAgentConfig,
        ),
        ("fuzzy_search", CodexProviderCommandCategory::FuzzySearch),
        (
            "windows_sandbox",
            CodexProviderCommandCategory::WindowsSandbox,
        ),
    ];

    families
        .into_iter()
        .map(|(key, category)| {
            let commands = PROVIDER_COMMANDS
                .iter()
                .filter(|command| command.category == category)
                .collect::<Vec<_>>();
            let methods = commands
                .iter()
                .map(|command| command.method.to_owned())
                .collect::<Vec<_>>();
            let status = if commands.iter().all(|command| {
                matches!(
                    command.availability,
                    CodexProviderCommandAvailability::Supported
                        | CodexProviderCommandAvailability::Guarded
                )
            }) {
                ProviderCapabilityStatus::Supported
            } else {
                ProviderCapabilityStatus::Degraded
            };

            ProviderCapabilityDetail {
                key: key.to_owned(),
                status,
                methods,
                reason: None,
            }
        })
        .collect()
}

fn method_set<'a>(methods: impl IntoIterator<Item = &'a str>) -> BTreeSet<&'a str> {
    methods.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use super::*;

    #[test]
    fn codex_provider_commands_keep_required_native_metadata() {
        assert!(
            !PROVIDER_COMMANDS.is_empty(),
            "manifest should expose provider command entries"
        );

        for command in PROVIDER_COMMANDS {
            assert!(
                !command.method.trim().is_empty(),
                "command method must be retained"
            );
            assert!(
                !command.label.trim().is_empty(),
                "{} should have a human-readable label",
                command.method
            );
            assert!(
                !command.schema.params_type.trim().is_empty(),
                "{} should retain a params schema type or placeholder",
                command.method
            );
            assert!(
                !command.schema.response_type.trim().is_empty(),
                "{} should retain a response schema type",
                command.method
            );
            assert_eq!(
                command.raw_payload_display,
                RawPayloadDisplay::Always,
                "{} provider command results must expose raw native payloads during the Codex research phase",
                command.method
            );
        }
    }

    #[test]
    fn codex_provider_commands_classify_every_current_client_request() {
        let actual = current_codex_client_request_methods();
        let dispositions = client_request_dispositions();
        let disposition_methods = dispositions
            .iter()
            .map(|disposition| disposition.method)
            .collect::<Vec<_>>();

        assert_no_duplicates("client request dispositions", &disposition_methods);

        let covered = method_set(disposition_methods);
        let missing = actual.difference(&covered).copied().collect::<Vec<_>>();
        let stale = covered.difference(&actual).copied().collect::<Vec<_>>();

        assert!(
            missing.is_empty() && stale.is_empty(),
            "Codex ClientRequest disposition drifted.\nmissing: {}\nstale: {}",
            missing.join(", "),
            stale.join(", ")
        );
    }

    #[test]
    fn codex_provider_commands_classify_non_core_requests() {
        for disposition in client_request_dispositions() {
            match disposition.disposition {
                CodexClientRequestDispositionKind::Core => {
                    assert!(
                        CORE_CLIENT_REQUESTS
                            .iter()
                            .any(|core| core.method == disposition.method),
                        "{} is marked core without being in the core table",
                        disposition.method
                    );
                }
                CodexClientRequestDispositionKind::ProviderCommand => {
                    assert!(
                        provider_command(disposition.method).is_some(),
                        "{} is marked as a provider command without a manifest entry",
                        disposition.method
                    );
                }
                CodexClientRequestDispositionKind::AdapterInternal
                | CodexClientRequestDispositionKind::Deferred => {
                    assert!(
                        !disposition.reason.trim().is_empty(),
                        "{} must document why it is not a manifest command",
                        disposition.method
                    );
                }
            }
        }
    }

    #[test]
    fn codex_provider_commands_cover_stage7_feature_families() {
        for category in [
            CodexProviderCommandCategory::Memory,
            CodexProviderCommandCategory::SkillsPluginsApps,
            CodexProviderCommandCategory::Filesystem,
            CodexProviderCommandCategory::AccountDeviceAuth,
            CodexProviderCommandCategory::ConfigMcp,
            CodexProviderCommandCategory::Realtime,
            CodexProviderCommandCategory::Feedback,
            CodexProviderCommandCategory::ShellBackgroundTerminals,
            CodexProviderCommandCategory::ExternalAgentConfig,
            CodexProviderCommandCategory::FuzzySearch,
            CodexProviderCommandCategory::WindowsSandbox,
            CodexProviderCommandCategory::MockExperimental,
        ] {
            assert!(
                PROVIDER_COMMANDS
                    .iter()
                    .any(|command| command.category == category),
                "missing provider command family {}",
                category.as_str()
            );
        }
    }

    #[test]
    fn codex_provider_commands_keep_guarded_and_disabled_policies() {
        for method in [
            "fs/writeFile",
            "account/logout",
            "config/value/write",
            "command/exec",
            "externalAgentConfig/import",
            "mock/experimentalMethod",
        ] {
            let command = provider_command(method).expect("command should exist");
            assert!(
                !matches!(
                    command.availability,
                    CodexProviderCommandAvailability::Supported
                ),
                "{method} should remain guarded, disabled, experimental, or gated"
            );
        }
    }

    #[test]
    fn codex_provider_commands_capabilities_reference_manifest_methods() {
        let manifest_methods = method_set(PROVIDER_COMMANDS.iter().map(|command| command.method));

        for detail in provider_capability_details() {
            assert!(
                !detail.methods.is_empty(),
                "{} capability should reference at least one Codex method",
                detail.key
            );
            for method in detail.methods {
                assert!(
                    manifest_methods.contains(method.as_str()),
                    "{} capability references unknown method {}",
                    detail.key,
                    method
                );
            }
        }
    }

    fn current_codex_client_request_methods() -> BTreeSet<&'static str> {
        let repo = repo_root();
        let path = repo.join("tmp/codex/codex-rs/app-server-protocol/src/protocol/common.rs");
        let source = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));

        extract_macro_methods(
            Box::leak(source.into_boxed_str()),
            "client_request_definitions",
        )
    }

    fn repo_root() -> &'static Path {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .expect("runner crate should be two levels below repo root")
    }

    fn assert_no_duplicates(label: &str, values: &[&str]) {
        let mut seen = BTreeSet::new();
        let duplicates = values
            .iter()
            .filter_map(|value| {
                if seen.insert(*value) {
                    None
                } else {
                    Some(*value)
                }
            })
            .collect::<Vec<_>>();
        assert!(
            duplicates.is_empty(),
            "{label} has duplicate values: {}",
            duplicates.join(", ")
        );
    }

    fn extract_macro_methods(source: &'static str, macro_name: &str) -> BTreeSet<&'static str> {
        split_top_level_entries(strip_line_comments(extract_macro_body(source, macro_name)))
            .into_iter()
            .filter(|entry| !entry.trim().is_empty())
            .map(method_name_for_entry)
            .collect()
    }

    fn extract_macro_body(source: &'static str, macro_name: &str) -> &'static str {
        let marker = format!("{macro_name}! {{");
        let start = source
            .find(&marker)
            .unwrap_or_else(|| panic!("missing macro invocation {macro_name}!"));
        let body_start = start + marker.len();
        let body_end = matching_delimiter(source, body_start - 1, '{', '}')
            .unwrap_or_else(|| panic!("unterminated macro invocation {macro_name}!"));

        &source[body_start..body_end]
    }

    fn method_name_for_entry(entry: &'static str) -> &'static str {
        explicit_wire_name(entry)
            .or_else(|| serde_rename(entry))
            .unwrap_or_else(|| lower_camel_case(first_identifier(entry)))
    }

    fn explicit_wire_name(entry: &'static str) -> Option<&'static str> {
        let arrow = entry.find("=>")?;
        quoted_string(&entry[arrow + 2..])
    }

    fn serde_rename(entry: &'static str) -> Option<&'static str> {
        let marker = "serde(rename =";
        let start = entry.find(marker)?;
        quoted_string(&entry[start + marker.len()..])
    }

    fn quoted_string(input: &'static str) -> Option<&'static str> {
        let start = input.find('"')? + 1;
        let end = input[start..].find('"')? + start;
        Some(&input[start..end])
    }

    fn first_identifier(entry: &'static str) -> &'static str {
        for line in entry.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty()
                || trimmed.starts_with('#')
                || trimmed.starts_with("///")
                || trimmed.starts_with("//")
            {
                continue;
            }

            let end = trimmed
                .char_indices()
                .find_map(|(index, ch)| (!ch.is_ascii_alphanumeric() && ch != '_').then_some(index))
                .unwrap_or(trimmed.len());
            if end > 0 {
                return &trimmed[..end];
            }
        }

        panic!("entry has no variant identifier: {entry}");
    }

    fn lower_camel_case(ident: &'static str) -> &'static str {
        match ident {
            "Initialize" => "initialize",
            "GetConversationSummary" => "getConversationSummary",
            "GitDiffToRemote" => "gitDiffToRemote",
            "GetAuthStatus" => "getAuthStatus",
            "FuzzyFileSearch" => "fuzzyFileSearch",
            other => panic!("unexpected implicit ClientRequest variant without wire name: {other}"),
        }
    }

    fn split_top_level_entries(body: String) -> Vec<&'static str> {
        let body = Box::leak(body.into_boxed_str());
        let mut entries = Vec::new();
        let mut start = 0;
        let mut depth = Vec::new();
        let mut in_string = false;
        let mut escaped = false;

        for (index, ch) in body.char_indices() {
            if in_string {
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == '"' {
                    in_string = false;
                }
                continue;
            }

            match ch {
                '"' => in_string = true,
                '{' | '(' | '[' => depth.push(ch),
                '}' => pop_delimiter(&mut depth, '{', ch),
                ')' => pop_delimiter(&mut depth, '(', ch),
                ']' => pop_delimiter(&mut depth, '[', ch),
                ',' if depth.is_empty() => {
                    entries.push(&body[start..index]);
                    start = index + ch.len_utf8();
                }
                _ => {}
            }
        }

        if start < body.len() {
            entries.push(&body[start..]);
        }

        entries
    }

    fn strip_line_comments(source: &'static str) -> String {
        let mut stripped = String::with_capacity(source.len());

        for line in source.lines() {
            let mut in_string = false;
            let mut escaped = false;
            let mut comment_start = line.len();
            let mut previous = '\0';

            for (index, ch) in line.char_indices() {
                if in_string {
                    if escaped {
                        escaped = false;
                    } else if ch == '\\' {
                        escaped = true;
                    } else if ch == '"' {
                        in_string = false;
                    }
                } else if ch == '"' {
                    in_string = true;
                } else if previous == '/' && ch == '/' {
                    comment_start = index - previous.len_utf8();
                    break;
                }
                previous = ch;
            }

            stripped.push_str(&line[..comment_start]);
            stripped.push('\n');
        }

        stripped
    }

    fn matching_delimiter(
        source: &str,
        open_index: usize,
        open: char,
        close: char,
    ) -> Option<usize> {
        let mut depth = 0;
        let mut in_string = false;
        let mut escaped = false;

        for (offset, ch) in source[open_index..].char_indices() {
            if in_string {
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == '"' {
                    in_string = false;
                }
                continue;
            }

            if ch == '"' {
                in_string = true;
            } else if ch == open {
                depth += 1;
            } else if ch == close {
                depth -= 1;
                if depth == 0 {
                    return Some(open_index + offset);
                }
            }
        }

        None
    }

    fn pop_delimiter(depth: &mut Vec<char>, expected: char, actual: char) {
        let popped = depth
            .pop()
            .unwrap_or_else(|| panic!("unmatched delimiter {actual}"));
        assert_eq!(
            popped, expected,
            "mismatched delimiter: expected {expected}, got {actual}"
        );
    }
}
