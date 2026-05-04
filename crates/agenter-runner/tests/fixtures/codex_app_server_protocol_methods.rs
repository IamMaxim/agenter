// Extracted from tmp/codex/codex-rs/app-server-protocol/src/protocol/common.rs
// at local Codex app-server protocol/TUI snapshot 637f7dd6d7.
//
// Refresh by checking out the target Codex snapshot under tmp/codex, then copy
// the client_request_definitions!, server_request_definitions!, and
// server_notification_definitions! invocation method lines from common.rs.

client_request_definitions! {
    Initialize {}
    ThreadStart => "thread/start" {}
    ThreadResume => "thread/resume" {}
    ThreadFork => "thread/fork" {}
    ThreadArchive => "thread/archive" {}
    ThreadUnsubscribe => "thread/unsubscribe" {}
    ThreadIncrementElicitation => "thread/increment_elicitation" {}
    ThreadDecrementElicitation => "thread/decrement_elicitation" {}
    ThreadSetName => "thread/name/set" {}
    ThreadMetadataUpdate => "thread/metadata/update" {}
    ThreadMemoryModeSet => "thread/memoryMode/set" {}
    MemoryReset => "memory/reset" {}
    ThreadUnarchive => "thread/unarchive" {}
    ThreadCompactStart => "thread/compact/start" {}
    ThreadShellCommand => "thread/shellCommand" {}
    ThreadApproveGuardianDeniedAction => "thread/approveGuardianDeniedAction" {}
    ThreadBackgroundTerminalsClean => "thread/backgroundTerminals/clean" {}
    ThreadRollback => "thread/rollback" {}
    ThreadList => "thread/list" {}
    ThreadLoadedList => "thread/loaded/list" {}
    ThreadRead => "thread/read" {}
    ThreadContextWindowInspect => "thread/contextWindow/inspect" {}
    ThreadTurnsList => "thread/turns/list" {}
    ThreadInjectItems => "thread/inject_items" {}
    SkillsList => "skills/list" {}
    MarketplaceAdd => "marketplace/add" {}
    MarketplaceRemove => "marketplace/remove" {}
    MarketplaceUpgrade => "marketplace/upgrade" {}
    PluginList => "plugin/list" {}
    PluginRead => "plugin/read" {}
    AppsList => "app/list" {}
    DeviceKeyCreate => "device/key/create" {}
    DeviceKeyPublic => "device/key/public" {}
    DeviceKeySign => "device/key/sign" {}
    FsReadFile => "fs/readFile" {}
    FsWriteFile => "fs/writeFile" {}
    FsCreateDirectory => "fs/createDirectory" {}
    FsGetMetadata => "fs/getMetadata" {}
    FsReadDirectory => "fs/readDirectory" {}
    FsRemove => "fs/remove" {}
    FsCopy => "fs/copy" {}
    FsWatch => "fs/watch" {}
    FsUnwatch => "fs/unwatch" {}
    SkillsConfigWrite => "skills/config/write" {}
    PluginInstall => "plugin/install" {}
    PluginUninstall => "plugin/uninstall" {}
    TurnStart => "turn/start" {}
    TurnSteer => "turn/steer" {}
    TurnInterrupt => "turn/interrupt" {}
    ThreadRealtimeStart => "thread/realtime/start" {}
    ThreadRealtimeAppendAudio => "thread/realtime/appendAudio" {}
    ThreadRealtimeAppendText => "thread/realtime/appendText" {}
    ThreadRealtimeStop => "thread/realtime/stop" {}
    ThreadRealtimeListVoices => "thread/realtime/listVoices" {}
    ReviewStart => "review/start" {}
    ModelList => "model/list" {}
    ExperimentalFeatureList => "experimentalFeature/list" {}
    ExperimentalFeatureEnablementSet => "experimentalFeature/enablement/set" {}
    CollaborationModeList => "collaborationMode/list" {}
    MockExperimentalMethod => "mock/experimentalMethod" {}
    McpServerOauthLogin => "mcpServer/oauth/login" {}
    McpServerRefresh => "config/mcpServer/reload" {}
    McpServerStatusList => "mcpServerStatus/list" {}
    McpResourceRead => "mcpServer/resource/read" {}
    McpServerToolCall => "mcpServer/tool/call" {}
    WindowsSandboxSetupStart => "windowsSandbox/setupStart" {}
    LoginAccount => "account/login/start" {}
    CancelLoginAccount => "account/login/cancel" {}
    LogoutAccount => "account/logout" {}
    GetAccountRateLimits => "account/rateLimits/read" {}
    SendAddCreditsNudgeEmail => "account/sendAddCreditsNudgeEmail" {}
    FeedbackUpload => "feedback/upload" {}
    OneOffCommandExec => "command/exec" {}
    CommandExecWrite => "command/exec/write" {}
    CommandExecTerminate => "command/exec/terminate" {}
    CommandExecResize => "command/exec/resize" {}
    ConfigRead => "config/read" {}
    ExternalAgentConfigDetect => "externalAgentConfig/detect" {}
    ExternalAgentConfigImport => "externalAgentConfig/import" {}
    ConfigValueWrite => "config/value/write" {}
    ConfigBatchWrite => "config/batchWrite" {}
    ConfigRequirementsRead => "configRequirements/read" {}
    GetAccount => "account/read" {}
    GetConversationSummary {}
    GitDiffToRemote {}
    GetAuthStatus {}
    FuzzyFileSearch {}
    FuzzyFileSearchSessionStart => "fuzzyFileSearch/sessionStart" {}
    FuzzyFileSearchSessionUpdate => "fuzzyFileSearch/sessionUpdate" {}
    FuzzyFileSearchSessionStop => "fuzzyFileSearch/sessionStop" {}
}

server_request_definitions! {
    CommandExecutionRequestApproval => "item/commandExecution/requestApproval" {}
    FileChangeRequestApproval => "item/fileChange/requestApproval" {}
    ToolRequestUserInput => "item/tool/requestUserInput" {}
    McpServerElicitationRequest => "mcpServer/elicitation/request" {}
    PermissionsRequestApproval => "item/permissions/requestApproval" {}
    DynamicToolCall => "item/tool/call" {}
    ChatgptAuthTokensRefresh => "account/chatgptAuthTokens/refresh" {}
    ApplyPatchApproval {}
    ExecCommandApproval {}
}

server_notification_definitions! {
    Error => "error" {}
    ThreadStarted => "thread/started" {}
    ThreadStatusChanged => "thread/status/changed" {}
    ThreadArchived => "thread/archived" {}
    ThreadUnarchived => "thread/unarchived" {}
    ThreadClosed => "thread/closed" {}
    SkillsChanged => "skills/changed" {}
    ThreadNameUpdated => "thread/name/updated" {}
    ThreadTokenUsageUpdated => "thread/tokenUsage/updated" {}
    ThreadContextWindowUpdated => "thread/contextWindow/updated" {}
    TurnStarted => "turn/started" {}
    HookStarted => "hook/started" {}
    TurnCompleted => "turn/completed" {}
    HookCompleted => "hook/completed" {}
    TurnDiffUpdated => "turn/diff/updated" {}
    TurnPlanUpdated => "turn/plan/updated" {}
    ItemStarted => "item/started" {}
    ItemGuardianApprovalReviewStarted => "item/autoApprovalReview/started" {}
    ItemGuardianApprovalReviewCompleted => "item/autoApprovalReview/completed" {}
    ItemCompleted => "item/completed" {}
    RawResponseItemCompleted => "rawResponseItem/completed" {}
    AgentMessageDelta => "item/agentMessage/delta" {}
    PlanDelta => "item/plan/delta" {}
    CommandExecOutputDelta => "command/exec/outputDelta" {}
    CommandExecutionOutputDelta => "item/commandExecution/outputDelta" {}
    TerminalInteraction => "item/commandExecution/terminalInteraction" {}
    FileChangeOutputDelta => "item/fileChange/outputDelta" {}
    FileChangePatchUpdated => "item/fileChange/patchUpdated" {}
    ServerRequestResolved => "serverRequest/resolved" {}
    McpToolCallProgress => "item/mcpToolCall/progress" {}
    McpServerOauthLoginCompleted => "mcpServer/oauthLogin/completed" {}
    McpServerStatusUpdated => "mcpServer/startupStatus/updated" {}
    AccountUpdated => "account/updated" {}
    AccountRateLimitsUpdated => "account/rateLimits/updated" {}
    AppListUpdated => "app/list/updated" {}
    ExternalAgentConfigImportCompleted => "externalAgentConfig/import/completed" {}
    FsChanged => "fs/changed" {}
    ReasoningSummaryTextDelta => "item/reasoning/summaryTextDelta" {}
    ReasoningSummaryPartAdded => "item/reasoning/summaryPartAdded" {}
    ReasoningTextDelta => "item/reasoning/textDelta" {}
    ContextCompacted => "thread/compacted" {}
    ModelRerouted => "model/rerouted" {}
    ModelVerification => "model/verification" {}
    Warning => "warning" {}
    GuardianWarning => "guardianWarning" {}
    DeprecationNotice => "deprecationNotice" {}
    ConfigWarning => "configWarning" {}
    FuzzyFileSearchSessionUpdated => "fuzzyFileSearch/sessionUpdated" {}
    FuzzyFileSearchSessionCompleted => "fuzzyFileSearch/sessionCompleted" {}
    ThreadRealtimeStarted => "thread/realtime/started" {}
    ThreadRealtimeItemAdded => "thread/realtime/itemAdded" {}
    ThreadRealtimeTranscriptDelta => "thread/realtime/transcript/delta" {}
    ThreadRealtimeTranscriptDone => "thread/realtime/transcript/done" {}
    ThreadRealtimeOutputAudioDelta => "thread/realtime/outputAudio/delta" {}
    ThreadRealtimeSdp => "thread/realtime/sdp" {}
    ThreadRealtimeError => "thread/realtime/error" {}
    ThreadRealtimeClosed => "thread/realtime/closed" {}
    WindowsWorldWritableWarning => "windows/worldWritableWarning" {}
    WindowsSandboxSetupCompleted => "windowsSandbox/setupCompleted" {}
    AccountLoginCompleted => "account/login/completed" {}
}
