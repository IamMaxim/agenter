import { afterEach, describe, expect, test, vi } from 'vitest';

import { ApiError, ApiParseError, requestJson } from './http';

describe('requestJson', () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  test('throws ApiError with method path and status for HTTP failures', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue({
        ok: false,
        status: 503,
        text: () => Promise.resolve('')
      })
    );

    await expect(requestJson('/api/sessions')).rejects.toMatchObject({
      name: 'ApiError',
      status: 503,
      message: 'GET /api/sessions failed with 503'
    } satisfies Partial<ApiError>);
  });

  test('throws ApiError with server-provided JSON error detail', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue({
        ok: false,
        status: 502,
        text: () =>
          Promise.resolve(
            JSON.stringify({
              code: 'provider_command_failed',
              message: 'thread not found'
            })
          )
      })
    );

    await expect(requestJson('/api/sessions/s1/slash-commands', { method: 'POST' })).rejects.toMatchObject({
      name: 'ApiError',
      status: 502,
      code: 'provider_command_failed',
      detail: 'thread not found',
      message: 'thread not found'
    } satisfies Partial<ApiError>);
  });

  test('throws ApiParseError when JSON response is malformed', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue({
        ok: true,
        status: 200,
        text: () => Promise.resolve('{not json')
      })
    );

    await expect(requestJson('/api/sessions')).rejects.toBeInstanceOf(ApiParseError);
    await expect(requestJson('/api/sessions')).rejects.toMatchObject({
      name: 'ApiParseError',
      message: 'GET /api/sessions returned invalid JSON'
    });
  });
});
