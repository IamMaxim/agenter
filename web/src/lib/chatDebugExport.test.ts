import { describe, expect, test } from 'vitest';

import type { BrowserServerMessage } from '../api/types';
import type { ChatState } from './chatEvents';
import { buildChatDebugExport, summarizeChatDebugState, type ChatDebugWsEntry } from './chatDebugExport';
import { createUniversalClientState } from './sessionSnapshot';

describe('chat debug export', () => {
  test('includes websocket messages, row order, chat rows, and approval source timestamps', () => {
    const universalState = createUniversalClientState();
    universalState.latestSeq = '9';
    universalState.usingUniversal = true;
    universalState.snapshot = {
      session_id: 's1',
      latest_seq: '9',
      turns: {},
      items: {},
      approvals: {
        ap1: {
          approval_id: 'ap1',
          session_id: 's1',
          kind: 'command',
          title: 'Run command',
          options: [],
          status: 'approved',
          requested_at: '2026-05-04T12:00:00Z',
          resolved_at: '2026-05-04T12:00:10Z'
        }
      },
      questions: {},
      plans: {},
      diffs: {},
      artifacts: {},
      active_turns: []
    };
    universalState.rowOrder.set('approval:ap1', {
      seq: '2',
      ts: '2026-05-04T12:00:00Z',
      index: 1
    });

    const chatState: ChatState = {
      seenEventIds: new Set(),
      items: [
        {
          id: 'approval:ap1',
          kind: 'approval',
          approvalId: 'ap1',
          title: 'Run command',
          options: [],
          status: 'approved',
          resolvedDecision: 'accept'
        }
      ],
      planTurnComplete: false
    };
    const message: BrowserServerMessage = {
      type: 'universal_event',
      protocol_version: 'uap/2',
      event_id: 'evt-1',
      seq: '9',
      session_id: 's1',
      ts: '2026-05-04T12:00:10Z',
      source: 'control_plane',
      event: {
        type: 'approval.requested',
        data: {
          approval: {
            approval_id: 'ap1',
            session_id: 's1',
            kind: 'provider_specific',
            title: 'Approval resolved',
            options: [],
            status: 'approved',
            resolved_at: '2026-05-04T12:00:10Z'
          }
        }
      }
    };
    const wsMessages: ChatDebugWsEntry[] = [
      {
        receivedAt: '2026-05-04T12:00:11Z',
        message,
        stateAfter: summarizeChatDebugState({ chatState, visibleItems: chatState.items, universalState })
      }
    ];

    const exported = buildChatDebugExport({
      activeSessionId: 's1',
      connectionState: 'Subscribed',
      verbosity: 'debug',
      chatState,
      visibleItems: chatState.items,
      universalState,
      wsMessages
    });

    expect(exported.wsMessages).toHaveLength(1);
    expect(exported.universal.rowOrder).toEqual([
      { rowId: 'approval:ap1', seq: '2', ts: '2026-05-04T12:00:00Z', index: 1 }
    ]);
    expect(exported.chatItems[0].diagnostics).toMatchObject({
      id: 'approval:ap1',
      kind: 'approval',
      status: 'approved',
      resolvedDecision: 'accept',
      rowOrder: { seq: '2', ts: '2026-05-04T12:00:00Z', index: 1 },
      source: {
        kind: 'approval',
        requested_at: '2026-05-04T12:00:00Z',
        resolved_at: '2026-05-04T12:00:10Z'
      }
    });
  });
});
