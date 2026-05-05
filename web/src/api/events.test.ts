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

  test('subscribes with snapshot replay cursor options', () => {
    const socket = new FakeWebSocket();
    vi.stubGlobal('WebSocket', vi.fn(() => socket));
    vi.stubGlobal('crypto', { randomUUID: () => 'request-2' });
    vi.stubGlobal('window', { location: { protocol: 'http:', host: 'localhost:7777' } });

    connectSessionEvents('session-2', { afterSeq: '41', includeSnapshot: true }, { onMessage: vi.fn() });
    socket.emitOpen();

    expect(socket.sentMessages).toEqual([
      JSON.stringify({
        type: 'subscribe_session',
        request_id: 'request-2',
        session_id: 'session-2',
        after_seq: '41',
        include_snapshot: true
      })
    ]);
  });

  test('rejects malformed universal events without a valid seq and event id', () => {
    expect(() =>
      parseBrowserServerMessage(
        JSON.stringify({
          type: 'universal_event',
          protocol_version: 'uap/1',
          seq: 'not-a-seq',
          event_id: '11111111-1111-4111-8111-111111111111',
          session_id: 's1',
          ts: '2026-05-03T12:00:00Z',
          source: 'runner',
          event: { type: 'native.unknown', data: { summary: 'bad' } }
        })
      )
    ).toThrow(/valid universal seq/);

    expect(() =>
      parseBrowserServerMessage(
        JSON.stringify({
          type: 'universal_event',
          protocol_version: 'uap/1',
          seq: '7',
          session_id: 's1',
          ts: '2026-05-03T12:00:00Z',
          source: 'runner',
          event: { type: 'native.unknown', data: { summary: 'bad' } }
        })
      )
    ).toThrow(/event_id/);

    expect(() =>
      parseBrowserServerMessage(
        JSON.stringify({
          type: 'universal_event',
          protocol_version: 'uap/1',
          seq: '7',
          event_id: 'not-a-uuid',
          session_id: 's1',
          ts: '2026-05-03T12:00:00Z',
          source: 'runner',
          event: { type: 'native.unknown', data: { summary: 'bad' } }
        })
      )
    ).toThrow(/UUID/);
  });
});

class FakeWebSocket {
  private listeners: Record<string, Array<(event: unknown) => void>> = {};
  sentMessages: string[] = [];

  addEventListener(type: string, listener: (event: unknown) => void) {
    this.listeners[type] = [...(this.listeners[type] ?? []), listener];
  }

  send(message: string) {
    this.sentMessages.push(message);
  }

  close() {}

  emitOpen() {
    for (const listener of this.listeners.open ?? []) {
      listener({});
    }
  }

  emitMessage(data: string) {
    for (const listener of this.listeners.message ?? []) {
      listener({ data });
    }
  }
}
