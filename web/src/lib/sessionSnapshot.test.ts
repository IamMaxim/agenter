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
    questions: {},
    plans: {},
    diffs: {},
    artifacts: {},
    active_turns: [],
    ...overrides
  };
}

function universalEvent(seq: string, eventId = `evt-${seq}`): UniversalEventEnvelope {
  return {
    protocol_version: 'uap/2',
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
  test('orders snapshot-only structured rows by timestamp without index collisions', () => {
    const rowOrder = new Map([
      ['agent:before', { seq: '10', ts: '2026-05-06T10:00:00Z', index: 0 }],
      ['agent:after', { seq: '20', ts: '2026-05-06T10:10:00Z', index: 1 }]
    ]);

    const state = materializeSnapshotChatState(
      snapshot({
        latest_seq: '21',
        items: {
          before: {
            item_id: 'before',
            session_id: 's1',
            role: 'assistant',
            status: 'completed',
            content: [{ block_id: 'before-text', kind: 'text', text: 'Before' }]
          },
          after: {
            item_id: 'after',
            session_id: 's1',
            role: 'assistant',
            status: 'completed',
            content: [{ block_id: 'after-text', kind: 'text', text: 'After' }]
          }
        },
        questions: {
          q1: {
            question_id: 'q1',
            session_id: 's1',
            title: 'First question',
            fields: [],
            status: 'answered',
            answered_at: '2026-05-06T10:05:00Z'
          },
          q2: {
            question_id: 'q2',
            session_id: 's1',
            title: 'Second question',
            fields: [],
            status: 'answered',
            answered_at: '2026-05-06T10:05:00Z'
          }
        },
        plans: {
          p1: {
            plan_id: 'p1',
            session_id: 's1',
            status: 'draft',
            title: 'Plan',
            content: 'Plan body',
            entries: [],
            artifact_refs: [],
            source: 'native_structured',
            partial: false,
            updated_at: '2026-05-06T10:06:00Z'
          }
        }
      }),
      rowOrder
    );

    expect(state.items.map((item) => item.id)).toEqual([
      'agent:before',
      'question:q1',
      'question:q2',
      'plan:p1',
      'agent:after'
    ]);
    expect(rowOrder.get('question:q1')?.index).not.toBe(rowOrder.get('question:q2')?.index);
    expect(rowOrder.get('plan:p1')?.index).not.toBe(rowOrder.get('question:q2')?.index);
  });

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

  test('materializes plan handoff state from snapshots', () => {
    const state = materializeSnapshotChatState(
      snapshot({
        plans: {
          p1: {
            plan_id: 'p1',
            session_id: 's1',
            status: 'draft',
            title: 'Plan',
            content: 'Do it',
            entries: [],
            artifact_refs: [],
            source: 'native_structured',
            partial: false,
            handoff: {
              state: 'implementing',
              action: 'same_thread',
              updated_at: '2026-05-06T12:00:00Z'
            }
          }
        }
      })
    );

    const plan = state.items.find((item) => item.kind === 'plan');
    expect(plan).toMatchObject({
      kind: 'plan',
      handoff: {
        state: 'implementing',
        action: 'same_thread'
      }
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

  test('uses native Codex user messages without rendering old control-plane duplicates', () => {
    const state = materializeSnapshotChatState(
      snapshot({
        items: {
          echo: {
            item_id: 'echo',
            session_id: 's1',
            role: 'user',
            status: 'completed',
            content: [{ block_id: 'echo-text', kind: 'text', text: 'Implement the plan.' }]
          },
          native: {
            item_id: 'native',
            session_id: 's1',
            turn_id: 't1',
            role: 'user',
            status: 'completed',
            content: [{ block_id: 'native-text', kind: 'text', text: 'Implement the plan.' }],
            native: {
              protocol: 'codex/app-server/v2',
              method: 'item/completed',
              raw_payload: {
                method: 'item/completed',
                params: { item: { type: 'userMessage', id: 'native' } }
              }
            }
          }
        }
      })
    );

    expect(state.items).toMatchObject([
      { id: 'user:echo', kind: 'user', content: 'Implement the plan.' }
    ]);
  });

  test('keeps repeated control-plane user messages while suppressing matching Codex echoes', () => {
    const state = materializeSnapshotChatState(
      snapshot({
        items: {
          echo1: {
            item_id: 'echo1',
            session_id: 's1',
            role: 'user',
            status: 'completed',
            content: [{ block_id: 'echo1-text', kind: 'text', text: 'Again' }]
          },
          native1: {
            item_id: 'native1',
            session_id: 's1',
            role: 'user',
            status: 'completed',
            content: [{ block_id: 'native1-text', kind: 'text', text: 'Again' }],
            native: {
              protocol: 'codex/app-server/v2',
              raw_payload: { params: { item: { type: 'userMessage' } } }
            }
          },
          echo2: {
            item_id: 'echo2',
            session_id: 's1',
            role: 'user',
            status: 'completed',
            content: [{ block_id: 'echo2-text', kind: 'text', text: 'Again' }]
          },
          native2: {
            item_id: 'native2',
            session_id: 's1',
            role: 'user',
            status: 'completed',
            content: [{ block_id: 'native2-text', kind: 'text', text: 'Again' }],
            native: {
              protocol: 'codex/app-server/v2',
              raw_payload: { params: { item: { type: 'userMessage' } } }
            }
          }
        }
      })
    );

    expect(state.items.map((item) => item.id)).toEqual(['user:echo1', 'user:echo2']);
  });

  test('keeps native-only Codex user messages', () => {
    const state = materializeSnapshotChatState(
      snapshot({
        items: {
          native: {
            item_id: 'native',
            session_id: 's1',
            role: 'user',
            status: 'completed',
            content: [{ block_id: 'native-text', kind: 'text', text: 'Native only' }],
            native: {
              protocol: 'codex/app-server/v2',
              raw_payload: { params: { item: { type: 'userMessage' } } }
            }
          }
        }
      })
    );

    expect(state.items).toMatchObject([
      { id: 'user:native', kind: 'user', content: 'Native only' }
    ]);
  });

  test('renders Codex native plan items only through the plan card', () => {
    const planText = '# Scratch-Direction Exercise\n\nUse the scratch workspace.';
    const state = materializeSnapshotChatState(
      snapshot({
        items: {
          planItem: {
            item_id: 'planItem',
            session_id: 's1',
            role: 'assistant',
            status: 'completed',
            content: [{ block_id: 'plan-text', kind: 'text', text: planText }],
            native: {
              protocol: 'codex/app-server/v2',
              raw_payload: { params: { item: { type: 'plan' } } }
            }
          },
          agent: {
            item_id: 'agent',
            session_id: 's1',
            role: 'assistant',
            status: 'completed',
            content: [{ block_id: 'agent-text', kind: 'text', text: planText }],
            native: {
              protocol: 'codex/app-server/v2',
              raw_payload: { params: { item: { type: 'agentMessage' } } }
            }
          }
        },
        plans: {
          p1: {
            plan_id: 'p1',
            session_id: 's1',
            status: 'draft',
            title: 'Codex plan',
            content: planText,
            entries: [],
            artifact_refs: [],
            source: 'native_structured',
            partial: false
          }
        }
      })
    );

    expect(state.items.map((item) => item.id)).toEqual(['agent:agent', 'plan:p1']);
  });

  test('materializes universal_projection terminal items without tool projection as command rows', () => {
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
            native: {
              protocol: 'agenter.native_projection',
              method: 'command_started'
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
        detail: undefined,
        output: 'ok\n',
        status: 'completed',
        source: 'agenter.native_projection'
      }
    ]);
  });

  test('materializes universal_projection fileChange items as file rows with inline diff', () => {
    const state = materializeSnapshotChatState(
      snapshot({
        items: {
          fc1: {
            item_id: 'fc1',
            session_id: 's1',
            role: 'tool',
            status: 'streaming',
            content: [
              { block_id: 'fc1-call', kind: 'tool_call', text: '' },
              {
                block_id: 'fc1-diff',
                kind: 'file_diff',
                text: '@@ -1,2 +1,2 @@\n-line 1\n+line 2\n'
              }
            ],
            native: {
              protocol: 'agenter.native_projection',
              method: 'file_change_proposed'
            }
          }
        }
      })
    );

    expect(state.items).toMatchObject([
      {
        id: 'event:item:fc1',
        kind: 'inlineEvent',
        eventKind: 'file',
        title: 'File change proposed',
        detail: '@@ -1,2 +1,2 @@\n-line 1\n+line 2\n',
        status: 'streaming',
        source: 'agenter.native_projection'
      }
    ]);
  });

  test('materializes universal_projection fileChange rows for broader command-change method families', () => {
    const state = materializeSnapshotChatState(
      snapshot({
        items: {
          fc1: {
            item_id: 'fc1',
            session_id: 's1',
            role: 'tool',
            status: 'completed',
            content: [
              { block_id: 'fc1-call', kind: 'tool_call', text: '' },
              {
                block_id: 'fc1-diff',
                kind: 'file_diff',
                text: '@@ -1,1 +1,1 @@\n-old\n+new\n'
              }
            ],
            native: {
              protocol: 'agenter.native_projection',
              method: 'file_change'
            }
          }
        }
      })
    );

    expect(state.items).toMatchObject([
      {
        id: 'event:item:fc1',
        kind: 'inlineEvent',
        eventKind: 'file',
        title: 'File change',
        detail: '@@ -1,1 +1,1 @@\n-old\n+new\n',
        status: 'completed',
        source: 'agenter.native_projection'
      }
    ]);
  });

  test('materializes universal_projection command rows for broader command method families', () => {
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
            native: {
              protocol: 'agenter.native_projection',
              method: 'command_completed'
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
        detail: undefined,
        output: 'ok\n',
        status: 'completed',
        source: 'agenter.native_projection'
      }
    ]);
  });

  test('applies snapshot first and only replays events after the snapshot cursor', () => {
    let state = createUniversalClientState();
    const message: BrowserServerMessage = {
      type: 'session_snapshot',
      protocol_version: 'uap/2',      snapshot: snapshot({
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
      replay_through_seq: '6',
      replay_complete: true,
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
      protocol_version: 'uap/2',      snapshot: snapshot({ latest_seq: '5' }),
      replay_through_seq: '6',
      replay_complete: true,
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

  test('applies versioned snapshot and live universal event frames', () => {
    let state = createUniversalClientState();
    state = applyUniversalClientMessage(state, {
      type: 'session_snapshot',
      protocol_version: 'uap/2',
      snapshot: snapshot({
        latest_seq: '1',
        items: {
          a1: {
            item_id: 'a1',
            session_id: 's1',
            role: 'assistant',
            status: 'streaming',
            content: [{ block_id: 'assistant-block', kind: 'text', text: 'Hello' }]
          }
        }
      }),
      replay_through_seq: '1',
      replay_complete: true,
      events: []
    });

    state = applyUniversalClientMessage(state, {
      type: 'universal_event',
      ...universalEvent('2', '22222222-2222-4222-8222-222222222222')
    });

    expect(state.latestSeq).toBe('2');
    expect(state.snapshot?.items.a1.content[0].text).toBe('Hello live');
  });

  test('materializes universal assistant and command events instead of native-only rows', () => {
    const state = applyUniversalClientMessage(createUniversalClientState(), {
      type: 'session_snapshot',
      protocol_version: 'uap/2',
      snapshot: snapshot({ latest_seq: '0' }),
      replay_through_seq: '4',
      replay_complete: true,
      events: [
        {
          ...universalEvent('1', 'evt-assistant-completed'),
          item_id: 'assistant-raw-response',
          native: {
            protocol: 'acp',
            method: 'rawResponseItem/completed',
            type: 'qwen',
            summary: 'Assistant message completed'
          },
          event: {
            type: 'content.completed',
            data: {
              block_id: 'acp-text-turn-1',
              kind: 'text',
              text: 'Final answer'
            }
          }
        },
        {
          ...universalEvent('2', 'evt-command-started'),
          item_id: 'command-1',
          native: {
            protocol: 'acp',
            method: 'item/started',
            type: 'qwen',
            summary: 'Item started'
          },
          event: {
            type: 'item.created',
            data: {
              item: {
                item_id: 'command-1',
                session_id: 's1',
                role: 'tool',
                status: 'streaming',
                content: [{ block_id: 'acp-command-cmd-1', kind: 'tool_call', text: 'cargo test' }],
                tool: {
                  kind: 'command',
                  name: 'command',
                  title: 'cargo test',
                  status: 'streaming',
                  command: { command: 'cargo test', actions: [] }
                }
              }
            }
          }
        },
        {
          ...universalEvent('3', 'evt-command-output'),
          item_id: 'command-1',
          native: {
            protocol: 'acp',
            method: 'item/commandExecution/outputDelta',
            type: 'qwen',
            summary: 'Command output'
          },
          event: {
            type: 'content.delta',
            data: {
              block_id: 'acp-command-cmd-1-stdout',
              kind: 'command_output',
              delta: 'ok\n'
            }
          }
        },
        {
          ...universalEvent('4', 'evt-command-completed'),
          item_id: 'command-1',
          native: {
            protocol: 'acp',
            method: 'item/completed',
            type: 'qwen',
            summary: 'Item completed'
          },
          event: {
            type: 'content.completed',
            data: {
              block_id: 'acp-command-cmd-1-status',
              kind: 'command_output',
              text: 'command completed'
            }
          }
        }
      ]
    });

    expect(state.chat.items).toMatchObject([
      { id: 'agent:assistant-raw-response', kind: 'assistant', content: 'Final answer', completed: true },
      {
        id: 'event:item:command-1',
        kind: 'inlineEvent',
        eventKind: 'command',
        title: 'cargo test',
        output: 'ok\n',
        status: 'completed'
      }
    ]);
    expect(state.chat.items.some((item) => item.kind === 'inlineEvent' && item.eventKind === 'event')).toBe(false);
  });

  test('rejects unseen live events at or behind the latest seq cursor', () => {
    let state = createUniversalClientState();
    state = applyUniversalClientMessage(state, {
      type: 'session_snapshot',
      protocol_version: 'uap/2',      snapshot: snapshot({ latest_seq: '6' }),
      replay_through_seq: '6',
      replay_complete: true,
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

  test('applies snapshot checkpoint when replay page is truncated', () => {
    const state = applyUniversalClientMessage(createUniversalClientState(), {
      type: 'session_snapshot',
      protocol_version: 'uap/2',      snapshot: snapshot({
        latest_seq: '5',
        items: {
          a1: {
            item_id: 'a1',
            session_id: 's1',
            role: 'assistant',
            status: 'completed',
            content: [{ block_id: 'assistant-block', kind: 'text', text: 'Snapshot text' }]
          }
        }
      }),
      replay_through_seq: '9',
      replay_complete: false,
      events: [universalEvent('9')]
    });

    expect(state.latestSeq).toBe('5');
    expect(state.snapshotIncomplete).toBe(true);
    expect(state.chat.items).toMatchObject([
      { id: 'agent:a1', kind: 'assistant', content: 'Snapshot text' }
    ]);
  });

  test('keeps raw native payloads in materialized snapshot state', () => {
    const state = applyUniversalClientMessage(createUniversalClientState(), {
      type: 'session_snapshot',
      protocol_version: 'uap/2',
      snapshot: snapshot({
        latest_seq: '7',
        items: {
          item1: {
            item_id: 'item1',
            session_id: 's1',
            role: 'tool',
            status: 'completed',
            content: [],
            tool: {
              kind: 'tool',
              subkind: 'context_compaction',
              name: 'context_compaction',
              title: 'Context compaction',
              status: 'completed'
            },
            native: {
              protocol: 'codex/app-server/v2',
              method: 'rawResponseItem/completed',
              raw_payload: { item: { id: 'item1', type: 'context_compaction' } }
            }
          }
        }
      }),
      events: [],
      replay_complete: true
    });

    expect(state.snapshot!.items.item1.tool?.subkind).toBe('context_compaction');
    expect(state.snapshot!.items.item1.native?.raw_payload).toEqual({
      item: { id: 'item1', type: 'context_compaction' }
    });
  });

  test('keeps raw native payloads on provider rows after snapshot materialization', () => {
    const state = applyUniversalClientMessage(createUniversalClientState(), {
      type: 'session_snapshot',
      protocol_version: 'uap/2',
      snapshot: snapshot({ latest_seq: '1' }),
      events: [
        {
          ...universalEvent('2', 'provider-raw'),
          event_id: 'provider-raw',
          native: {
            protocol: 'codex/app-server/v2',
            method: 'rawResponseItem/completed',
            raw_payload: { params: { item: { id: 'native-1' } } }
          },
          event: {
            type: 'native.unknown',
            data: { summary: 'rawResponseItem/completed' }
          }
        }
      ],
      replay_complete: true
    });

    expect(state.snapshot!.artifacts['native:provider-raw'].native?.raw_payload).toEqual({
      params: { item: { id: 'native-1' } }
    });
    expect(state.chat.items).toEqual([
      expect.objectContaining({
        id: 'event:artifact:native:provider-raw',
        kind: 'inlineEvent',
        rawPayload: { params: { item: { id: 'native-1' } } }
      })
    ]);
  });

  test('materializes raw native payloads and schema metadata on approval and question rows', () => {
    const state = materializeSnapshotChatState(
      snapshot({
        approvals: {
          permission1: {
            approval_id: 'permission1',
            session_id: 's1',
            kind: 'permission',
            title: 'Allow network access',
            details: 'Grant network access for strict auto review.',
            options: [{ option_id: 'accept', kind: 'approve_once', label: 'Allow once' }],
            status: 'pending',
            subject: 'network',
            native_request_id: 'req-permission',
            native_blocking: true,
            native: {
              protocol: 'codex/app-server/v2',
              method: 'item/permissions/requestApproval',
              raw_payload: { method: 'item/permissions/requestApproval', params: { scope: 'network' } }
            }
          }
        },
        questions: {
          q_schema: {
            question_id: 'q_schema',
            session_id: 's1',
            title: 'Pick deployment target',
            description: 'MCP elicitation',
            fields: [
              {
                id: 'target',
                label: 'Target',
                prompt: 'Choose one',
                kind: 'single_select',
                required: true,
                secret: false,
                choices: [{ value: 'prod', label: 'Production' }],
                default_answers: ['prod'],
                schema: { type: 'string', enum: ['prod', 'staging'] }
              }
            ],
            status: 'pending',
            native_request_id: 'req-question',
            native_blocking: true,
            native: {
              protocol: 'codex/app-server/v2',
              method: 'mcpServer/elicitation/request',
              raw_payload: { method: 'mcpServer/elicitation/request', params: { schema: true } }
            }
          }
        }
      })
    );

    expect(state.items).toEqual([
      expect.objectContaining({
        id: 'approval:permission1',
        kind: 'approval',
        approvalKind: 'permission',
        rawPayload: { method: 'item/permissions/requestApproval', params: { scope: 'network' } }
      }),
      expect.objectContaining({
        id: 'question:q_schema',
        kind: 'question',
        nativeRequestId: 'req-question',
        nativeBlocking: true,
        rawPayload: { method: 'mcpServer/elicitation/request', params: { schema: true } },
        fields: [
          expect.objectContaining({
            id: 'target',
            schema: { type: 'string', enum: ['prod', 'staging'] },
            default_answers: ['prod']
          })
        ]
      })
    ]);
  });

  test('materializes tool subkinds and item raw payloads from universal rows', () => {
    const state = materializeSnapshotChatState(
      snapshot({
        items: {
          search: {
            item_id: 'search',
            session_id: 's1',
            role: 'tool',
            status: 'completed',
            content: [{ block_id: 'search-result', kind: 'tool_result', text: 'Found docs' }],
            tool: {
              kind: 'tool',
              subkind: 'web_search',
              name: 'web_search',
              title: 'Search web',
              status: 'completed',
              output_summary: 'Found docs'
            },
            native: {
              protocol: 'codex/app-server/v2',
              method: 'item/completed',
              raw_payload: { item: { type: 'web_search', query: 'uap2' } }
            }
          },
          compaction: {
            item_id: 'compaction',
            session_id: 's1',
            role: 'system',
            status: 'completed',
            content: [{ block_id: 'compact', kind: 'provider_status', text: 'Context compacted' }],
            tool: {
              kind: 'tool',
              subkind: 'context_compaction',
              name: 'context_compaction',
              title: 'Context compaction',
              status: 'completed'
            },
            native: {
              protocol: 'codex/app-server/v2',
              method: 'thread/compact/start',
              raw_payload: { method: 'thread/compact/start' }
            }
          },
          review: {
            item_id: 'review',
            session_id: 's1',
            role: 'system',
            status: 'completed',
            content: [{ block_id: 'review-mode', kind: 'provider_status', text: 'Entered review mode' }],
            tool: {
              kind: 'tool',
              subkind: 'review_mode',
              name: 'review_mode',
              title: 'Review mode',
              status: 'completed'
            },
            native: {
              protocol: 'codex/app-server/v2',
              method: 'review/start',
              raw_payload: { method: 'review/start' }
            }
          }
        }
      })
    );

    expect(state.items).toEqual([
      expect.objectContaining({
        id: 'event:item:search',
        kind: 'inlineEvent',
        eventKind: 'tool',
        subkind: 'web_search',
        rawPayload: { item: { type: 'web_search', query: 'uap2' } }
      }),
      expect.objectContaining({
        id: 'event:item:compaction',
        kind: 'inlineEvent',
        eventKind: 'tool',
        subkind: 'context_compaction',
        rawPayload: { method: 'thread/compact/start' }
      }),
      expect.objectContaining({
        id: 'event:item:review',
        kind: 'inlineEvent',
        eventKind: 'tool',
        subkind: 'review_mode',
        rawPayload: { method: 'review/start' }
      })
    ]);
  });

  test('detects real capability data before feature gating existing controls', () => {
    expect(hasCapabilitySignal(undefined)).toBe(false);
    expect(hasCapabilitySignal(snapshot({ capabilities: undefined }).capabilities)).toBe(false);
    expect(hasCapabilitySignal(baseCapabilities)).toBe(true);
    expect(
      hasCapabilitySignal({
        protocol: {},
        content: {},
        tools: {},
        approvals: {},
        plan: {},
        modes: {},
        integration: {},
        provider_details: [
          {
            key: 'dynamic_tools',
            status: 'degraded',
            methods: ['item/tool/call'],
            reason: null
          }
        ]
      })
    ).toBe(false);
    expect(
      hasCapabilitySignal({
        protocol: {},
        content: {},
        tools: {},
        approvals: {},
        plan: {},
        modes: {},
        integration: {},
        provider_details: [
          {
            key: 'realtime',
            status: 'unsupported',
            methods: ['thread/realtime/started'],
            reason: null
          }
        ]
      })
    ).toBe(false);
    expect(
      hasCapabilitySignal({
        protocol: 'bad',
        provider_details: 'bad'
      } as unknown as CapabilitySet)
    ).toBe(false);
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

  test('renders terminal question snapshot states after orphan or detach', () => {
    const state = applyUniversalClientMessage(createUniversalClientState(), {
      type: 'session_snapshot',
      protocol_version: 'uap/2',      snapshot: snapshot({
        latest_seq: null,
        questions: {
          orphaned: {
            question_id: 'orphaned',
            session_id: 's1',
            title: 'Pick target',
            description: 'Provider exited',
            fields: [],
            status: 'orphaned'
          },
          detached: {
            question_id: 'detached',
            session_id: 's1',
            title: 'Clarify input',
            fields: [],
            status: 'detached'
          }
        }
      }),
      events: [],
      replay_through_seq: null,
      replay_complete: true
    });

    expect(state.chat.items).toMatchObject([
      { id: 'question:orphaned', kind: 'question', answered: false, status: 'orphaned', resolvedState: 'orphaned' },
      { id: 'question:detached', kind: 'question', answered: false, status: 'detached', resolvedState: 'detached' }
    ]);
  });

  test('preserves replay chronology for interleaved assistant approval and diff rows', () => {
    let state = createUniversalClientState();
    state = applyUniversalClientMessage(state, {
      type: 'session_snapshot',
      protocol_version: 'uap/2',      snapshot: snapshot({ latest_seq: '0' }),
      replay_through_seq: '3',
      replay_complete: true,
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
      protocol_version: 'uap/2',      snapshot: snapshot({
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
      replay_through_seq: '3',
      replay_complete: true,
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
      protocol_version: 'uap/2',      snapshot: snapshot({
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
      replay_through_seq: '3',
      replay_complete: true,
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

  test('uses bounded replay order for approvals in incomplete snapshot messages', () => {
    let state = createUniversalClientState();
    state = applyUniversalClientMessage(state, {
      type: 'session_snapshot',
      protocol_version: 'uap/2',      snapshot: snapshot({
        latest_seq: '5',
        items: {
          'assistant-before': {
            item_id: 'assistant-before',
            session_id: 's1',
            role: 'assistant',
            status: 'completed',
            content: [{ block_id: 'assistant-before-block', kind: 'text', text: 'before' }]
          },
          'assistant-after': {
            item_id: 'assistant-after',
            session_id: 's1',
            role: 'assistant',
            status: 'completed',
            content: [{ block_id: 'assistant-after-block', kind: 'text', text: 'after' }]
          }
        },
        approvals: {
          ap1: {
            approval_id: 'ap1',
            session_id: 's1',
            kind: 'command',
            title: 'Run command',
            options: [{ option_id: 'approve_once', kind: 'approve_once', label: 'Approve once' }],
            status: 'approved'
          }
        }
      }),
      replay_through_seq: '3',
      replay_complete: false,
      events: [
        {
          ...universalEvent('1', 'evt-assistant-before'),
          item_id: 'assistant-before',
          event: {
            type: 'content.delta',
            data: { block_id: 'assistant-before-block', kind: 'text', delta: 'before' }
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
                status: 'approved'
              }
            }
          }
        },
        {
          ...universalEvent('3', 'evt-assistant-after'),
          item_id: 'assistant-after',
          event: {
            type: 'content.delta',
            data: { block_id: 'assistant-after-block', kind: 'text', delta: 'after' }
          }
        }
      ]
    });

    expect(state.snapshotIncomplete).toBe(true);
    expect(state.chat.items.map((item) => item.id)).toEqual([
      'agent:assistant-before',
      'approval:ap1',
      'agent:assistant-after'
    ]);
    expect(state.rowOrder.get('approval:ap1')?.seq).toBe('2');
  });

  test('reanchors resolved approvals by request time when bounded replay only has resolution event', () => {
    let state = createUniversalClientState();
    state = applyUniversalClientMessage(state, {
      type: 'session_snapshot',
      protocol_version: 'uap/2',      snapshot: snapshot({
        latest_seq: '10',
        items: {
          'assistant-before': {
            item_id: 'assistant-before',
            session_id: 's1',
            role: 'assistant',
            status: 'completed',
            content: [{ block_id: 'assistant-before-block', kind: 'text', text: 'before' }]
          },
          'assistant-after': {
            item_id: 'assistant-after',
            session_id: 's1',
            role: 'assistant',
            status: 'completed',
            content: [{ block_id: 'assistant-after-block', kind: 'text', text: 'after' }]
          }
        },
        approvals: {
          ap1: {
            approval_id: 'ap1',
            session_id: 's1',
            kind: 'command',
            title: 'Run command',
            options: [{ option_id: 'approve_once', kind: 'approve_once', label: 'Approve once' }],
            status: 'approved',
            requested_at: '2026-05-04T12:00:02Z',
            resolved_at: '2026-05-04T12:00:10Z'
          }
        }
      }),
      replay_through_seq: '10',
      replay_complete: false,
      events: [
        {
          ...universalEvent('1', 'evt-assistant-before'),
          ts: '2026-05-04T12:00:01Z',
          item_id: 'assistant-before',
          event: {
            type: 'content.delta',
            data: { block_id: 'assistant-before-block', kind: 'text', delta: 'before' }
          }
        },
        {
          ...universalEvent('3', 'evt-assistant-after'),
          ts: '2026-05-04T12:00:03Z',
          item_id: 'assistant-after',
          event: {
            type: 'content.delta',
            data: { block_id: 'assistant-after-block', kind: 'text', delta: 'after' }
          }
        },
        {
          ...universalEvent('10', 'evt-approval-resolved'),
          ts: '2026-05-04T12:00:10Z',
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
        }
      ]
    });

    expect(state.snapshotIncomplete).toBe(true);
    expect(state.chat.items.map((item) => item.id)).toEqual([
      'agent:assistant-before',
      'approval:ap1',
      'agent:assistant-after'
    ]);
    expect(state.rowOrder.get('approval:ap1')).toMatchObject({
      seq: '3',
      ts: '2026-05-04T12:00:02Z'
    });
  });

  test('orders snapshot-only approval and question rows by request timestamp', () => {
    const state = materializeSnapshotChatState(
      snapshot({
        latest_seq: '8',
        approvals: {
          ap1: {
            approval_id: 'ap1',
            session_id: 's1',
            kind: 'command',
            title: 'Run command',
            options: [{ option_id: 'approve_once', kind: 'approve_once', label: 'Approve once' }],
            status: 'approved',
            requested_at: '2026-05-04T12:00:03Z',
            resolved_at: '2026-05-04T12:00:05Z'
          }
        },
        questions: {
          q1: {
            question_id: 'q1',
            session_id: 's1',
            title: 'Pick target',
            fields: [],
            status: 'answered',
            requested_at: '2026-05-04T12:00:01Z',
            answered_at: '2026-05-04T12:00:04Z'
          }
        }
      })
    );

    expect(state.items.map((item) => item.id)).toEqual(['question:q1', 'approval:ap1']);
  });

  test('places snapshot-only rows before replayed rows when synthetic ordering is needed', () => {
    let state = createUniversalClientState();
    state = applyUniversalClientMessage(state, {
      type: 'session_snapshot',
      protocol_version: 'uap/2',      snapshot: snapshot({
        latest_seq: '3',
        plans: {
          p1: {
            plan_id: 'p1',
            session_id: 's1',
            turn_id: 't1',
            status: 'implementing',
            title: 'Implementation plan',
            content: 'Use this plan',
            entries: [],
            artifact_refs: [],
            source: 'native_structured',
            partial: false
          }
        }
      }),
      replay_through_seq: '5',
      replay_complete: true,
      events: [
        {
          ...universalEvent('5', 'evt-assistant'),
          item_id: 'assistant-item',
          event: {
            type: 'content.delta',
            data: { block_id: 'assistant-block', kind: 'text', delta: 'hello' }
          }
        }
      ]
    });

    expect(state.chat.items.map((item) => item.id)).toEqual([
      'plan:p1',
      'agent:assistant-item'
    ]);
    expect(state.rowOrder.get('plan:p1')?.seq).toBe('4');
    expect(state.rowOrder.get('agent:assistant-item')?.seq).toBe('5');
  });

  test('renders provider error events as expandable error rows', () => {
    let state = createUniversalClientState();
    state = applyUniversalClientMessage(state, {
      type: 'universal_event',
      ...universalEvent('6', 'evt-capability-gap'),
      native: {
        protocol: 'acp',
        method: 'item/tool/call',
        type: 'qwen',
        native_id: null,
        summary: 'error reported',
        hash: null,
        pointer: null
      },
      event: {
        type: 'error.reported',
        data: {
          code: 'provider_capability_gap',
          message: 'Provider server request `item/tool/call` is classified but not supported by Agenter.'
        }
      }
    });

    expect(state.chat.items).toMatchObject([
      {
        id: 'event:artifact:error:evt-capability-gap',
        kind: 'error',
        title: 'Provider capability gap',
        detail: 'Provider server request `item/tool/call` is classified but not supported by Agenter.'
      }
    ]);
  });

  test('renders promoted native provider notifications and hides raw native noise outside debug', () => {
    let state = createUniversalClientState();
    state = applyUniversalClientMessage(state, {
      type: 'universal_event',
      ...universalEvent('7', 'evt-guardian'),
      native: {
        protocol: 'acp',
        method: 'guardianWarning',
        type: 'qwen',
        native_id: null,
        summary: 'native notification',
        hash: null,
        pointer: null
      },
      event: {
        type: 'native.unknown',
        data: { summary: 'native notification' }
      }
    });
    state = applyUniversalClientMessage(state, {
      type: 'universal_event',
      ...universalEvent('8', 'evt-raw-native'),
      native: {
        protocol: 'acp',
        method: 'raw/unclassified',
        type: 'qwen',
        native_id: null,
        summary: 'native notification',
        hash: null,
        pointer: null
      },
      event: {
        type: 'native.unknown',
        data: { summary: 'native notification' }
      }
    });

    expect(state.chat.items).toMatchObject([
      {
        id: 'event:artifact:native:evt-guardian',
        kind: 'inlineEvent',
        eventKind: 'event',
        displayLevel: 'normal',
        title: 'Guardian warning',
        status: 'native'
      },
      {
        id: 'event:artifact:native:evt-raw-native',
        kind: 'inlineEvent',
        eventKind: 'event',
        displayLevel: 'raw',
        title: 'native notification',
        status: 'native'
      }
    ]);
  });

  test('renders provider notifications without native unknown fallback', () => {
    let state = createUniversalClientState();
    state = applyUniversalClientMessage(state, {
      type: 'universal_event',
      ...universalEvent('9', 'evt-hook'),
      native: {
        protocol: 'acp',
        method: 'hook/started',
        type: 'qwen',
        native_id: 'hook-1',
        summary: 'Provider notification',
        hash: null,
        pointer: null
      },
      event: {
        type: 'provider.notification',
        data: {
          notification: {
            category: 'hook',
            title: 'Hook started',
            detail: null,
            status: 'started',
            severity: 'info',
            subject: 'hook-1'
          }
        }
      }
    });

    expect(state.chat.items).toMatchObject([
      {
        id: 'event:artifact:provider:evt-hook',
        kind: 'inlineEvent',
        eventKind: 'event',
        displayLevel: 'normal',
        title: 'Hook started',
        detail: 'status: started\nsubject: hook-1',
        status: 'native'
      }
    ]);
  });

  test('keeps latest usage from live universal usage events without session info', () => {
    let state = createUniversalClientState();
    state = applyUniversalClientMessage(state, {
      type: 'universal_event',
      ...universalEvent('10', 'evt-usage'),
      event: {
        type: 'usage.updated',
        data: {
          usage: {
            context: { used_percent: 52, used_tokens: 52000, total_tokens: 100000 },
            window_5h: { remaining_percent: 21 },
            week: { remaining_percent: 71 }
          }
        }
      }
    });

    expect(state.latestUsage).toMatchObject({
      context: { used_percent: 52, used_tokens: 52000, total_tokens: 100000 },
      window_5h: { remaining_percent: 21 },
      week: { remaining_percent: 71 }
    });
    expect(state.snapshot?.info?.usage).toMatchObject({
      context: { used_percent: 52 }
    });
  });

  test('orders reducer-created error provider and native rows by event sequence', () => {
    let state = createUniversalClientState();
    state = applyUniversalClientMessage(state, {
      type: 'universal_event',
      ...universalEvent('11', 'evt-provider-order'),
      event: {
        type: 'provider.notification',
        data: { notification: { category: 'hook', title: 'Hook completed', status: 'completed' } }
      }
    });
    state = applyUniversalClientMessage(state, {
      type: 'universal_event',
      ...universalEvent('12', 'evt-error-order'),
      event: {
        type: 'error.reported',
        data: { code: 'provider_error', message: 'Provider failed' }
      }
    });
    state = applyUniversalClientMessage(state, {
      type: 'universal_event',
      ...universalEvent('13', 'evt-native-order'),
      event: {
        type: 'native.unknown',
        data: { summary: 'raw provider payload' }
      }
    });

    expect(state.chat.items.map((item) => item.id)).toEqual([
      'event:artifact:provider:evt-provider-order',
      'event:artifact:error:evt-error-order',
      'event:artifact:native:evt-native-order'
    ]);
    expect(state.rowOrder.get('event:artifact:provider:evt-provider-order')?.seq).toBe('11');
    expect(state.rowOrder.get('event:artifact:error:evt-error-order')?.seq).toBe('12');
    expect(state.rowOrder.get('event:artifact:native:evt-native-order')?.seq).toBe('13');
  });

  test('orders command content deltas by event item row instead of assistant fallback row', () => {
    let state = createUniversalClientState();
    state = applyUniversalClientMessage(state, {
      type: 'universal_event',
      ...universalEvent('14', 'evt-command-output'),
      item_id: 'cmd-live',
      event: {
        type: 'content.delta',
        data: { block_id: 'cmd-live-stdout', kind: 'command_output', delta: 'ok\n' }
      }
    });

    expect(state.chat.items).toMatchObject([
      {
        id: 'event:item:cmd-live',
        kind: 'inlineEvent',
        eventKind: 'command',
        output: 'ok\n'
      }
    ]);
    expect(state.rowOrder.get('event:item:cmd-live')?.seq).toBe('14');
    expect(state.rowOrder.has('agent:cmd-live')).toBe(false);
  });

  test('materializes all universal content block kinds with deliberate row policies', () => {
    const state = materializeSnapshotChatState(
      snapshot({
        items: {
          reason: {
            item_id: 'reason',
            session_id: 's1',
            role: 'assistant',
            status: 'streaming',
            content: [{ block_id: 'reason-block', kind: 'reasoning', text: 'Checking constraints' }]
          },
          terminal: {
            item_id: 'terminal',
            session_id: 's1',
            role: 'tool',
            status: 'completed',
            content: [
              { block_id: 'stdin', kind: 'terminal_input', text: 'yes\n' },
              { block_id: 'stdout', kind: 'command_output', text: 'accepted\n' }
            ]
          },
          warning: {
            item_id: 'warning',
            session_id: 's1',
            role: 'system',
            status: 'completed',
            content: [{ block_id: 'warning-block', kind: 'warning', text: 'Sandbox degraded' }]
          },
          provider: {
            item_id: 'provider',
            session_id: 's1',
            role: 'system',
            status: 'completed',
            content: [{ block_id: 'provider-block', kind: 'provider_status', text: 'MCP connected' }]
          },
          image: {
            item_id: 'image',
            session_id: 's1',
            role: 'assistant',
            status: 'completed',
            content: [{ block_id: 'image-block', kind: 'image', text: 'Screenshot', artifact_id: 'artifact-image' }]
          },
          native: {
            item_id: 'native',
            session_id: 's1',
            role: 'system',
            status: 'completed',
            content: [{ block_id: 'native-block', kind: 'native', text: 'Raw detail' }]
          }
        }
      })
    );

    expect(state.items).toMatchObject([
      {
        id: 'event:item:reason',
        kind: 'inlineEvent',
        eventKind: 'tool',
        displayLevel: 'thinking',
        title: 'Reasoning',
        detail: 'Checking constraints'
      },
      {
        id: 'event:item:terminal',
        kind: 'inlineEvent',
        eventKind: 'command',
        output: '$ yes\naccepted\n'
      },
      {
        id: 'event:item:warning',
        kind: 'inlineEvent',
        eventKind: 'event',
        title: 'Warning',
        detail: 'Sandbox degraded'
      },
      {
        id: 'event:item:provider',
        kind: 'inlineEvent',
        eventKind: 'event',
        title: 'Provider status',
        detail: 'MCP connected'
      },
      {
        id: 'event:item:image',
        kind: 'inlineEvent',
        eventKind: 'event',
        title: 'Image',
        detail: 'artifact-image\nScreenshot'
      },
      {
        id: 'event:item:native',
        kind: 'inlineEvent',
        eventKind: 'event',
        displayLevel: 'raw',
        title: 'Native event',
        detail: 'Raw detail'
      }
    ]);
  });

});
