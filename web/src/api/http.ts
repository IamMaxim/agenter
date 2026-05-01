import { debugLog } from './debug';

export class ApiError extends Error {
  readonly status: number;
  readonly code?: string;
  readonly detail?: string;
  readonly payload?: unknown;

  constructor(status: number, message: string, options: { code?: string; detail?: string; payload?: unknown } = {}) {
    super(message);
    this.name = 'ApiError';
    this.status = status;
    this.code = options.code;
    this.detail = options.detail;
    this.payload = options.payload;
  }
}

export class ApiParseError extends Error {
  readonly cause: unknown;

  constructor(message: string, cause: unknown) {
    super(message);
    this.name = 'ApiParseError';
    this.cause = cause;
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
    const errorText = await response.text().catch(() => '');
    const parsed = parseErrorBody(errorText);
    const message = parsed.detail ?? parsed.message ?? `${method} ${path} failed with ${response.status}`;
    debugLog('api:error', { method, path, status: response.status, code: parsed.code });
    throw new ApiError(response.status, message, {
      code: parsed.code,
      detail: parsed.detail ?? parsed.message,
      payload: parsed.payload
    });
  }

  debugLog('api:ok', { method, path, status: response.status });

  if (response.status === 204) {
    return undefined as T;
  }

  const text = await response.text();
  if (text.length === 0) {
    return undefined as T;
  }

  try {
    return JSON.parse(text) as T;
  } catch (error) {
    debugLog('api:parse-error', { method, path });
    throw new ApiParseError(`${method} ${path} returned invalid JSON`, error);
  }
}

function parseErrorBody(text: string): {
  code?: string;
  message?: string;
  detail?: string;
  payload?: unknown;
} {
  if (text.length === 0) {
    return {};
  }
  try {
    const value = JSON.parse(text) as unknown;
    if (typeof value !== 'object' || value === null) {
      return { message: text, payload: value };
    }
    const record = value as Record<string, unknown>;
    return {
      code: typeof record.code === 'string' ? record.code : undefined,
      message: typeof record.message === 'string' ? record.message : undefined,
      detail: typeof record.detail === 'string' ? record.detail : undefined,
      payload: value
    };
  } catch {
    return { message: text };
  }
}
