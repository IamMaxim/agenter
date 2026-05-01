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
  WorkspaceRef
} from './types';

export interface CreateSessionRequest {
  workspace_id: string;
  provider_id: string;
  title?: string;
}

export interface SendMessageRequest {
  content: string;
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

export async function sendSessionMessage(
  sessionId: string,
  request: SendMessageRequest
): Promise<void> {
  await requestJson<void>(`/api/sessions/${encodeURIComponent(sessionId)}/messages`, {
    method: 'POST',
    body: JSON.stringify(request)
  });
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
