import { describe, expect, test } from 'vitest';
import { applyChatEnvelope, createChatState } from './chatEvents';
import type { BrowserEventEnvelope } from '../api/types';

describe('chat event state', () => {
  test('accumulates assistant deltas by message id and ignores duplicate event ids', () => {
    let state = createChatState();
    const firstDelta: BrowserEventEnvelope = {
      type: 'app_event',
      event_id: 'evt-1',
      event: {
        type: 'agent_message_delta',
        payload: { session_id: 's1', message_id: 'm1', delta: 'hello ' }
      }
    };

    state = applyChatEnvelope(state, firstDelta);
    state = applyChatEnvelope(state, firstDelta);
    state = applyChatEnvelope(state, {
      type: 'app_event',
      event_id: 'evt-2',
      event: {
        type: 'agent_message_delta',
        payload: { session_id: 's1', message_id: 'm1', delta: 'world' }
      }
    });

    expect(state.items).toEqual([
      {
        id: 'agent:m1',
        kind: 'assistant',
        messageId: 'm1',
        content: 'hello world',
        completed: false
      }
    ]);
  });

  test('updates approval cards when a resolution arrives', () => {
    let state = createChatState();
    state = applyChatEnvelope(state, {
      type: 'app_event',
      event_id: 'evt-approval',
      event: {
        type: 'approval_requested',
        payload: {
          session_id: 's1',
          approval_id: 'a1',
          kind: 'command',
          title: 'Run tests',
          details: 'cargo test'
        }
      }
    });
    state = applyChatEnvelope(state, {
      type: 'app_event',
      event_id: 'evt-resolved',
      event: {
        type: 'approval_resolved',
        payload: {
          session_id: 's1',
          approval_id: 'a1',
          decision: { decision: 'accept' },
          resolved_at: '2026-04-30T00:00:00Z'
        }
      }
    });

    expect(state.items[0]).toMatchObject({
      id: 'approval:a1',
      kind: 'approval',
      approvalId: 'a1',
      resolvedDecision: 'accept'
    });
  });
});
