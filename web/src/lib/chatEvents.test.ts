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
        markdown: true,
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

  test('maps multi-select question requests and answered events', () => {
    let state = createChatState();
    state = applyChatEnvelope(state, {
      type: 'app_event',
      event_id: 'evt-question',
      event: {
        type: 'question_requested',
        payload: {
          session_id: 's1',
          question_id: 'q1',
          title: 'Need input',
          description: 'Pick targets',
          fields: [
            {
              id: 'targets',
              label: 'Targets',
              prompt: 'Which targets?',
              kind: 'multi_select',
              required: true,
              secret: false,
              choices: [
                { value: 'web', label: 'Web' },
                { value: 'runner', label: 'Runner' }
              ],
              default_answers: ['web']
            }
          ]
        }
      }
    });

    expect(state.items[0]).toMatchObject({
      id: 'question:q1',
      kind: 'question',
      questionId: 'q1',
      answered: false,
      fields: [{ id: 'targets', kind: 'multi_select' }]
    });

    state = applyChatEnvelope(state, {
      type: 'app_event',
      event_id: 'evt-question-answered',
      event: {
        type: 'question_answered',
        payload: {
          session_id: 's1',
          question_id: 'q1',
          answer: { question_id: 'q1', answers: { targets: ['web', 'runner'] } }
        }
      }
    });

    expect(state.items[0]).toMatchObject({
      id: 'question:q1',
      kind: 'question',
      answered: true
    });
  });

  test('keeps user and assistant messages as markdown-capable transcript items', () => {
    let state = createChatState();

    state = applyChatEnvelope(state, {
      type: 'app_event',
      event_id: 'evt-user-markdown',
      event: {
        type: 'user_message',
        payload: {
          session_id: 's1',
          message_id: 'u1',
          content: 'Please run **frontend verification**.'
        }
      }
    });
    state = applyChatEnvelope(state, {
      type: 'app_event',
      event_id: 'evt-agent-markdown',
      event: {
        type: 'agent_message_completed',
        payload: {
          session_id: 's1',
          message_id: 'a1',
          content: '- Added markdown\n- Updated event rows'
        }
      }
    });

    expect(state.items).toEqual([
      {
        id: 'user:u1',
        kind: 'user',
        messageId: 'u1',
        content: 'Please run **frontend verification**.',
        markdown: true
      },
      {
        id: 'agent:a1',
        kind: 'assistant',
        messageId: 'a1',
        content: '- Added markdown\n- Updated event rows',
        completed: true,
        markdown: true
      }
    ]);
  });

  test('maps command tool and file activity to inline expandable event rows', () => {
    let state = createChatState();

    state = applyChatEnvelope(state, {
      type: 'app_event',
      event_id: 'evt-command-start',
      event: {
        type: 'command_started',
        payload: {
          session_id: 's1',
          command_id: 'cmd1',
          command: 'npm run check',
          cwd: 'web/'
        }
      }
    });
    state = applyChatEnvelope(state, {
      type: 'app_event',
      event_id: 'evt-tool',
      event: {
        type: 'tool_started',
        payload: {
          session_id: 's1',
          tool_call_id: 'tool1',
          name: 'codex_item',
          input: { item_id: 'i1' }
        }
      }
    });
    state = applyChatEnvelope(state, {
      type: 'app_event',
      event_id: 'evt-file',
      event: {
        type: 'file_change_applied',
        payload: {
          session_id: 's1',
          path: 'web/src/App.svelte',
          diff: '+ dark shell'
        }
      }
    });

    expect(state.items).toEqual([
      {
        id: 'event:command:cmd1',
        kind: 'inlineEvent',
        eventKind: 'command',
        title: 'npm run check',
        detail: 'web/',
        output: '',
        status: 'running',
        success: undefined,
        processId: undefined,
        source: undefined,
        actions: []
      },
      {
        id: 'event:tool:tool1',
        kind: 'inlineEvent',
        eventKind: 'tool',
        title: 'codex_item',
        detail: '{\n  "item_id": "i1"\n}',
        status: 'running'
      },
      {
        id: 'event:file:web/src/App.svelte',
        kind: 'inlineEvent',
        eventKind: 'file',
        title: 'web/src/App.svelte',
        detail: '+ dark shell',
        status: 'applied'
      }
    ]);
  });

  test('renders plan updates as dedicated plan cards', () => {
    let state = createChatState();

    state = applyChatEnvelope(state, {
      type: 'app_event',
      event_id: 'evt-plan',
      event: {
        type: 'plan_updated',
        payload: {
          session_id: 's1',
          title: 'Implementation plan',
          content: '1. Add markdown\n2. Restyle chat'
        }
      }
    });

    expect(state.items).toEqual([
      {
        id: 'plan:evt-plan',
        kind: 'plan',
        title: 'Implementation plan',
        content: '1. Add markdown\n2. Restyle chat'
      }
    ]);
  });

  test('appends streamed plan deltas and lets full snapshots replace content', () => {
    let state = createChatState();

    state = applyChatEnvelope(state, {
      type: 'app_event',
      event_id: 'evt-plan-delta-1',
      event: {
        type: 'plan_updated',
        payload: {
          session_id: 's1',
          plan_id: 'plan-1',
          title: 'Implementation plan',
          content: '1. Add '
        }
      }
    });
    state = applyChatEnvelope(state, {
      type: 'app_event',
      event_id: 'evt-plan-delta-2',
      event: {
        type: 'plan_updated',
        payload: {
          session_id: 's1',
          plan_id: 'plan-1',
          content: 'tests',
          append: true
        }
      }
    });

    expect(state.items[0]).toMatchObject({
      id: 'plan:plan-1',
      kind: 'plan',
      content: '1. Add tests'
    });

    state = applyChatEnvelope(state, {
      type: 'app_event',
      event_id: 'evt-plan-snapshot',
      event: {
        type: 'plan_updated',
        payload: {
          session_id: 's1',
          plan_id: 'plan-1',
          title: 'Implementation plan',
          content: '1. Final snapshot'
        }
      }
    });

    expect(state.items[0]).toMatchObject({
      id: 'plan:plan-1',
      kind: 'plan',
      content: '1. Final snapshot'
    });
  });

  test('maps session status changes to visible rows and activity state', () => {
    let state = createChatState();

    state = applyChatEnvelope(state, {
      type: 'app_event',
      event_id: 'evt-running',
      event: {
        type: 'session_status_changed',
        payload: {
          session_id: 's1',
          status: 'running',
          reason: 'Turn started.'
        }
      }
    });

    expect(state.activity).toEqual({ status: 'running', active: true, label: 'Working' });
    expect(state.items[0]).toMatchObject({
      id: 'status:evt-running',
      kind: 'inlineEvent',
      eventKind: 'event',
      title: 'Working',
      detail: 'Turn started.',
      status: 'running'
    });

    state = applyChatEnvelope(state, {
      type: 'app_event',
      event_id: 'evt-completed',
      event: {
        type: 'session_status_changed',
        payload: {
          session_id: 's1',
          status: 'completed'
        }
      }
    });

    expect(state.activity).toEqual({ status: 'completed', active: false, label: 'Turn complete' });
  });

  test('renders usage rate limit slash and compaction provider events compactly', () => {
    let state = createChatState();

    for (const [event_id, payload] of [
      [
        'evt-token',
        {
          session_id: 's1',
          provider_id: 'codex',
          event_id: 'turn-1',
          category: 'token_usage',
          title: 'Token usage updated',
          detail: 'last 10 · total 100 · window 1000',
          status: 'updated'
        }
      ],
      [
        'evt-rate',
        {
          session_id: 's1',
          provider_id: 'codex',
          category: 'rate_limits',
          title: 'Rate limits updated',
          detail: 'prolite · primary 57%',
          status: 'updated'
        }
      ],
      [
        'evt-slash',
        {
          session_id: 's1',
          provider_id: 'codex',
          category: 'slash_command',
          title: '/compact',
          detail: 'Codex compaction started.',
          status: 'accepted'
        }
      ],
      [
        'evt-compact',
        {
          session_id: 's1',
          provider_id: 'codex',
          event_id: 'item-237',
          category: 'compaction',
          title: 'Context compacted',
          status: 'completed'
        }
      ]
    ] as const) {
      state = applyChatEnvelope(state, {
        type: 'app_event',
        event_id,
        event: {
          type: 'provider_event',
          payload
        }
      });
    }

    expect(state.items).toMatchObject([
      { kind: 'inlineEvent', title: 'Token usage updated', status: 'updated' },
      { kind: 'inlineEvent', title: 'Rate limits updated', status: 'updated' },
      { kind: 'inlineEvent', title: '/compact', status: 'accepted' },
      { kind: 'inlineEvent', title: 'Context compacted', status: 'completed' }
    ]);
  });

  test('preserves command title while streaming output and exposes command metadata', () => {
    let state = createChatState();

    state = applyChatEnvelope(state, {
      type: 'app_event',
      event_id: 'evt-command-start',
      event: {
        type: 'command_started',
        payload: {
          session_id: 's1',
          command_id: 'cmd1',
          command: "sed -n '1,20p' SKILL.md",
          cwd: '/work/agenter',
          source: 'unifiedExecStartup',
          process_id: '123',
          actions: [
            {
              kind: 'read',
              command: "sed -n '1,20p' /tmp/skills/demo/SKILL.md",
              name: 'SKILL.md',
              path: '/tmp/skills/demo/SKILL.md'
            }
          ]
        }
      }
    });
    state = applyChatEnvelope(state, {
      type: 'app_event',
      event_id: 'evt-command-output',
      event: {
        type: 'command_output_delta',
        payload: {
          session_id: 's1',
          command_id: 'cmd1',
          delta: 'hello\n'
        }
      }
    });
    state = applyChatEnvelope(state, {
      type: 'app_event',
      event_id: 'evt-command-completed',
      event: {
        type: 'command_completed',
        payload: {
          session_id: 's1',
          command_id: 'cmd1',
          exit_code: 0,
          duration_ms: 17,
          success: true
        }
      }
    });

    expect(state.items[0]).toMatchObject({
      id: 'event:command:cmd1',
      kind: 'inlineEvent',
      eventKind: 'command',
      title: "sed -n '1,20p' SKILL.md",
      detail: '/work/agenter · unifiedExecStartup · pid 123',
      output: 'hello\n',
      status: 'completed',
      exitCode: 0,
      durationMs: 17,
      processId: '123',
      source: 'unifiedExecStartup',
      actions: [
        {
          kind: 'skill',
          label: 'Skill: demo',
          detail: '/tmp/skills/demo/SKILL.md',
          path: '/tmp/skills/demo/SKILL.md'
        }
      ]
    });
  });

  test('maps codex spawn agent tools to structured subagent rows', () => {
    let state = createChatState();

    state = applyChatEnvelope(state, {
      type: 'app_event',
      event_id: 'evt-spawn',
      event: {
        type: 'tool_completed',
        payload: {
          session_id: 's1',
          tool_call_id: 'tool1',
          name: 'spawnAgent',
          provider_payload: {
            type: 'collabAgentToolCall',
            tool: 'spawnAgent',
            receiverThreadIds: ['agent-1'],
            model: 'gpt-5.5',
            reasoningEffort: 'medium',
            prompt: 'Implement task'
          }
        }
      }
    });

    expect(state.items[0]).toMatchObject({
      id: 'subagent:tool1',
      kind: 'subagent',
      operation: 'spawn',
      title: 'Spawn subagent',
      status: 'completed',
      agentIds: ['agent-1'],
      model: 'gpt-5.5',
      reasoningEffort: 'medium',
      prompt: 'Implement task'
    });
  });

  test('maps codex wait and close tools to subagent result rows', () => {
    let state = createChatState();

    state = applyChatEnvelope(state, {
      type: 'app_event',
      event_id: 'evt-wait',
      event: {
        type: 'tool_completed',
        payload: {
          session_id: 's1',
          tool_call_id: 'tool-wait',
          name: 'wait',
          provider_payload: {
            type: 'collabAgentToolCall',
            tool: 'wait',
            receiverThreadIds: ['agent-1'],
            agentsStates: {
              'agent-1': {
                status: 'completed',
                message: 'DONE\n\nVerification passed.'
              }
            }
          }
        }
      }
    });
    state = applyChatEnvelope(state, {
      type: 'app_event',
      event_id: 'evt-close',
      event: {
        type: 'tool_completed',
        payload: {
          session_id: 's1',
          tool_call_id: 'tool-close',
          name: 'closeAgent',
          provider_payload: {
            type: 'collabAgentToolCall',
            tool: 'closeAgent',
            receiverThreadIds: ['agent-2'],
            agentsStates: {
              'agent-2': {
                status: 'completed',
                message: 'APPROVED'
              }
            }
          }
        }
      }
    });

    expect(state.items[0]).toMatchObject({
      id: 'subagent:tool-wait',
      kind: 'subagent',
      operation: 'wait',
      title: 'Wait for subagent',
      agentIds: ['agent-1'],
      states: [
        {
          agentId: 'agent-1',
          status: 'completed',
          message: 'DONE\n\nVerification passed.'
        }
      ]
    });
    expect(state.items[1]).toMatchObject({
      id: 'subagent:tool-close',
      kind: 'subagent',
      operation: 'close',
      title: 'Close subagent',
      agentIds: ['agent-2'],
      states: [
        {
          agentId: 'agent-2',
          status: 'completed',
          message: 'APPROVED'
        }
      ]
    });
  });

  test('keeps empty codex wait tools as harmless subagent lifecycle rows', () => {
    let state = createChatState();

    state = applyChatEnvelope(state, {
      type: 'app_event',
      event_id: 'evt-empty-wait',
      event: {
        type: 'tool_completed',
        payload: {
          session_id: 's1',
          tool_call_id: 'tool-empty-wait',
          name: 'wait',
          provider_payload: {
            type: 'collabAgentToolCall',
            tool: 'wait',
            receiverThreadIds: [],
            agentsStates: {}
          }
        }
      }
    });

    expect(state.items[0]).toMatchObject({
      id: 'subagent:tool-empty-wait',
      kind: 'subagent',
      operation: 'wait',
      title: 'Wait for subagent',
      agentIds: [],
      states: []
    });
  });

  test('renders provider events as compact inline rows', () => {
    let state = createChatState();

    state = applyChatEnvelope(state, {
      type: 'app_event',
      event_id: 'evt-provider-compact',
      event: {
        type: 'provider_event',
        payload: {
          session_id: 's1',
          provider_id: 'codex',
          event_id: 'compact-1',
          category: 'compaction',
          title: 'Context compacted',
          detail: 'Codex compacted the active thread context',
          status: 'completed'
        }
      }
    });

    expect(state.items).toEqual([
      {
        id: 'event:provider:compact-1',
        kind: 'inlineEvent',
        eventKind: 'event',
        title: 'Context compacted',
        detail: 'Codex compacted the active thread context',
        status: 'completed'
      }
    ]);
  });

  test('renders slash command echo and execution result without merging them', () => {
    let state = createChatState();

    state = applyChatEnvelope(state, {
      type: 'app_event',
      event_id: 'evt-slash-user',
      event: {
        type: 'user_message',
        payload: {
          session_id: 's1',
          message_id: 'slash-user-1',
          content: '/compact'
        }
      }
    });
    state = applyChatEnvelope(state, {
      type: 'app_event',
      event_id: 'evt-slash-result',
      event: {
        type: 'provider_event',
        payload: {
          session_id: 's1',
          provider_id: 'codex',
          event_id: 'slash-codex-compact',
          category: 'slash_command',
          title: '/compact',
          detail: 'Codex compaction started.',
          status: 'accepted',
          provider_payload: {
            command_id: 'codex.compact',
            target: 'provider',
            danger_level: 'safe'
          }
        }
      }
    });

    expect(state.items).toMatchObject([
      {
        kind: 'user',
        content: '/compact'
      },
      {
        kind: 'inlineEvent',
        eventKind: 'event',
        title: '/compact',
        detail: 'Codex compaction started.',
        status: 'accepted'
      }
    ]);
  });

  test('renders error code and provider payload details', () => {
    let state = createChatState();

    state = applyChatEnvelope(state, {
      type: 'app_event',
      event_id: 'evt-error',
      event: {
        type: 'error',
        payload: {
          session_id: 's1',
          code: 'codex_turn_failed',
          message: 'codex turn/start failed',
          provider_payload: {
            operation: 'send_session_message',
            request_id: 'req-1',
            detail: 'thread not found'
          }
        }
      }
    });

    expect(state.items[0]).toMatchObject({
      kind: 'error',
      title: 'codex turn/start failed',
      detail: expect.stringContaining('codex_turn_failed')
    });
    expect(state.items[0]).toMatchObject({
      detail: expect.stringContaining('thread not found')
    });
  });
});
