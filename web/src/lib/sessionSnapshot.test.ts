import { describe, expect, test } from 'vitest';

import {
  applyUniversalClientMessage,
  createUniversalClientState,
  hasCapabilitySignal,
  materializeSnapshotChatState
} from './sessionSnapshot';
import type {
  BrowserServerMessage,
  CapabilitySet,
  SessionSnapshot,
  UniversalEventEnvelope
} from '../api/types';

const baseCapabilities: CapabilitySet = {
  protocol: { streaming: true, session_resume: true, session_history: true, interrupt: true, snapshots: true, after_seq_replay: true },
  content: { text: true, images: false, file_changes: true, diffs: true },
  tools: { command_execution: true, tool_user_input: true },
  approvals: { enabled: true, per_session_allow: true, deny_with_feedback: true, cancel_turn: true },
  plan: { updates: true, approval: true },
  modes: { model_selection: true, reasoning_effort: true, collaboration_modes: true },
  integration: { mcp_elicitation: false }
};

function snapshot(overrides: Partial<SessionSnapshot> = {}): SessionSnapshot {
  return {
    session_id: 's1',
    latest_seq: '5',
    capabilities: baseCapabilities,
    turns: {},
    items: {},
    approvals: {},
    plans: {},
    diffs: {},
    artifacts: {},
    active_turns: [],
    ...overrides
  };
}

function universalEvent(seq: string, eventId = `evt-${seq}`): UniversalEventEnvelope {
  return {
    event_id: eventId,
    seq,
    session_id: 's1',
    ts: '2026-05-03T12:00:00Z',
    source: 'runner',
    event: {
      type: 'content.delta',
      data: {
        block_id: 'assistant-block',
        kind: 'text',
        delta: ' live'
      }
    }
  };
}

describe('universal session snapshot client reducer', () => {
  test('materializes snapshot turns, items, plans, approvals, diffs, and artifacts into chat rows', () => {
    const state = materializeSnapshotChatState(
      snapshot({
        items: {
          u1: {
            item_id: 'u1',
            session_id: 's1',
            turn_id: 't1',
            role: 'user',
            status: 'completed',
            content: [{ block_id: 'u1-text', kind: 'text', text: 'Build it' }]
          },
          a1: {
            item_id: 'a1',
            session_id: 's1',
            turn_id: 't1',
            role: 'assistant',
            status: 'streaming',
            content: [{ block_id: 'assistant-block', kind: 'text', text: 'Working' }]
          }
        },
        turns: {
          t1: {
            turn_id: 't1',
            session_id: 's1',
            status: 'running'
          }
        },
        plans: {
          p1: {
            plan_id: 'p1',
            session_id: 's1',
            turn_id: 't1',
            status: 'implementing',
            title: 'Plan',
            content: 'Use markdown',
            entries: [
              { entry_id: 'e1', label: 'Add types', status: 'completed' },
              { entry_id: 'e2', label: 'Wire replay', status: 'in_progress' }
            ],
            artifact_refs: [],
            source: 'native_structured',
            partial: false
          }
        },
        approvals: {
          ap1: {
            approval_id: 'ap1',
            session_id: 's1',
            kind: 'command',
            title: 'Run tests',
            details: 'npm run test',
            options: [
              { option_id: 'approve_once', kind: 'approve_once', label: 'Approve once' },
              { option_id: 'deny', kind: 'deny', label: 'Deny' }
            ],
            status: 'pending',
            native_blocking: true
          }
        },
        diffs: {
          d1: {
            diff_id: 'd1',
            session_id: 's1',
            title: 'Frontend diff',
            files: [{ path: 'web/src/lib/sessionSnapshot.ts', status: 'modified', diff: '@@' }]
          }
        },
        artifacts: {
          art1: {
            artifact_id: 'art1',
            session_id: 's1',
            kind: 'file',
            title: 'Run log',
            uri: 'artifact://run-log'
          }
        },
        active_turns: ['t1']
      })
    );

    expect(state.activity).toEqual({ status: 'running', active: true, label: 'Working' });
    expect(state.items).toMatchObject([
      { id: 'user:u1', kind: 'user', content: 'Build it' },
      { id: 'agent:a1', kind: 'assistant', content: 'Working', completed: false },
      { id: 'plan:p1', kind: 'plan' },
      { id: 'approval:ap1', kind: 'approval' },
      { id: 'event:diff:d1', kind: 'inlineEvent', eventKind: 'file' },
      { id: 'event:artifact:art1', kind: 'inlineEvent', eventKind: 'event' }
    ]);
    const plan = state.items.find((item) => item.id === 'plan:p1');
    expect(plan).toMatchObject({ kind: 'plan' });
    expect(plan && 'entries' in plan ? plan.entries?.[0] : undefined).toMatchObject({
      label: 'Add types'
    });
    const approval = state.items.find((item) => item.id === 'approval:ap1');
    expect(approval).toMatchObject({ kind: 'approval' });
    expect(approval && 'options' in approval ? approval.options?.[0] : undefined).toMatchObject({
      optionId: 'approve_once',
      decision: 'accept',
      label: 'Approve once'
    });
  });

  test('materializes universal semantic command subagent and mcp tool rows', () => {
    const state = materializeSnapshotChatState(
      snapshot({
        items: {
          cmd1: {
            item_id: 'cmd1',
            session_id: 's1',
            role: 'tool',
            status: 'completed',
            content: [
              { block_id: 'cmd1-call', kind: 'tool_call', text: 'cargo test' },
              { block_id: 'cmd1-stdout', kind: 'command_output', text: 'ok\n' }
            ],
            tool: {
              kind: 'command',
              name: 'command',
              title: 'cargo test',
              status: 'completed',
              command: {
                command: 'cargo test',
                cwd: '/work/agenter',
                source: 'unifiedExecStartup',
                process_id: '123',
                exit_code: 0,
                duration_ms: 17,
                success: true,
                actions: [
                  {
                    kind: 'read',
                    label: 'Read SKILL.md',
                    detail: '/tmp/skills/demo/SKILL.md',
                    path: '/tmp/skills/demo/SKILL.md'
                  }
                ]
              }
            }
          },
          sub1: {
            item_id: 'sub1',
            session_id: 's1',
            role: 'tool',
            status: 'completed',
            content: [{ block_id: 'sub1-result', kind: 'tool_result', text: 'Spawn subagent' }],
            tool: {
              kind: 'subagent',
              name: 'spawnAgent',
              title: 'Spawn subagent',
              status: 'completed',
              subagent: {
                operation: 'spawn',
                agent_ids: ['agent-1'],
                model: 'gpt-5.5',
                reasoning_effort: 'medium',
                prompt: 'Implement task',
                states: []
              }
            }
          },
          mcp1: {
            item_id: 'mcp1',
            session_id: 's1',
            role: 'tool',
            status: 'completed',
            content: [{ block_id: 'mcp1-result', kind: 'tool_result', text: 'read_file' }],
            tool: {
              kind: 'mcp',
              name: 'read_file',
              title: 'read_file',
              status: 'completed',
              detail: '{\n  "path": "README.md"\n}',
              mcp: {
                server: 'filesystem',
                tool: 'read_file',
                arguments_summary: '{\n  "path": "README.md"\n}'
              }
            }
          }
        }
      })
    );

    expect(state.items).toMatchObject([
      {
        id: 'event:item:cmd1',
        kind: 'inlineEvent',
        eventKind: 'command',
        title: 'cargo test',
        detail: '/work/agenter · unifiedExecStartup · pid 123',
        output: 'ok\n',
        status: 'completed',
        exitCode: 0,
        durationMs: 17,
        processId: '123',
        source: 'unifiedExecStartup',
        actions: [{ kind: 'read', label: 'Read SKILL.md' }]
      },
      {
        id: 'subagent:sub1',
        kind: 'subagent',
        operation: 'spawn',
        title: 'Spawn subagent',
        status: 'completed',
        agentIds: ['agent-1'],
        model: 'gpt-5.5',
        reasoningEffort: 'medium',
        prompt: 'Implement task'
      },
      {
        id: 'event:item:mcp1',
        kind: 'inlineEvent',
        eventKind: 'tool',
        title: 'read_file',
        detail: '{\n  "path": "README.md"\n}',
        status: 'completed'
      }
    ]);
    expect(state.items.map((item) => ('title' in item ? item.title : ''))).not.toContain('tool completed');
    expect(state.items.map((item) => ('title' in item ? item.title : ''))).not.toContain('Tool activity');
  });

  test('applies snapshot first and only replays events after the snapshot cursor', () => {
    let state = createUniversalClientState();
    const message: BrowserServerMessage = {
      type: 'session_snapshot',
      snapshot: snapshot({
        latest_seq: '5',
        items: {
          a1: {
            item_id: 'a1',
            session_id: 's1',
            role: 'assistant',
            status: 'streaming',
            content: [{ block_id: 'assistant-block', kind: 'text', text: 'Snapshot' }]
          }
        }
      }),
      latest_seq: '6',
      has_more: false,
      events: [universalEvent('5', 'evt-snapshot-boundary'), universalEvent('6')]
    };

    state = applyUniversalClientMessage(state, message);

    expect(state.latestSeq).toBe('6');
    expect(state.chat.items).toMatchObject([
      { id: 'agent:a1', kind: 'assistant', content: 'Snapshot live' }
    ]);
  });

  test('dedupes duplicate seq and event id across replay and live boundary', () => {
    let state = createUniversalClientState();
    state = applyUniversalClientMessage(state, {
      type: 'session_snapshot',
      snapshot: snapshot({ latest_seq: '5' }),
      latest_seq: '6',
      has_more: false,
      events: [universalEvent('6', 'evt-live')]
    });
    state = applyUniversalClientMessage(state, {
      type: 'universal_event',
      ...universalEvent('6', 'evt-live')
    });

    expect(state.latestSeq).toBe('6');
    expect(state.chat.items).toMatchObject([
      { id: 'agent:assistant-block', kind: 'assistant', content: ' live' }
    ]);
  });

  test('rejects unseen live events at or behind the latest seq cursor', () => {
    let state = createUniversalClientState();
    state = applyUniversalClientMessage(state, {
      type: 'session_snapshot',
      snapshot: snapshot({ latest_seq: '6' }),
      latest_seq: '6',
      has_more: false,
      events: []
    });

    state = applyUniversalClientMessage(state, {
      type: 'universal_event',
      ...universalEvent('6', 'evt-same-seq-different-id')
    });
    state = applyUniversalClientMessage(state, {
      type: 'universal_event',
      ...universalEvent('5', 'evt-older')
    });

    expect(state.latestSeq).toBe('6');
    expect(state.chat.items).toEqual([]);
  });

  test('does not advance latest seq when snapshot replay is incomplete', () => {
    const state = applyUniversalClientMessage(createUniversalClientState(), {
      type: 'session_snapshot',
      snapshot: snapshot({ latest_seq: '5' }),
      latest_seq: '9',
      has_more: true,
      events: [universalEvent('9')]
    });

    expect(state.latestSeq).toBeUndefined();
    expect(state.snapshotIncomplete).toBe(true);
    expect(state.chat.items).toEqual([]);
  });

  test('keeps legacy fallback path for app_event messages', () => {
    const state = applyUniversalClientMessage(createUniversalClientState(), {
      type: 'app_event',
      event_id: 'legacy-1',
      event: {
        type: 'user_message',
        payload: { session_id: 's1', message_id: 'm1', content: 'legacy hello' }
      }
    });

    expect(state.usingUniversal).toBe(false);
    expect(state.chat.items).toMatchObject([{ id: 'user:m1', content: 'legacy hello' }]);
  });

  test('detects real capability data before feature gating existing controls', () => {
    expect(hasCapabilitySignal(undefined)).toBe(false);
    expect(hasCapabilitySignal(snapshot({ capabilities: undefined }).capabilities)).toBe(false);
    expect(hasCapabilitySignal(baseCapabilities)).toBe(true);
  });

  test('renders terminal approval snapshot states without canonical options', () => {
    const state = materializeSnapshotChatState(
      snapshot({
        approvals: {
          expired: {
            approval_id: 'expired',
            session_id: 's1',
            kind: 'command',
            title: 'Expired approval',
            options: [{ option_id: 'approve_once', kind: 'approve_once', label: 'Approve once' }],
            status: 'expired'
          },
          orphaned: {
            approval_id: 'orphaned',
            session_id: 's1',
            kind: 'tool',
            title: 'Orphaned approval',
            options: [{ option_id: 'deny', kind: 'deny', label: 'Deny' }],
            status: 'orphaned'
          }
        }
      })
    );

    expect(state.items).toMatchObject([
      { id: 'approval:expired', kind: 'approval', resolvedDecision: 'expired', options: [] },
      { id: 'approval:orphaned', kind: 'approval', resolvedDecision: 'orphaned', options: [] }
    ]);
  });

  test('preserves replay chronology for interleaved assistant approval and diff rows', () => {
    let state = createUniversalClientState();
    state = applyUniversalClientMessage(state, {
      type: 'session_snapshot',
      snapshot: snapshot({ latest_seq: '0' }),
      latest_seq: '3',
      has_more: false,
      events: [
        {
          ...universalEvent('1', 'evt-assistant'),
          item_id: 'assistant-item',
          event: {
            type: 'content.delta',
            data: { block_id: 'assistant-block', kind: 'text', delta: 'hello' }
          }
        },
        {
          ...universalEvent('2', 'evt-approval'),
          event: {
            type: 'approval.requested',
            data: {
              approval: {
                approval_id: 'ap1',
                session_id: 's1',
                kind: 'command',
                title: 'Run command',
                options: [{ option_id: 'approve_once', kind: 'approve_once', label: 'Approve once' }],
                status: 'pending'
              }
            }
          }
        },
        {
          ...universalEvent('3', 'evt-diff'),
          event: {
            type: 'diff.updated',
            data: {
              diff: {
                diff_id: 'd1',
                session_id: 's1',
                title: 'Patch',
                files: [{ path: 'a.ts', status: 'modified', diff: '@@' }]
              }
            }
          }
        }
      ]
    });

    expect(state.chat.items.map((item) => item.id)).toEqual([
      'agent:assistant-item',
      'approval:ap1',
      'event:diff:d1'
    ]);
  });

  test('uses historical replay events to order rows already present in a current snapshot', () => {
    let state = createUniversalClientState();
    state = applyUniversalClientMessage(state, {
      type: 'session_snapshot',
      snapshot: snapshot({
        latest_seq: '3',
        diffs: {
          d1: {
            diff_id: 'd1',
            session_id: 's1',
            title: 'Patch',
            files: [{ path: 'a.ts', status: 'modified', diff: '@@' }]
          }
        },
        approvals: {
          ap1: {
            approval_id: 'ap1',
            session_id: 's1',
            kind: 'command',
            title: 'Run command',
            options: [{ option_id: 'approve_once', kind: 'approve_once', label: 'Approve once' }],
            status: 'pending'
          }
        },
        items: {
          'assistant-item': {
            item_id: 'assistant-item',
            session_id: 's1',
            role: 'assistant',
            status: 'completed',
            content: [{ block_id: 'assistant-block', kind: 'text', text: 'hello' }]
          }
        }
      }),
      latest_seq: '3',
      has_more: false,
      events: [
        {
          ...universalEvent('1', 'evt-assistant'),
          item_id: 'assistant-item',
          event: {
            type: 'content.delta',
            data: { block_id: 'assistant-block', kind: 'text', delta: 'hello' }
          }
        },
        {
          ...universalEvent('2', 'evt-approval'),
          event: {
            type: 'approval.requested',
            data: {
              approval: {
                approval_id: 'ap1',
                session_id: 's1',
                kind: 'command',
                title: 'Run command',
                options: [{ option_id: 'approve_once', kind: 'approve_once', label: 'Approve once' }],
                status: 'pending'
              }
            }
          }
        },
        {
          ...universalEvent('3', 'evt-diff'),
          event: {
            type: 'diff.updated',
            data: {
              diff: {
                diff_id: 'd1',
                session_id: 's1',
                title: 'Patch',
                files: [{ path: 'a.ts', status: 'modified', diff: '@@' }]
              }
            }
          }
        }
      ]
    });

    expect(state.chat.items.map((item) => item.id)).toEqual([
      'agent:assistant-item',
      'approval:ap1',
      'event:diff:d1'
    ]);
    expect(state.chat.items[0]).toMatchObject({ content: 'hello' });
    expect([...state.rowOrder.entries()].map(([rowId, order]) => [rowId, order.seq])).toEqual([
      ['agent:assistant-item', '1'],
      ['approval:ap1', '2'],
      ['event:diff:d1', '3']
    ]);
  });

  test('historical replay ordering can place approval before assistant from current snapshot maps', () => {
    let state = createUniversalClientState();
    state = applyUniversalClientMessage(state, {
      type: 'session_snapshot',
      snapshot: snapshot({
        latest_seq: '3',
        items: {
          'assistant-item': {
            item_id: 'assistant-item',
            session_id: 's1',
            role: 'assistant',
            status: 'completed',
            content: [{ block_id: 'assistant-block', kind: 'text', text: 'hello' }]
          }
        },
        diffs: {
          d1: {
            diff_id: 'd1',
            session_id: 's1',
            title: 'Patch',
            files: [{ path: 'a.ts', status: 'modified', diff: '@@' }]
          }
        },
        approvals: {
          ap1: {
            approval_id: 'ap1',
            session_id: 's1',
            kind: 'command',
            title: 'Run command',
            options: [{ option_id: 'approve_once', kind: 'approve_once', label: 'Approve once' }],
            status: 'pending'
          }
        }
      }),
      latest_seq: '3',
      has_more: false,
      events: [
        {
          ...universalEvent('1', 'evt-approval'),
          event: {
            type: 'approval.requested',
            data: {
              approval: {
                approval_id: 'ap1',
                session_id: 's1',
                kind: 'command',
                title: 'Run command',
                options: [{ option_id: 'approve_once', kind: 'approve_once', label: 'Approve once' }],
                status: 'pending'
              }
            }
          }
        },
        {
          ...universalEvent('2', 'evt-assistant'),
          item_id: 'assistant-item',
          event: {
            type: 'content.delta',
            data: { block_id: 'assistant-block', kind: 'text', delta: 'hello' }
          }
        },
        {
          ...universalEvent('3', 'evt-diff'),
          event: {
            type: 'diff.updated',
            data: {
              diff: {
                diff_id: 'd1',
                session_id: 's1',
                title: 'Patch',
                files: [{ path: 'a.ts', status: 'modified', diff: '@@' }]
              }
            }
          }
        }
      ]
    });

    expect(state.chat.items.map((item) => item.id)).toEqual([
      'approval:ap1',
      'agent:assistant-item',
      'event:diff:d1'
    ]);
  });

  test('dual-delivered live question app events still render question cards during universal subscriptions', () => {
    let state = createUniversalClientState();
    state = applyUniversalClientMessage(state, {
      type: 'session_snapshot',
      snapshot: snapshot({ latest_seq: '5' }),
      latest_seq: '5',
      has_more: false,
      events: []
    });
    state = applyUniversalClientMessage(state, {
      type: 'app_event',
      event_id: 'question-live',
      event: {
        type: 'question_requested',
        payload: {
          session_id: 's1',
          question_id: 'q1',
          title: 'Need input',
          fields: [
            {
              id: 'target',
              label: 'Target',
              kind: 'single_select',
              required: true,
              secret: false,
              choices: [{ value: 'web', label: 'Web' }],
              default_answers: []
            }
          ]
        }
      }
    });

    expect(state.chat.items).toMatchObject([
      { id: 'question:q1', kind: 'question', questionId: 'q1', answered: false }
    ]);
  });

  test('legacy question rows survive later universal snapshot rebuilds', () => {
    let state = createUniversalClientState();
    state = applyUniversalClientMessage(state, {
      type: 'session_snapshot',
      snapshot: snapshot({ latest_seq: '5' }),
      latest_seq: '5',
      has_more: false,
      events: []
    });
    state = applyUniversalClientMessage(state, {
      type: 'app_event',
      event_id: 'question-live',
      event: {
        type: 'question_requested',
        payload: {
          session_id: 's1',
          question_id: 'q1',
          title: 'Need input',
          fields: [
            {
              id: 'target',
              label: 'Target',
              kind: 'single_select',
              required: true,
              secret: false,
              choices: [{ value: 'web', label: 'Web' }],
              default_answers: []
            }
          ]
        }
      }
    });
    state = applyUniversalClientMessage(state, {
      type: 'universal_event',
      event_id: '11111111-1111-4111-8111-111111111111',
      seq: '6',
      session_id: 's1',
      ts: '2026-05-03T12:01:00Z',
      source: 'runner',
      event: {
        type: 'content.delta',
        data: { block_id: 'assistant-block', kind: 'text', delta: 'after question' }
      }
    });

    expect(state.chat.items.map((item) => item.id)).toEqual([
      'agent:assistant-block',
      'question:q1'
    ]);
  });

  test('legacy approval resolution patches matching pending universal approval row', () => {
    let state = createUniversalClientState();
    state = applyUniversalClientMessage(state, {
      type: 'session_snapshot',
      snapshot: snapshot({
        latest_seq: '5',
        approvals: {
          ap1: {
            approval_id: 'ap1',
            session_id: 's1',
            kind: 'command',
            title: 'Run command',
            options: [{ option_id: 'approve_once', kind: 'approve_once', label: 'Approve once' }],
            status: 'pending'
          }
        }
      }),
      latest_seq: '5',
      has_more: false,
      events: []
    });

    state = applyUniversalClientMessage(state, {
      type: 'app_event',
      event_id: 'approval-resolved',
      event: {
        type: 'approval_resolved',
        payload: {
          session_id: 's1',
          approval_id: 'ap1',
          decision: { decision: 'accept' }
        }
      }
    });

    expect(state.chat.items).toMatchObject([
      {
        id: 'approval:ap1',
        kind: 'approval',
        resolvedDecision: 'accept',
        resolutionState: undefined
      }
    ]);
    const approval = state.chat.items[0];
    expect(approval.kind).toBe('approval');
    if (approval.kind === 'approval') {
      expect(approval.options).toEqual([]);
    }
  });

  test('legacy question answer patches matching question row and survives universal rebuild', () => {
    let state = createUniversalClientState();
    state = applyUniversalClientMessage(state, {
      type: 'app_event',
      event_id: 'question-live',
      event: {
        type: 'question_requested',
        payload: {
          session_id: 's1',
          question_id: 'q1',
          title: 'Need input',
          fields: [
            {
              id: 'target',
              label: 'Target',
              kind: 'single_select',
              required: true,
              secret: false,
              choices: [{ value: 'web', label: 'Web' }],
              default_answers: []
            }
          ]
        }
      }
    });
    state = applyUniversalClientMessage(state, {
      type: 'app_event',
      event_id: 'question-answered',
      event: {
        type: 'question_answered',
        payload: {
          session_id: 's1',
          question_id: 'q1',
          answer: { question_id: 'q1', answers: { target: ['web'] } }
        }
      }
    });
    state = applyUniversalClientMessage(state, {
      type: 'universal_event',
      event_id: '22222222-2222-4222-8222-222222222222',
      seq: '6',
      session_id: 's1',
      ts: '2026-05-03T12:01:00Z',
      source: 'runner',
      event: {
        type: 'content.delta',
        data: { block_id: 'assistant-block', kind: 'text', delta: 'after answer' }
      }
    });

    expect(state.chat.items.map((item) => item.id)).toEqual([
      'agent:assistant-block',
      'question:q1'
    ]);
    expect(state.chat.items[1]).toMatchObject({
      id: 'question:q1',
      kind: 'question',
      answered: true
    });
  });
});
