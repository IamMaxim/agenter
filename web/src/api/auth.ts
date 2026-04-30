import { requestJson } from './http';
import type { AuthenticatedUser } from './types';

export interface PasswordLoginRequest {
  email: string;
  password: string;
}

export async function loginPassword(request: PasswordLoginRequest): Promise<void> {
  await requestJson<{ ok: true }>('/api/auth/password/login', {
    method: 'POST',
    body: JSON.stringify(request)
  });
}

export async function logout(): Promise<void> {
  await requestJson<void>('/api/auth/password/logout', {
    method: 'POST'
  });
}

export async function getCurrentUser(): Promise<AuthenticatedUser> {
  return requestJson<AuthenticatedUser>('/api/auth/me');
}
