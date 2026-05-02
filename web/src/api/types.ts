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
}

export type AgentProviderId = 'codex' | 'qwen' | string;

export type SessionStatus =
  | 'starting'
  | 'running'
  | 'waiting_for_input'
  | 'waiting_for_approval'
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
  provider_payload?: Record<string, unknown> | null;
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
}

export interface AgentQuestionAnswer {
  question_id: string;
  answers: Record<string, string[]>;
}

export type AppEventType =
  | 'session_started'
  | 'session_status_changed'
  | 'user_message'
  | 'agent_message_delta'
  | 'agent_message_completed'
  | 'plan_updated'
  | 'tool_started'
  | 'tool_updated'
  | 'tool_completed'
  | 'command_started'
  | 'command_output_delta'
  | 'command_completed'
  | 'file_change_proposed'
  | 'file_change_applied'
  | 'file_change_rejected'
  | 'approval_requested'
  | 'approval_resolved'
  | 'question_requested'
  | 'question_answered'
  | 'turn_diff_updated'
  | 'item_reasoning'
  | 'server_request_resolved'
  | 'mcp_tool_call_progress'
  | 'thread_realtime_event'
  | 'provider_event'
  | 'error';

export interface AppEvent {
  type: AppEventType;
  payload: Record<string, unknown>;
}

export interface BrowserEventEnvelope {
  type: 'app_event';
  event_id?: string;
  event: AppEvent;
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

export type BrowserServerMessage = BrowserEventEnvelope | BrowserAck | BrowserError;

export type ApprovalDecisionName = 'accept' | 'accept_for_session' | 'decline' | 'cancel';

export interface ApprovalDecision {
  decision: ApprovalDecisionName;
}
