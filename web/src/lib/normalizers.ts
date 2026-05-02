import type {
  AgentCollaborationMode,
  AgentModelOption,
  AgentOptions,
  AgentReasoningEffort,
  AgentTurnSettings,
  AppEventType,
  BrowserEventEnvelope,
  BrowserServerMessage,
  RunnerInfo,
  RunnerStatus,
  SessionInfo,
  SessionStatus,
  SessionUsageContext,
  SessionUsageSnapshot,
  SessionUsageWindow,
  WorkspaceRef
} from '../api/types';

export const defaultReasoningEfforts: AgentReasoningEffort[] = [
  'minimal',
  'low',
  'medium',
  'high',
  'xhigh'
];

const reasoningEfforts = new Set<AgentReasoningEffort>([
  'none',
  'minimal',
  'low',
  'medium',
  'high',
  'xhigh'
]);

const sessionStatuses = new Set<SessionStatus>([
  'starting',
  'running',
  'waiting_for_input',
  'waiting_for_approval',
  'idle',
  'stopped',
  'completed',
  'interrupted',
  'degraded',
  'failed',
  'archived'
]);

const appEventTypes = new Set<AppEventType>([
  'session_started',
  'session_status_changed',
  'user_message',
  'agent_message_delta',
  'agent_message_completed',
  'plan_updated',
  'tool_started',
  'tool_updated',
  'tool_completed',
  'command_started',
  'command_output_delta',
  'command_completed',
  'file_change_proposed',
  'file_change_applied',
  'file_change_rejected',
  'approval_requested',
  'approval_resolved',
  'question_requested',
  'question_answered',
  'turn_diff_updated',
  'item_reasoning',
  'server_request_resolved',
  'mcp_tool_call_progress',
  'thread_realtime_event',
  'provider_event',
  'error'
]);

export function normalizeAgentOptions(value: unknown): AgentOptions {
  const record = objectRecord(value);
  return {
    models: arrayValue(record.models).map(normalizeAgentModelOption).filter(isPresent),
    collaboration_modes: arrayValue(record.collaboration_modes)
      .map(normalizeCollaborationMode)
      .filter(isPresent)
  };
}

export function normalizeTurnSettings(value: unknown): AgentTurnSettings {
  const record = objectRecord(value);
  const settings: AgentTurnSettings = {};
  if (typeof record.model === 'string') {
    settings.model = record.model;
  }
  if (isReasoningEffort(record.reasoning_effort)) {
    settings.reasoning_effort = record.reasoning_effort;
  }
  if (typeof record.collaboration_mode === 'string') {
    settings.collaboration_mode = record.collaboration_mode;
  }
  return settings;
}

export function effortsForSelectedModel(
  options: AgentOptions,
  settings: AgentTurnSettings
): AgentReasoningEffort[] {
  const selected = options.models.find((model) => model.id === settings.model);
  return selected?.supported_reasoning_efforts.length
    ? selected.supported_reasoning_efforts
    : defaultReasoningEfforts;
}

export function normalizeRunners(value: unknown): RunnerInfo[] {
  return arrayValue(value).map(normalizeRunner).filter(isPresent);
}

export function normalizeWorkspaces(value: unknown): WorkspaceRef[] {
  return arrayValue(value).map(normalizeWorkspace).filter(isPresent);
}

export function normalizeSessions(value: unknown): SessionInfo[] {
  return arrayValue(value).map(normalizeSession).filter(isPresent);
}

export function normalizeSessionInfo(value: unknown): SessionInfo | undefined {
  return normalizeSession(value);
}

export function normalizeBrowserEventEnvelope(value: unknown): BrowserEventEnvelope {
  const record = objectRecord(value);
  const eventRecord = objectRecord(record.event);
  return {
    type: 'app_event',
    ...(typeof record.event_id === 'string' ? { event_id: record.event_id } : {}),
    event: {
      type: isAppEventType(eventRecord.type) ? eventRecord.type : 'error',
      payload: objectRecord(eventRecord.payload)
    }
  };
}

export function normalizeBrowserServerMessage(value: unknown): BrowserServerMessage {
  const record = objectRecord(value);
  if (record.type === 'ack') {
    return {
      type: 'ack',
      ...(typeof record.request_id === 'string' ? { request_id: record.request_id } : {})
    };
  }
  if (record.type === 'error') {
    return {
      type: 'error',
      ...(typeof record.request_id === 'string' ? { request_id: record.request_id } : {}),
      code: typeof record.code === 'string' ? record.code : 'unknown',
      message: typeof record.message === 'string' ? record.message : 'Unknown browser event error.'
    };
  }
  return normalizeBrowserEventEnvelope(value);
}

function normalizeAgentModelOption(value: unknown): AgentModelOption | undefined {
  const record = objectRecord(value);
  if (typeof record.id !== 'string') {
    return undefined;
  }
  return {
    id: record.id,
    display_name: typeof record.display_name === 'string' ? record.display_name : record.id,
    description: typeof record.description === 'string' ? record.description : null,
    is_default: record.is_default === true,
    default_reasoning_effort: isReasoningEffort(record.default_reasoning_effort)
      ? record.default_reasoning_effort
      : null,
    supported_reasoning_efforts: arrayValue(record.supported_reasoning_efforts).filter(
      isReasoningEffort
    ),
    input_modalities: arrayValue(record.input_modalities).filter(isString)
  };
}

function normalizeCollaborationMode(value: unknown): AgentCollaborationMode | undefined {
  const record = objectRecord(value);
  if (typeof record.id !== 'string') {
    return undefined;
  }
  return {
    id: record.id,
    label: typeof record.label === 'string' ? record.label : record.id,
    model: typeof record.model === 'string' ? record.model : null,
    reasoning_effort: isReasoningEffort(record.reasoning_effort) ? record.reasoning_effort : null
  };
}

function normalizeRunner(value: unknown): RunnerInfo | undefined {
  const record = objectRecord(value);
  if (typeof record.runner_id !== 'string') {
    return undefined;
  }
  return {
    runner_id: record.runner_id,
    name: typeof record.name === 'string' ? record.name : record.runner_id,
    status: normalizeRunnerStatus(record.status),
    last_seen_at: typeof record.last_seen_at === 'string' ? record.last_seen_at : null
  };
}

function normalizeWorkspace(value: unknown): WorkspaceRef | undefined {
  const record = objectRecord(value);
  if (typeof record.workspace_id !== 'string' || typeof record.runner_id !== 'string') {
    return undefined;
  }
  return {
    workspace_id: record.workspace_id,
    runner_id: record.runner_id,
    path: typeof record.path === 'string' ? record.path : 'Unknown workspace',
    display_name: typeof record.display_name === 'string' ? record.display_name : null
  };
}

function normalizeSession(value: unknown): SessionInfo | undefined {
  const record = objectRecord(value);
  if (typeof record.session_id !== 'string' || typeof record.workspace_id !== 'string') {
    return undefined;
  }
  return {
    session_id: record.session_id,
    owner_user_id: typeof record.owner_user_id === 'string' ? record.owner_user_id : '',
    runner_id: typeof record.runner_id === 'string' ? record.runner_id : '',
    workspace_id: record.workspace_id,
    provider_id: typeof record.provider_id === 'string' ? record.provider_id : 'unknown',
    status: isSessionStatus(record.status) ? record.status : 'degraded',
    external_session_id:
      typeof record.external_session_id === 'string' ? record.external_session_id : null,
    title: typeof record.title === 'string' ? record.title : null,
    created_at: typeof record.created_at === 'string' ? record.created_at : null,
    updated_at: typeof record.updated_at === 'string' ? record.updated_at : null,
    usage: normalizeSessionUsage(record.usage)
  };
}

export function normalizeSessionUsage(value: unknown): SessionUsageSnapshot | null {
  if (value === undefined || value === null) {
    return null;
  }
  const record = objectRecord(value);
  return {
    mode_label: typeof record.mode_label === 'string' ? record.mode_label : null,
    model: typeof record.model === 'string' ? record.model : null,
    reasoning_effort: isReasoningEffort(record.reasoning_effort) ? record.reasoning_effort : null,
    context: normalizeUsageContext(record.context),
    window_5h: normalizeUsageWindow(record.window_5h),
    week: normalizeUsageWindow(record.week)
  };
}

function normalizeUsageContext(value: unknown): SessionUsageContext | null {
  if (value === undefined || value === null) {
    return null;
  }
  const record = objectRecord(value);
  return {
    used_percent: numberOrNull(record.used_percent),
    used_tokens: numberOrNull(record.used_tokens),
    total_tokens: numberOrNull(record.total_tokens)
  };
}

function normalizeUsageWindow(value: unknown): SessionUsageWindow | null {
  if (value === undefined || value === null) {
    return null;
  }
  const record = objectRecord(value);
  return {
    used_percent: numberOrNull(record.used_percent),
    remaining_percent: numberOrNull(record.remaining_percent),
    resets_at: typeof record.resets_at === 'string' ? record.resets_at : null,
    window_label: typeof record.window_label === 'string' ? record.window_label : null,
    remaining_text_hint:
      typeof record.remaining_text_hint === 'string' ? record.remaining_text_hint : null
  };
}

function normalizeRunnerStatus(value: unknown): RunnerStatus {
  return typeof value === 'string' ? value : 'offline';
}

function objectRecord(value: unknown): Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : {};
}

function arrayValue(value: unknown): unknown[] {
  return Array.isArray(value) ? value : [];
}

function numberOrNull(value: unknown): number | null {
  return typeof value === 'number' && Number.isFinite(value) ? value : null;
}

function isReasoningEffort(value: unknown): value is AgentReasoningEffort {
  return typeof value === 'string' && reasoningEfforts.has(value as AgentReasoningEffort);
}

function isSessionStatus(value: unknown): value is SessionStatus {
  return typeof value === 'string' && sessionStatuses.has(value as SessionStatus);
}

function isAppEventType(value: unknown): value is AppEventType {
  return typeof value === 'string' && appEventTypes.has(value as AppEventType);
}

function isString(value: unknown): value is string {
  return typeof value === 'string';
}

function isPresent<T>(value: T | undefined): value is T {
  return value !== undefined;
}
