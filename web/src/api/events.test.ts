import { describe, expect, test, vi } from 'vitest';

import { connectSessionEvents, parseBrowserServerMessage } from './events';

describe('browser websocket events', () => {
  test('reports invalid websocket JSON through error handler instead of throwing', () => {
    const error = vi.fn();
    const message = vi.fn();
    const socket = new FakeWebSocket();
    vi.stubGlobal('WebSocket', vi.fn(() => socket));
    vi.stubGlobal('crypto', { randomUUID: () => 'request-1' });
    vi.stubGlobal('window', { location: { protocol: 'http:', host: 'localhost:7777' } });

    connectSessionEvents('session-1', {
      onMessage: message,
      onError: error
    });

    expect(() => socket.emitMessage('{bad json')).not.toThrow();
    expect(message).not.toHaveBeenCalled();
    expect(error).toHaveBeenCalledWith(expect.any(Error));
  });

  test('normalizes valid websocket messages', () => {
    expect(
      parseBrowserServerMessage(
        JSON.stringify({
          type: 'app_event',
          event_id: 10,
          event: { type: 'error', payload: null }
        })
      )
    ).toEqual({
      type: 'app_event',
      event: {
        type: 'error',
        payload: {}
      }
    });
  });
});

class FakeWebSocket {
  private listeners: Record<string, Array<(event: unknown) => void>> = {};

  addEventListener(type: string, listener: (event: unknown) => void) {
    this.listeners[type] = [...(this.listeners[type] ?? []), listener];
  }

  send() {}

  close() {}

  emitMessage(data: string) {
    for (const listener of this.listeners.message ?? []) {
      listener({ data });
    }
  }
}
