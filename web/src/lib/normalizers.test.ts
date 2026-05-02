import { describe, expect, test } from 'vitest';

import {
  defaultReasoningEfforts,
  effortsForSelectedModel,
  normalizeAgentOptions,
  normalizeBrowserEventEnvelope,
  normalizeRunners,
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
        last_seen_at: null
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

  test('normalizes turn settings and browser event envelopes defensively', () => {
    expect(normalizeTurnSettings({ model: 123, reasoning_effort: 'mega', collaboration_mode: 'plan' })).toEqual({
      collaboration_mode: 'plan'
    });

    const envelope = normalizeBrowserEventEnvelope({
      type: 'app_event',
      event_id: 5,
      event: {
        type: 'agent_message_delta',
        payload: null
      }
    });

    expect(envelope).toEqual({
      type: 'app_event',
      event: {
        type: 'agent_message_delta',
        payload: {}
      }
    });
  });
});
