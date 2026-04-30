import { requestJson } from './http';
import type { SessionInfo, WorkspaceRef } from './types';

export interface CreateSessionRequest {
  workspace_id: string;
  provider_id: string;
  title?: string;
}

export interface SendMessageRequest {
  content: string;
}

export async function listWorkspaces(): Promise<WorkspaceRef[]> {
  return requestJson<WorkspaceRef[]>('/api/workspaces');
}

export async function listSessions(): Promise<SessionInfo[]> {
  return requestJson<SessionInfo[]>('/api/sessions');
}

export async function getSession(sessionId: string): Promise<SessionInfo> {
  return requestJson<SessionInfo>(`/api/sessions/${encodeURIComponent(sessionId)}`);
}

export async function createSession(request: CreateSessionRequest): Promise<SessionInfo> {
  return requestJson<SessionInfo>('/api/sessions', {
    method: 'POST',
    body: JSON.stringify(request)
  });
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
