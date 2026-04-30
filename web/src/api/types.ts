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

export interface RunnerInfo {
  runner_id: string;
  name: string;
  status?: string;
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
