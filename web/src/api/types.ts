export interface AuthenticatedUser {
  user_id: string;
  email: string;
  display_name?: string | null;
}

export interface WorkspaceRef {
  workspace_id: string;
  runner_id: string;
  path: string;
  display_name?: string | null;
}

export type RunnerStatus = 'online' | 'offline' | 'connected' | string;

export interface RunnerInfo {
  runner_id: string;
  name: string;
  status?: RunnerStatus;
  last_seen_at?: string | null;
  provider_ids?: AgentProviderId[];
}

export type AgentProviderId = string;

export type SessionStatus =
  | 'starting'
  | 'running'
  | 'waiting_for_input'
  | 'waiting_for_approval'
  | 'idle'
  | 'stopped'
  | 'completed'
  | 'interrupted'
  | 'degraded'
  | 'failed'
  | 'archived';

export interface SessionInfo {
  session_id: string;
  owner_user_id: string;
  runner_id: string;
  workspace_id: string;
  provider_id: AgentProviderId;
  status: SessionStatus;
  external_session_id?: string | null;
  title?: string | null;
  created_at?: string | null;
  updated_at?: string | null;
  usage?: SessionUsageSnapshot | null;
}

export type AgentReasoningEffort = 'none' | 'minimal' | 'low' | 'medium' | 'high' | 'xhigh';

export interface SessionUsageContext {
  used_percent?: number | null;
  used_tokens?: number | null;
  total_tokens?: number | null;
}

export interface SessionUsageWindow {
  used_percent?: number | null;
  remaining_percent?: number | null;
  resets_at?: string | null;
  window_label?: string | null;
  remaining_text_hint?: string | null;
}

export interface SessionUsageSnapshot {
  mode_label?: string | null;
  model?: string | null;
  reasoning_effort?: AgentReasoningEffort | null;
  context?: SessionUsageContext | null;
  window_5h?: SessionUsageWindow | null;
  week?: SessionUsageWindow | null;
}

export interface AgentModelOption {
  id: string;
  display_name: string;
  description?: string | null;
  is_default: boolean;
  default_reasoning_effort?: AgentReasoningEffort | null;
  supported_reasoning_efforts: AgentReasoningEffort[];
  input_modalities: string[];
}

export interface AgentCollaborationMode {
  id: string;
  label: string;
  model?: string | null;
  reasoning_effort?: AgentReasoningEffort | null;
}

export interface AgentOptions {
  models: AgentModelOption[];
  collaboration_modes: AgentCollaborationMode[];
}

export interface AgentTurnSettings {
  model?: string | null;
  reasoning_effort?: AgentReasoningEffort | null;
  collaboration_mode?: string | null;
}

export type SlashCommandTarget = 'local' | 'runner' | 'provider';
export type SlashCommandDangerLevel = 'safe' | 'confirm' | 'dangerous';
export type SlashCommandArgumentKind = 'string' | 'number' | 'enum' | 'rest';

export interface SlashCommandArgument {
  name: string;
  kind: SlashCommandArgumentKind;
  required: boolean;
  description?: string | null;
  choices: string[];
}

export interface SlashCommandDefinition {
  id: string;
  name: string;
  aliases: string[];
  description: string;
  category: string;
  provider_id?: AgentProviderId | null;
  target: SlashCommandTarget;
  danger_level: SlashCommandDangerLevel;
  arguments: SlashCommandArgument[];
  examples: string[];
}

export interface SlashCommandRequest {
  command_id: string;
  arguments: Record<string, unknown>;
  raw_input: string;
  confirmed: boolean;
}

export interface SlashCommandResult {
  accepted: boolean;
  message: string;
  session?: SessionInfo | null;
  provider_payload?: unknown;
}

export interface AgentQuestionChoice {
  value: string;
  label: string;
  description?: string | null;
}

export interface AgentQuestionField {
  id: string;
  label: string;
  prompt?: string | null;
  kind: string;
  required: boolean;
  secret: boolean;
  choices: AgentQuestionChoice[];
  default_answers: string[];
  schema?: unknown;
}

export interface AgentQuestionAnswer {
  question_id: string;
  answers: Record<string, string[]>;
}

export type QuestionStatus =
  | 'pending'
  | 'answered'
  | 'cancelled'
  | 'expired'
  | 'orphaned'
  | 'detached'
  | string;

export interface QuestionState {
  question_id: string;
  session_id: string;
  turn_id?: string | null;
  title: string;
  description?: string | null;
  fields: AgentQuestionField[];
  status: QuestionStatus;
  answer?: AgentQuestionAnswer | null;
  native_request_id?: string | null;
  native_blocking?: boolean;
  native?: NativeRef | null;
  requested_at?: string | null;
  answered_at?: string | null;
}

export type UniversalSeq = string;
export type UniversalEventSource = 'control_plane' | 'runner' | 'browser' | 'connector' | 'native' | string;
export const UAP_PROTOCOL_VERSION = 'uap/2' as const;
export type UapProtocolVersion = typeof UAP_PROTOCOL_VERSION;

export interface NativeRef {
  protocol: string;
  method?: string | null;
  type?: string | null;
  native_id?: string | null;
  summary?: string | null;
  hash?: string | null;
  pointer?: string | null;
  raw_payload?: unknown;
}

export interface CapabilitySet {
  protocol: {
    streaming?: boolean;
    session_resume?: boolean;
    session_history?: boolean;
    interrupt?: boolean;
    snapshots?: boolean;
    after_seq_replay?: boolean;
  };
  content: {
    text?: boolean;
    images?: boolean;
    file_changes?: boolean;
    diffs?: boolean;
  };
  tools: {
    command_execution?: boolean;
    tool_user_input?: boolean;
  };
  approvals: {
    enabled?: boolean;
    per_session_allow?: boolean;
    deny_with_feedback?: boolean;
    cancel_turn?: boolean;
  };
  plan: {
    updates?: boolean;
    approval?: boolean;
  };
  modes: {
    model_selection?: boolean;
    reasoning_effort?: boolean;
    collaboration_modes?: boolean;
  };
  integration: {
    mcp_elicitation?: boolean;
  };
  provider_details?: ProviderCapabilityDetail[];
}

export type ProviderCapabilityStatus =
  | 'supported'
  | 'degraded'
  | 'unsupported'
  | 'not_applicable'
  | string;

export interface ProviderCapabilityDetail {
  key: string;
  status: ProviderCapabilityStatus;
  methods: string[];
  reason?: string | null;
}

export type TurnStatus =
  | 'starting'
  | 'running'
  | 'waiting_for_input'
  | 'waiting_for_approval'
  | 'interrupting'
  | 'completed'
  | 'failed'
  | 'cancelled'
  | 'interrupted'
  | 'detached'
  | string;

export interface TurnState {
  turn_id: string;
  session_id: string;
  status: TurnStatus;
  started_at?: string | null;
  completed_at?: string | null;
  model?: string | null;
  mode?: string | null;
}

export type ItemRole = 'user' | 'assistant' | 'tool' | 'system' | string;
export type ItemStatus = 'created' | 'streaming' | 'completed' | 'failed' | 'cancelled' | string;
export type ContentBlockKind =
  | 'text'
  | 'reasoning'
  | 'tool_call'
  | 'tool_result'
  | 'command_output'
  | 'terminal_input'
  | 'file_diff'
  | 'image'
  | 'native'
  | 'warning'
  | 'provider_status'
  | string;

export interface ContentBlock {
  block_id: string;
  kind: ContentBlockKind;
  text?: string | null;
  mime_type?: string | null;
  artifact_id?: string | null;
}

export type ToolProjectionKind = 'command' | 'subagent' | 'mcp' | 'tool' | string;

export interface ToolProjection {
  kind: ToolProjectionKind;
  subkind?: string | null;
  name: string;
  title: string;
  status: ItemStatus;
  detail?: string | null;
  input_summary?: string | null;
  output_summary?: string | null;
  command?: ToolCommandProjection | null;
  subagent?: ToolSubagentProjection | null;
  mcp?: ToolMcpProjection | null;
}

export interface ToolCommandProjection {
  command: string;
  cwd?: string | null;
  source?: string | null;
  process_id?: string | null;
  actions: ToolActionProjection[];
  exit_code?: number | null;
  duration_ms?: number | null;
  success?: boolean | null;
}

export interface ToolActionProjection {
  kind: string;
  label: string;
  detail?: string | null;
  path?: string | null;
}

export type ToolSubagentOperation = 'spawn' | 'wait' | 'close' | string;

export interface ToolSubagentProjection {
  operation: ToolSubagentOperation;
  agent_ids: string[];
  model?: string | null;
  reasoning_effort?: string | null;
  prompt?: string | null;
  states: ToolSubagentStateProjection[];
}

export interface ToolSubagentStateProjection {
  agent_id: string;
  status: string;
  message?: string | null;
}

export interface ToolMcpProjection {
  server?: string | null;
  tool: string;
  arguments_summary?: string | null;
  result_summary?: string | null;
}

export interface ItemState {
  item_id: string;
  session_id: string;
  turn_id?: string | null;
  role: ItemRole;
  status: ItemStatus;
  content: ContentBlock[];
  tool?: ToolProjection | null;
  native?: NativeRef | null;
}

export type ApprovalStatus =
  | 'pending'
  | 'presented'
  | 'resolving'
  | 'approved'
  | 'denied'
  | 'cancelled'
  | 'expired'
  | 'orphaned'
  | 'detached'
  | string;
export type ApprovalKind = 'command' | 'file_change' | 'tool' | 'provider_specific' | string;
export type ApprovalOptionKind =
  | 'approve_once'
  | 'approve_always'
  | 'persist_approval_rule'
  | 'deny'
  | 'deny_with_feedback'
  | 'cancel_turn'
  | 'provider_specific'
  | string;

export interface ApprovalOption {
  option_id: string;
  kind: ApprovalOptionKind;
  label: string;
  description?: string | null;
  scope?: string | null;
  native_option_id?: string | null;
  policy_rule?: ApprovalPolicyRulePreview | null;
}

export interface ApprovalPolicyRulePreview {
  kind: ApprovalKind;
  matcher: Record<string, unknown>;
  decision: ApprovalDecision;
  label: string;
}

export interface ApprovalRequest {
  approval_id: string;
  session_id: string;
  turn_id?: string | null;
  item_id?: string | null;
  kind: ApprovalKind;
  title: string;
  details?: string | null;
  options: ApprovalOption[];
  status: ApprovalStatus;
  risk?: string | null;
  subject?: string | null;
  native_request_id?: string | null;
  native_blocking?: boolean;
  policy?: Record<string, unknown> | null;
  native?: NativeRef | null;
  requested_at?: string | null;
  resolved_at?: string | null;
}

export type PlanStatus =
  | 'none'
  | 'discovering'
  | 'draft'
  | 'awaiting_approval'
  | 'revision_requested'
  | 'approved'
  | 'implementing'
  | 'completed'
  | 'cancelled'
  | 'failed'
  | string;
export type PlanEntryStatus =
  | 'pending'
  | 'in_progress'
  | 'completed'
  | 'skipped'
  | 'failed'
  | 'cancelled'
  | string;
export type PlanSource = 'native_structured' | 'markdown_file' | 'todo_tool' | 'synthetic' | string;

export interface UniversalPlanEntry {
  entry_id: string;
  label: string;
  status: PlanEntryStatus;
}

export interface PlanState {
  plan_id: string;
  session_id: string;
  turn_id?: string | null;
  status: PlanStatus;
  title?: string | null;
  content?: string | null;
  entries: UniversalPlanEntry[];
  artifact_refs: string[];
  source: PlanSource;
  partial?: boolean;
  updated_at?: string | null;
}

export type FileChangeKind = 'added' | 'modified' | 'deleted' | 'renamed' | string;

export interface DiffFile {
  path: string;
  status: FileChangeKind;
  diff?: string | null;
}

export interface DiffState {
  diff_id: string;
  session_id: string;
  turn_id?: string | null;
  title?: string | null;
  files: DiffFile[];
  updated_at?: string | null;
}

export type ProviderNotificationSeverity = 'debug' | 'info' | 'warning' | 'error' | string;

export interface ProviderNotification {
  category: string;
  title: string;
  detail?: string | null;
  status?: string | null;
  severity?: ProviderNotificationSeverity | null;
  subject?: string | null;
}

export type ArtifactKind = 'file' | 'image' | 'plan' | 'diff' | 'link' | 'native' | string;

export interface ArtifactState {
  artifact_id: string;
  session_id: string;
  turn_id?: string | null;
  kind: ArtifactKind;
  title: string;
  uri?: string | null;
  mime_type?: string | null;
  native?: NativeRef | null;
  created_at?: string | null;
}

export interface SessionSnapshot {
  session_id: string;
  latest_seq?: UniversalSeq | null;
  info?: SessionInfo | null;
  capabilities?: CapabilitySet;
  turns: Record<string, TurnState>;
  items: Record<string, ItemState>;
  approvals: Record<string, ApprovalRequest>;
  questions: Record<string, QuestionState>;
  plans: Record<string, PlanState>;
  diffs: Record<string, DiffState>;
  artifacts: Record<string, ArtifactState>;
  active_turns: string[];
}

export type UniversalEventKind =
  | { type: 'session.created'; data: { session: SessionInfo } }
  | { type: 'session.status_changed'; data: { status: SessionStatus; reason?: string | null } }
  | { type: 'session.metadata_changed'; data: { title?: string | null } }
  | { type: 'turn.started' | 'turn.status_changed' | 'turn.completed' | 'turn.failed' | 'turn.cancelled' | 'turn.interrupted' | 'turn.detached'; data: { turn: TurnState } }
  | { type: 'item.created'; data: { item: ItemState } }
  | { type: 'content.delta'; data: { block_id: string; kind?: ContentBlockKind | null; delta: string } }
  | { type: 'content.completed'; data: { block_id: string; kind?: ContentBlockKind | null; text?: string | null } }
  | { type: 'approval.requested'; data: { approval: ApprovalRequest } }
  | {
      type: 'approval.resolved';
      data: {
        approval_id: string;
        status: ApprovalStatus;
        resolved_at: string;
        resolved_by_user_id?: string | null;
        native?: NativeRef | null;
      };
    }
  | { type: 'question.requested'; data: { question: QuestionState } }
  | { type: 'question.answered'; data: { question: QuestionState } }
  | { type: 'plan.updated'; data: { plan: PlanState } }
  | { type: 'diff.updated'; data: { diff: DiffState } }
  | { type: 'artifact.created'; data: { artifact: ArtifactState } }
  | { type: 'usage.updated'; data: { usage: SessionUsageSnapshot } }
  | { type: 'error.reported'; data: { code?: string | null; message: string } }
  | { type: 'provider.notification'; data: { notification: ProviderNotification } }
  | { type: 'native.unknown'; data: { summary?: string | null } };

export interface UniversalEventEnvelope {
  protocol_version: UapProtocolVersion;
  event_id: string;
  seq: UniversalSeq;
  session_id: string;
  turn_id?: string | null;
  item_id?: string | null;
  ts: string;
  source: UniversalEventSource;
  native?: NativeRef | null;
  event: UniversalEventKind;
}

export interface BrowserSessionSnapshot {
  type: 'session_snapshot';
  protocol_version: UapProtocolVersion;
  request_id?: string;
  snapshot: SessionSnapshot;
  events: UniversalEventEnvelope[];
  snapshot_seq?: UniversalSeq | null;
  replay_from_seq?: UniversalSeq | null;
  replay_through_seq?: UniversalSeq | null;
  replay_complete: boolean;
}

export type UniversalCommand =
  | { type: 'start_turn'; input: unknown; settings?: AgentTurnSettings | null }
  | { type: 'resolve_approval'; approval_id: string; option_id: string; feedback?: string | null }
  | { type: 'answer_question'; question_id: string; answer: AgentQuestionAnswer }
  | { type: 'set_turn_settings'; settings: AgentTurnSettings }
  | { type: 'cancel_turn'; request?: SlashCommandRequest | null }
  | { type: 'request_diff'; diff_id?: string | null }
  | { type: 'revert_change'; diff_id: string; change_id?: string | null }
  | { type: string; [key: string]: unknown };

export interface UniversalCommandEnvelope {
  protocol_version: UapProtocolVersion;
  command_id: string;
  idempotency_key: string;
  session_id?: string | null;
  turn_id?: string | null;
  command: UniversalCommand;
}

export interface BrowserUniversalEventEnvelope extends UniversalEventEnvelope {
  type: 'universal_event';
}

export interface BrowserAck {
  type: 'ack';
  request_id?: string;
}

export interface BrowserError {
  type: 'error';
  request_id?: string;
  code: string;
  message: string;
}

export type BrowserServerMessage =
  | BrowserUniversalEventEnvelope
  | BrowserSessionSnapshot
  | BrowserAck
  | BrowserError;

export type ApprovalDecisionName = 'accept' | 'accept_for_session' | 'decline' | 'cancel';

export interface ApprovalDecision {
  decision: ApprovalDecisionName;
  option_id?: string;
  feedback?: string;
}

export interface ApprovalPolicyRule {
  rule_id: string;
  workspace_id: string;
  provider_id: string;
  kind: ApprovalKind;
  label: string;
  matcher: Record<string, unknown>;
  decision: ApprovalDecision;
  disabled_at?: string | null;
  created_at: string;
  updated_at: string;
}
