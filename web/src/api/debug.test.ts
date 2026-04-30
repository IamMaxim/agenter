import { beforeEach, describe, expect, it, vi } from 'vitest';

import { debugLog, isDebugEnabled } from './debug';

describe('debug logging', () => {
  beforeEach(() => {
    vi.unstubAllEnvs();
    vi.restoreAllMocks();
  });

  it('is disabled by default', () => {
    const consoleSpy = vi.spyOn(console, 'debug').mockImplementation(() => undefined);

    expect(isDebugEnabled()).toBe(false);
    debugLog('api:start', { path: '/api/auth/me' });

    expect(consoleSpy).not.toHaveBeenCalled();
  });

  it('logs when VITE_AGENTER_DEBUG is enabled', () => {
    vi.stubEnv('VITE_AGENTER_DEBUG', '1');
    const consoleSpy = vi.spyOn(console, 'debug').mockImplementation(() => undefined);

    expect(isDebugEnabled()).toBe(true);
    debugLog('api:start', { path: '/api/auth/me' });

    expect(consoleSpy).toHaveBeenCalledWith('[agenter]', 'api:start', {
      path: '/api/auth/me'
    });
  });
});
