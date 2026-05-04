import { requestJson } from './http';
import {
  normalizeAgentOptions,
  normalizeRunners,
  normalizeSessionInfo,
  normalizeSessions,
  normalizeTurnSettings,
  normalizeWorkspaces
} from '../lib/normalizers';
import type {
  ApprovalDecision,
  AgentOptions,
  AgentQuestionAnswer,
  AgentTurnSettings,
  RunnerInfo,
  SessionInfo,
  SlashCommandDefinition,
  SlashCommandRequest,
  SlashCommandResult,
  WorkspaceRef
} from './types';

export interface CreateSessionRequest {
  workspace_id: string;
  provider_id: string;
  title?: string;
  /**
   * Optional first user message dispatched to the runner immediately after
   * the session is registered. Used by the "Implement plan in fresh thread"
   * handoff so the new session starts with the prior plan content already in
   * the model's context.
   */
  initial_message?: string;
  /**
   * Sticky turn-settings to apply to the new session. When `initial_message`
   * is also present, the override is applied to that first turn as well.
   */
  settings_override?: AgentTurnSettings;
}

export interface SendMessageRequest {
  content: string;
  /**
   * Atomic per-turn settings override. The control plane persists these as
   * the session's sticky settings BEFORE forwarding the runner command, so
   * the model sees the new collaboration mode on this very turn. Mirrors
   * Codex TUI's `SubmitUserMessageWithMode` event.
   */
  settings_override?: AgentTurnSettings;
}

export interface RenameSessionRequest {
  title: string | null;
}

export interface WorkspaceSessionRefreshSummary {
  discovered_count: number;
  refreshed_cache_count: number;
  skipped_failed_count: number;
}

export type WorkspaceSessionRefreshStatus =
  | 'queued'
  | 'sent'
  | 'accepted'
  | 'discovering'
  | 'reading_history'
  | 'sending_results'
  | 'importing'
  | 'succeeded'
  | 'failed'
  | 'cancelled';

export type WorkspaceSessionRefreshLogLevel = 'debug' | 'info' | 'warning' | 'error';

export interface WorkspaceSessionRefreshProgress {
  current?: number;
  total?: number;
  percent?: number;
}

export interface WorkspaceSessionRefreshLogEntry {
  ts: string;
  level: WorkspaceSessionRefreshLogLevel;
  status: WorkspaceSessionRefreshStatus;
  message: string;
  progress?: WorkspaceSessionRefreshProgress;
}

export interface WorkspaceSessionRefreshAccepted {
  refresh_id: string;
  status: 'queued';
}

export interface WorkspaceSessionRefreshJob {
  refresh_id: string;
  status: WorkspaceSessionRefreshStatus;
  progress?: WorkspaceSessionRefreshProgress;
  log: WorkspaceSessionRefreshLogEntry[];
  summary?: WorkspaceSessionRefreshSummary;
  error?: string;
  updated_at: string;
}

export async function listRunners(): Promise<RunnerInfo[]> {
  return normalizeRunners(await requestJson<unknown>('/api/runners'));
}

export async function listRunnerWorkspaces(runnerId: string): Promise<WorkspaceRef[]> {
  return normalizeWorkspaces(
    await requestJson<unknown>(`/api/runners/${encodeURIComponent(runnerId)}/workspaces`)
  );
}

export async function listSessions(): Promise<SessionInfo[]> {
  return normalizeSessions(await requestJson<unknown>('/api/sessions'));
}

export async function refreshWorkspaceProviderSessions(
  workspaceId: string,
  providerId: string,
  options: { force?: boolean } = {}
): Promise<WorkspaceSessionRefreshAccepted> {
  const value = await requestJson<unknown>(
    `/api/workspaces/${encodeURIComponent(workspaceId)}/providers/${encodeURIComponent(providerId)}/sessions/refresh`,
    {
      method: 'POST',
      body: options.force ? JSON.stringify({ force: true }) : undefined
    }
  );
  if (typeof value !== 'object' || value === null) {
    throw new Error('Refresh sessions returned malformed data.');
  }
  const record = value as Record<string, unknown>;
  const refreshId = stringField(record, 'refresh_id');
  const status = stringField(record, 'status');
  if (status !== 'queued') {
    throw new Error('Refresh sessions returned malformed status.');
  }
  return {
    refresh_id: refreshId,
    status
  };
}

export async function getWorkspaceProviderSessionRefreshStatus(
  workspaceId: string,
  providerId: string,
  refreshId: string
): Promise<WorkspaceSessionRefreshJob> {
  const value = await requestJson<unknown>(
    `/api/workspaces/${encodeURIComponent(workspaceId)}/providers/${encodeURIComponent(providerId)}/sessions/refresh/${encodeURIComponent(refreshId)}`
  );
  if (typeof value !== 'object' || value === null) {
    throw new Error('Refresh status returned malformed data.');
  }
  const record = value as Record<string, unknown>;
  const status = stringField(record, 'status') as WorkspaceSessionRefreshStatus;
  if (!isWorkspaceSessionRefreshStatus(status)) {
    throw new Error('Refresh status returned malformed status.');
  }
  const summaryValue = record.summary;
  return {
    refresh_id: stringField(record, 'refresh_id'),
    status,
    progress: normalizeRefreshProgress(record.progress),
    log: Array.isArray(record.log) ? record.log.map(normalizeRefreshLogEntry).filter((entry) => entry !== undefined) : [],
    summary:
      typeof summaryValue === 'object' && summaryValue !== null
        ? {
            discovered_count: numberField(summaryValue as Record<string, unknown>, 'discovered_count'),
            refreshed_cache_count: numberField(summaryValue as Record<string, unknown>, 'refreshed_cache_count'),
            skipped_failed_count: numberField(summaryValue as Record<string, unknown>, 'skipped_failed_count')
          }
        : undefined,
    error: typeof record.error === 'string' ? record.error : undefined,
    updated_at: stringField(record, 'updated_at')
  };
}

export async function getSession(sessionId: string): Promise<SessionInfo> {
  const session = normalizeSessionInfo(
    await requestJson<unknown>(`/api/sessions/${encodeURIComponent(sessionId)}`)
  );
  if (!session) {
    throw new Error(`Session ${sessionId} returned malformed data.`);
  }
  return session;
}

export async function getSessionAgentOptions(sessionId: string): Promise<AgentOptions> {
  return normalizeAgentOptions(
    await requestJson<unknown>(`/api/sessions/${encodeURIComponent(sessionId)}/agent-options`)
  );
}

export async function getSessionSettings(sessionId: string): Promise<AgentTurnSettings> {
  return normalizeTurnSettings(
    await requestJson<unknown>(`/api/sessions/${encodeURIComponent(sessionId)}/settings`)
  );
}

export async function updateSessionSettings(
  sessionId: string,
  settings: AgentTurnSettings
): Promise<AgentTurnSettings> {
  return normalizeTurnSettings(
    await requestJson<unknown>(`/api/sessions/${encodeURIComponent(sessionId)}/settings`, {
      method: 'PATCH',
      body: JSON.stringify(settings)
    })
  );
}

export async function createSession(request: CreateSessionRequest): Promise<SessionInfo> {
  const session = normalizeSessionInfo(
    await requestJson<unknown>('/api/sessions', {
      method: 'POST',
      body: JSON.stringify(request)
    })
  );
  if (!session) {
    throw new Error('Create session returned malformed data.');
  }
  return session;
}

export async function renameSession(
  sessionId: string,
  request: RenameSessionRequest
): Promise<SessionInfo> {
  const session = normalizeSessionInfo(
    await requestJson<unknown>(`/api/sessions/${encodeURIComponent(sessionId)}`, {
      method: 'PATCH',
      body: JSON.stringify(request)
    })
  );
  if (!session) {
    throw new Error('Rename session returned malformed data.');
  }
  return session;
}

export async function sendSessionMessage(
  sessionId: string,
  request: SendMessageRequest
): Promise<void> {
  await requestJson<void>(`/api/sessions/${encodeURIComponent(sessionId)}/messages`, {
    method: 'POST',
    body: JSON.stringify(request)
  });
}

export async function listSlashCommands(sessionId: string): Promise<SlashCommandDefinition[]> {
  const value = await requestJson<unknown>(
    `/api/sessions/${encodeURIComponent(sessionId)}/slash-commands`
  );
  return Array.isArray(value)
    ? value
        .map(normalizeSlashCommandDefinition)
        .filter((command): command is SlashCommandDefinition => command !== undefined)
    : [];
}

export async function executeSlashCommand(
  sessionId: string,
  request: SlashCommandRequest
): Promise<SlashCommandResult> {
  const value = await requestJson<unknown>(
    `/api/sessions/${encodeURIComponent(sessionId)}/slash-commands`,
    {
      method: 'POST',
      body: JSON.stringify(request)
    }
  );
  if (typeof value !== 'object' || value === null) {
    throw new Error('Slash command returned malformed data.');
  }
  const record = value as Record<string, unknown>;
  return {
    accepted: record.accepted === true,
    message: typeof record.message === 'string' ? record.message : '',
    session: normalizeSessionInfo(record.session),
    provider_payload:
      typeof record.provider_payload === 'object' && record.provider_payload !== null
        ? (record.provider_payload as Record<string, unknown>)
        : null
  };
}

export async function interruptSessionTurn(sessionId: string): Promise<SlashCommandResult> {
  return executeSlashCommand(sessionId, {
    command_id: 'runner.interrupt',
    arguments: {},
    raw_input: '/interrupt',
    confirmed: true
  });
}

export async function decideApproval(
  approvalId: string,
  decision: ApprovalDecision
): Promise<void> {
  await requestJson<void>(`/api/approvals/${encodeURIComponent(approvalId)}/decision`, {
    method: 'POST',
    body: JSON.stringify(decision)
  });
}

export async function answerQuestion(
  questionId: string,
  answer: AgentQuestionAnswer
): Promise<void> {
  await requestJson<void>(`/api/questions/${encodeURIComponent(questionId)}/answer`, {
    method: 'POST',
    body: JSON.stringify(answer)
  });
}

function numberField(record: Record<string, unknown>, field: string): number {
  const value = record[field];
  return typeof value === 'number' && Number.isFinite(value) ? value : 0;
}

function stringField(record: Record<string, unknown>, field: string): string {
  const value = record[field];
  if (typeof value !== 'string') {
    throw new Error(`Expected ${field} to be a string.`);
  }
  return value;
}

function isWorkspaceSessionRefreshStatus(value: string): value is WorkspaceSessionRefreshStatus {
  return [
    'queued',
    'sent',
    'accepted',
    'discovering',
    'reading_history',
    'sending_results',
    'importing',
    'succeeded',
    'failed',
    'cancelled'
  ].includes(value);
}

function isWorkspaceSessionRefreshLogLevel(value: string): value is WorkspaceSessionRefreshLogLevel {
  return ['debug', 'info', 'warning', 'error'].includes(value);
}

function normalizeRefreshProgress(value: unknown): WorkspaceSessionRefreshProgress | undefined {
  if (typeof value !== 'object' || value === null) {
    return undefined;
  }
  const record = value as Record<string, unknown>;
  const progress: WorkspaceSessionRefreshProgress = {};
  if (typeof record.current === 'number' && Number.isFinite(record.current)) {
    progress.current = record.current;
  }
  if (typeof record.total === 'number' && Number.isFinite(record.total)) {
    progress.total = record.total;
  }
  if (typeof record.percent === 'number' && Number.isFinite(record.percent)) {
    progress.percent = record.percent;
  }
  return Object.keys(progress).length > 0 ? progress : undefined;
}

function normalizeRefreshLogEntry(value: unknown): WorkspaceSessionRefreshLogEntry | undefined {
  if (typeof value !== 'object' || value === null) {
    return undefined;
  }
  const record = value as Record<string, unknown>;
  if (
    typeof record.ts !== 'string' ||
    typeof record.status !== 'string' ||
    !isWorkspaceSessionRefreshStatus(record.status) ||
    typeof record.level !== 'string' ||
    !isWorkspaceSessionRefreshLogLevel(record.level) ||
    typeof record.message !== 'string'
  ) {
    return undefined;
  }
  return {
    ts: record.ts,
    level: record.level,
    status: record.status,
    message: record.message,
    progress: normalizeRefreshProgress(record.progress)
  };
}

function normalizeSlashCommandDefinition(value: unknown): SlashCommandDefinition | undefined {
  if (typeof value !== 'object' || value === null) {
    return undefined;
  }
  const record = value as Record<string, unknown>;
  if (
    typeof record.id !== 'string' ||
    typeof record.name !== 'string' ||
    typeof record.description !== 'string' ||
    typeof record.category !== 'string' ||
    typeof record.target !== 'string' ||
    typeof record.danger_level !== 'string'
  ) {
    return undefined;
  }
  return {
    id: record.id,
    name: record.name,
    aliases: stringArray(record.aliases),
    description: record.description,
    category: record.category,
    provider_id: typeof record.provider_id === 'string' ? record.provider_id : null,
    target: record.target as SlashCommandDefinition['target'],
    danger_level: record.danger_level as SlashCommandDefinition['danger_level'],
    arguments: Array.isArray(record.arguments)
      ? record.arguments
          .map(normalizeSlashCommandArgument)
          .filter(
            (argument): argument is SlashCommandDefinition['arguments'][number] =>
              argument !== undefined
          )
      : [],
    examples: stringArray(record.examples)
  };
}

function normalizeSlashCommandArgument(value: unknown): SlashCommandDefinition['arguments'][number] | undefined {
  if (typeof value !== 'object' || value === null) {
    return undefined;
  }
  const record = value as Record<string, unknown>;
  if (typeof record.name !== 'string' || typeof record.kind !== 'string') {
    return undefined;
  }
  return {
    name: record.name,
    kind: record.kind as SlashCommandDefinition['arguments'][number]['kind'],
    required: record.required === true,
    description: typeof record.description === 'string' ? record.description : null,
    choices: stringArray(record.choices)
  };
}

function stringArray(value: unknown): string[] {
  return Array.isArray(value) ? value.filter((item): item is string => typeof item === 'string') : [];
}
