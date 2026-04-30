import { debugLog } from './debug';

export class ApiError extends Error {
  readonly status: number;

  constructor(status: number, message: string) {
    super(message);
    this.name = 'ApiError';
    this.status = status;
  }
}

export async function requestJson<T>(path: string, init: RequestInit = {}): Promise<T> {
  const method = init.method ?? 'GET';
  debugLog('api:start', { method, path });
  const response = await fetch(path, {
    credentials: 'include',
    headers: {
      'Content-Type': 'application/json',
      ...init.headers
    },
    ...init
  });

  if (!response.ok) {
    debugLog('api:error', { method, path, status: response.status });
    throw new ApiError(response.status, `${method} ${path} failed`);
  }

  debugLog('api:ok', { method, path, status: response.status });

  if (response.status === 204) {
    return undefined as T;
  }

  const text = await response.text();
  if (text.length === 0) {
    return undefined as T;
  }

  return JSON.parse(text) as T;
}
