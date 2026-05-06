import { describe, expect, test } from 'vitest';

import {
  defaultReasoningEfforts,
  effortsForSelectedModel,
  normalizeAgentOptions,
  normalizeBrowserServerMessage,
  normalizeRunners,
  normalizeSessionSnapshot,
  normalizeSessions,
  normalizeTurnSettings,
  normalizeWorkspaces
} from './normalizers';

describe('frontend API normalizers', () => {
  test('normalizes partial agent options so selected model efforts are always safe', () => {
    const options = normalizeAgentOptions({
      models: [
        {
          id: 'gpt-5.3',
          display_name: 'GPT 5.3'
        }
      ],
      collaboration_modes: [{ id: 'plan', label: 'Plan' }]
    });

    expect(options.models[0]).toMatchObject({
      id: 'gpt-5.3',
      display_name: 'GPT 5.3',
      supported_reasoning_efforts: [],
      input_modalities: []
    });
    expect(effortsForSelectedModel(options, { model: 'gpt-5.3' })).toEqual(defaultReasoningEfforts);
  });

  test('drops malformed collections and fills safe fallback labels', () => {
    expect(normalizeAgentOptions({ models: 'bad', collaboration_modes: 'bad' })).toEqual({
      models: [],
      collaboration_modes: []
    });
    expect(normalizeRunners([{ runner_id: 12 }, { runner_id: 'runner-1' }])).toEqual([
      {
        runner_id: 'runner-1',
        name: 'runner-1',
        status: 'offline',
        last_seen_at: null,
        provider_ids: []
      }
    ]);
    expect(
      normalizeRunners([
        {
          runner_id: 'runner-codex',
          provider_ids: ['codex', 42, '', 'qwen']
        }
      ])
    ).toEqual([
      {
        runner_id: 'runner-codex',
        name: 'runner-codex',
        status: 'offline',
        last_seen_at: null,
        provider_ids: ['codex', 'qwen']
      }
    ]);
    expect(normalizeWorkspaces([{ workspace_id: 'workspace-1', runner_id: 'runner-1' }])).toEqual([
      {
        workspace_id: 'workspace-1',
        runner_id: 'runner-1',
        path: 'Unknown workspace',
        display_name: null
      }
    ]);
    expect(normalizeSessions([{ session_id: 'session-1', workspace_id: 'workspace-1' }])).toEqual([
      {
        session_id: 'session-1',
        owner_user_id: '',
        runner_id: '',
        workspace_id: 'workspace-1',
        provider_id: 'unknown',
        status: 'degraded',
        external_session_id: null,
        title: null,
        created_at: null,
        updated_at: null,
        usage: null
      }
    ]);
  });

  test('normalizes session usage snapshots with partial payloads', () => {
    expect(
      normalizeSessions([
        {
          session_id: 'session-1',
          workspace_id: 'workspace-1',
          usage: {
            mode_label: 'plan',
            model: 'gpt-5.4',
            reasoning_effort: 'high',
            context: {
              used_percent: 19,
              used_tokens: 42000,
              total_tokens: 258000
            },
            window_5h: {
              remaining_percent: 42,
              resets_at: '2026-04-12T20:01:00Z'
            },
            week: {
              remaining_percent: 'bad'
            }
          }
        }
      ])[0].usage
    ).toEqual({
      mode_label: 'plan',
      model: 'gpt-5.4',
      reasoning_effort: 'high',
      context: {
        used_percent: 19,
        used_tokens: 42000,
        total_tokens: 258000
      },
      window_5h: {
        used_percent: null,
        remaining_percent: 42,
        resets_at: '2026-04-12T20:01:00Z',
        window_label: null,
        remaining_text_hint: null
      },
      week: {
        used_percent: null,
        remaining_percent: null,
        resets_at: null,
        window_label: null,
        remaining_text_hint: null
      }
    });
  });

  test('accepts idle and stopped session statuses', () => {
    expect(
      normalizeSessions([
        { session_id: 'idle-session', workspace_id: 'workspace-1', status: 'idle' },
        { session_id: 'stopped-session', workspace_id: 'workspace-1', status: 'stopped' }
      ]).map((session) => session.status)
    ).toEqual(['idle', 'stopped']);
  });

  test('normalizes turn settings defensively', () => {
    expect(normalizeTurnSettings({ model: 123, reasoning_effort: 'mega', collaboration_mode: 'plan' })).toEqual({
      collaboration_mode: 'plan'
    });
  });

  test('preserves provider capability details in session snapshots', () => {
    const snapshot = normalizeSessionSnapshot({
      session_id: 'session-1',
      capabilities: {
        provider_details: [
          {
            key: 'dynamic_tools',
            status: 'degraded',
            methods: ['item/tool/call', 42],
            reason: 'Visible but not executed remotely.'
          }
        ]
      }
    });

    expect(snapshot.capabilities?.provider_details).toEqual([
      {
        key: 'dynamic_tools',
        status: 'degraded',
        methods: ['item/tool/call'],
        reason: 'Visible but not executed remotely.'
      }
    ]);
  });

  test('preserves raw native payloads and schema gap fields', () => {
    const snapshot = normalizeSessionSnapshot({
      session_id: 'session-1',
      items: {
        item1: {
          item_id: 'item1',
          session_id: 'session-1',
          role: 'tool',
          status: 'completed',
          tool: {
            kind: 'tool',
            subkind: 'web_search',
            name: 'web_search',
            title: 'Web search',
            status: 'completed'
          },
          native: {
            protocol: 'codex/app-server/v2',
            method: 'rawResponseItem/completed',
            raw_payload: { params: { item: { id: 'item1' } } }
          }
        }
      },
      questions: {
        question1: {
          question_id: 'question1',
          session_id: 'session-1',
          title: 'Input',
          status: 'pending',
          native_request_id: 'request-1',
          native_blocking: true,
          fields: [
            {
              id: 'choice',
              label: 'Choice',
              kind: 'select',
              required: true,
              secret: false,
              schema: { type: 'string', enum: ['a', 'b'] }
            }
          ]
        }
      }
    });

    expect(snapshot.items.item1.tool?.subkind).toBe('web_search');
    expect(snapshot.items.item1.native?.raw_payload).toEqual({
      params: { item: { id: 'item1' } }
    });
    expect(snapshot.questions.question1).toMatchObject({
      native_request_id: 'request-1',
      native_blocking: true
    });
    expect(snapshot.questions.question1.fields[0].schema).toEqual({
      type: 'string',
      enum: ['a', 'b']
    });
  });

  test('normalizes versioned universal snapshot frames and defaults legacy missing versions', () => {
    const versioned = normalizeBrowserServerMessage({
      type: 'session_snapshot',
      protocol_version: 'uap/2',
      snapshot: { session_id: 'session-1', latest_seq: '1' },
      events: [
        {
          protocol_version: 'uap/2',
          event_id: '11111111-1111-4111-8111-111111111111',
          seq: '1',
          session_id: 'session-1',
          ts: '2026-05-05T12:00:00Z',
          source: 'runner',
          event: { type: 'native.unknown', data: { summary: 'hello' } }
        }
      ],
      snapshot_seq: '1',
      replay_from_seq: '1',
      replay_through_seq: '1',
      replay_complete: true
    });

    expect(versioned.type).toBe('session_snapshot');
    if (versioned.type !== 'session_snapshot') throw new Error('expected snapshot');
    expect(versioned.protocol_version).toBe('uap/2');
    expect(versioned.events[0].protocol_version).toBe('uap/2');
    expect(versioned.snapshot_seq).toBe('1');
    expect(versioned.replay_from_seq).toBe('1');
    expect(versioned.replay_through_seq).toBe('1');
    expect(versioned.replay_complete).toBe(true);

    const legacy = normalizeBrowserServerMessage({
      type: 'universal_event',
      event_id: '22222222-2222-4222-8222-222222222222',
      seq: '2',
      session_id: 'session-1',
      ts: '2026-05-05T12:00:01Z',
      source: 'runner',
      event: { type: 'native.unknown', data: { summary: 'legacy' } }
    });

    expect(legacy.type).toBe('universal_event');
    if (legacy.type !== 'universal_event') throw new Error('expected universal event');
    expect(legacy.protocol_version).toBe('uap/2');
  });

  test('normalizes all known universal event types without native unknown fallback', () => {
    const base = {
      type: 'universal_event',
      protocol_version: 'uap/2',
      event_id: '33333333-3333-4333-8333-333333333333',
      seq: '3',
      session_id: 'session-1',
      ts: '2026-05-05T12:00:02Z',
      source: 'runner'
    };
    const cases: Array<{ type: string; data: Record<string, unknown> }> = [
      {
        type: 'session.created',
        data: { session: { session_id: 'session-1', workspace_id: 'workspace-1' } }
      },
      { type: 'session.status_changed', data: { status: 'running', reason: 'turn started' } },
      { type: 'session.metadata_changed', data: { title: 'Readable title' } },
      { type: 'turn.started', data: { turn: { turn_id: 'turn-1', session_id: 'session-1', status: 'running' } } },
      { type: 'turn.status_changed', data: { turn: { turn_id: 'turn-1', session_id: 'session-1', status: 'waiting_for_input' } } },
      { type: 'turn.completed', data: { turn: { turn_id: 'turn-1', session_id: 'session-1', status: 'completed' } } },
      { type: 'turn.failed', data: { turn: { turn_id: 'turn-1', session_id: 'session-1', status: 'failed' } } },
      { type: 'turn.cancelled', data: { turn: { turn_id: 'turn-1', session_id: 'session-1', status: 'cancelled' } } },
      { type: 'turn.interrupted', data: { turn: { turn_id: 'turn-1', session_id: 'session-1', status: 'interrupted' } } },
      { type: 'turn.detached', data: { turn: { turn_id: 'turn-1', session_id: 'session-1', status: 'detached' } } },
      { type: 'item.created', data: { item: { item_id: 'item-1', session_id: 'session-1', role: 'assistant', status: 'created' } } },
      { type: 'content.delta', data: { block_id: 'block-1', kind: 'text', delta: 'hello' } },
      { type: 'content.completed', data: { block_id: 'block-1', kind: 'text', text: 'hello' } },
      { type: 'approval.requested', data: { approval: { approval_id: 'approval-1', session_id: 'session-1', kind: 'command', title: 'Run command', status: 'pending' } } },
      { type: 'approval.resolved', data: { approval_id: 'approval-1', status: 'approved', resolved_at: '2026-05-05T12:00:03Z' } },
      { type: 'question.requested', data: { question: { question_id: 'question-1', session_id: 'session-1', title: 'Input', status: 'pending' } } },
      { type: 'question.answered', data: { question: { question_id: 'question-1', session_id: 'session-1', title: 'Input', status: 'answered' } } },
      { type: 'plan.updated', data: { plan: { plan_id: 'plan-1', session_id: 'session-1', status: 'draft' } } },
      { type: 'diff.updated', data: { diff: { diff_id: 'diff-1', session_id: 'session-1', files: [] } } },
      { type: 'artifact.created', data: { artifact: { artifact_id: 'artifact-1', session_id: 'session-1', kind: 'file', title: 'Artifact' } } },
      { type: 'usage.updated', data: { usage: { context: { used_percent: 25 } } } },
      { type: 'error.reported', data: { code: 'provider_error', message: 'Provider failed' } },
      { type: 'provider.notification', data: { notification: { category: 'hook', title: 'Hook started' } } },
      { type: 'native.unknown', data: { summary: 'future event' } }
    ];

    for (const entry of cases) {
      const message = normalizeBrowserServerMessage({ ...base, event: entry });
      expect(message.type).toBe('universal_event');
      if (message.type !== 'universal_event') throw new Error('expected universal event');
      expect(message.event.type).toBe(entry.type);
    }
  });
});
