import { requestJson } from './http';
import {
  normalizeAgentOptions,
  normalizeBrowserEventEnvelope,
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
  BrowserEventEnvelope,
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
  providerId: string
): Promise<WorkspaceSessionRefreshSummary> {
  const value = await requestJson<unknown>(
    `/api/workspaces/${encodeURIComponent(workspaceId)}/providers/${encodeURIComponent(providerId)}/sessions/refresh`,
    { method: 'POST' }
  );
  if (typeof value !== 'object' || value === null) {
    throw new Error('Refresh sessions returned malformed data.');
  }
  const record = value as Record<string, unknown>;
  return {
    discovered_count: numberField(record, 'discovered_count'),
    refreshed_cache_count: numberField(record, 'refreshed_cache_count'),
    skipped_failed_count: numberField(record, 'skipped_failed_count')
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

export async function getSessionHistory(sessionId: string): Promise<BrowserEventEnvelope[]> {
  const value = await requestJson<unknown>(`/api/sessions/${encodeURIComponent(sessionId)}/history`);
  return Array.isArray(value) ? value.map(normalizeBrowserEventEnvelope) : [];
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

export async function decideApproval(
  approvalId: string,
  decision: ApprovalDecision
): Promise<BrowserEventEnvelope> {
  return normalizeBrowserEventEnvelope(
    await requestJson<unknown>(`/api/approvals/${encodeURIComponent(approvalId)}/decision`, {
      method: 'POST',
      body: JSON.stringify(decision)
    })
  );
}

export async function answerQuestion(
  questionId: string,
  answer: AgentQuestionAnswer
): Promise<BrowserEventEnvelope> {
  return normalizeBrowserEventEnvelope(
    await requestJson<unknown>(`/api/questions/${encodeURIComponent(questionId)}/answer`, {
      method: 'POST',
      body: JSON.stringify(answer)
    })
  );
}

function numberField(record: Record<string, unknown>, field: string): number {
  const value = record[field];
  return typeof value === 'number' && Number.isFinite(value) ? value : 0;
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
