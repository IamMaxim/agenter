import { describe, expect, test } from 'vitest';

import type { SessionSnapshot, UniversalEventEnvelope } from '../api/types';
import { applyUniversalEvent } from './universalEvents';

function snapshot(overrides: Partial<SessionSnapshot> = {}): SessionSnapshot {
  return {
    session_id: 's1',
    latest_seq: '1',
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

function event(overrides: Partial<UniversalEventEnvelope>): UniversalEventEnvelope {
  return {
    protocol_version: 'uap/2',
    event_id: 'evt-1',
    seq: '2',
    session_id: 's1',
    ts: '2026-05-04T12:00:00Z',
    source: 'control_plane',
    event: {
      type: 'native.unknown',
      data: { summary: 'noop' }
    },
    ...overrides
  };
}

describe('universal event reducer', () => {
  test('stores usage updates even before session info is known', () => {
    const state = applyUniversalEvent(
      snapshot({ info: null }),
      event({
        event_id: 'evt-usage',
        event: {
          type: 'usage.updated',
          data: {
            usage: {
              context: { used_percent: 41, used_tokens: 41000, total_tokens: 100000 },
              window_5h: { remaining_percent: 33, resets_at: '2026-05-05T17:00:00Z' },
              week: { remaining_percent: 88 }
            }
          }
        }
      })
    );

    expect(state.info?.usage).toMatchObject({
      context: { used_percent: 41, used_tokens: 41000, total_tokens: 100000 },
      window_5h: { remaining_percent: 33, resets_at: '2026-05-05T17:00:00Z' },
      week: { remaining_percent: 88 }
    });
  });

  test('merges resolved approval projection without losing request details', () => {
    const state = applyUniversalEvent(
      snapshot({
        approvals: {
          ap1: {
            approval_id: 'ap1',
            session_id: 's1',
            kind: 'command',
            title: 'Run command',
            details: 'cargo test',
            options: [{ option_id: 'approve_once', kind: 'approve_once', label: 'Approve once' }],
            status: 'pending',
            risk: 'low',
            subject: 'cargo test',
            native_request_id: 'native-1',
            native_blocking: true,
            requested_at: '2026-05-04T11:59:00Z'
          }
        }
      }),
      event({
        event_id: 'evt-approval-resolved',
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
              native_blocking: true,
              resolved_at: '2026-05-04T12:00:00Z'
            }
          }
        }
      })
    );

    expect(state.approvals.ap1).toMatchObject({
      kind: 'command',
      title: 'Run command',
      details: 'cargo test',
      status: 'approved',
      risk: 'low',
      subject: 'cargo test',
      native_request_id: 'native-1',
      requested_at: '2026-05-04T11:59:00Z',
      resolved_at: '2026-05-04T12:00:00Z'
    });
    expect(state.approvals.ap1.options).toEqual([
      { option_id: 'approve_once', kind: 'approve_once', label: 'Approve once' }
    ]);
  });

  test('merges answered question projection without losing request fields', () => {
    const state = applyUniversalEvent(
      snapshot({
        questions: {
          q1: {
            question_id: 'q1',
            session_id: 's1',
            title: 'Pick target',
            description: 'Choose a deployment target',
            fields: [
              {
                id: 'target',
                label: 'Target',
                kind: 'select',
                required: true,
                secret: false,
                choices: [{ value: 'prod', label: 'Production' }],
                default_answers: []
              }
            ],
            status: 'pending',
            requested_at: '2026-05-04T11:58:00Z'
          }
        }
      }),
      event({
        event_id: 'evt-question-answered',
        event: {
          type: 'question.answered',
          data: {
            question: {
              question_id: 'q1',
              session_id: 's1',
              title: 'Input requested',
              fields: [],
              status: 'answered',
              answer: {
                question_id: 'q1',
                answers: { target: ['prod'] }
              },
              answered_at: '2026-05-04T12:00:00Z'
            }
          }
        }
      })
    );

    expect(state.questions.q1).toMatchObject({
      title: 'Pick target',
      description: 'Choose a deployment target',
      status: 'answered',
      requested_at: '2026-05-04T11:58:00Z',
      answered_at: '2026-05-04T12:00:00Z',
      answer: { question_id: 'q1', answers: { target: ['prod'] } }
    });
    expect(state.questions.q1.fields).toHaveLength(1);
    expect(state.questions.q1.fields[0].label).toBe('Target');
  });
});
