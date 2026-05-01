import { describe, expect, test } from 'vitest';

import { buildSessionTree, runnerStatusTone } from './sidebarTree';
import type { RunnerInfo, SessionInfo, WorkspaceRef } from '../api/types';

const runnerOnline: RunnerInfo = {
  runner_id: 'runner-a',
  name: 'mac-mini',
  status: 'online'
};

const runnerOffline: RunnerInfo = {
  runner_id: 'runner-b',
  name: 'mbp',
  status: 'offline'
};

const workspaceA: WorkspaceRef = {
  workspace_id: 'workspace-a',
  runner_id: 'runner-a',
  path: '/Users/maxim/work/agenter',
  display_name: null
};

const workspaceB: WorkspaceRef = {
  workspace_id: 'workspace-b',
  runner_id: 'runner-b',
  path: '/Users/maxim/work/Psychoville',
  display_name: 'Psychoville'
};

const sessions: SessionInfo[] = [
  {
    session_id: 'session-b',
    owner_user_id: 'user-1',
    runner_id: 'runner-a',
    workspace_id: 'workspace-a',
    provider_id: 'qwen',
    status: 'waiting_for_input',
    title: null,
    created_at: '2026-04-30T10:00:00Z',
    updated_at: '2026-04-30T10:05:00Z'
  },
  {
    session_id: 'session-a',
    owner_user_id: 'user-1',
    runner_id: 'runner-a',
    workspace_id: 'workspace-a',
    provider_id: 'codex',
    status: 'running',
    title: 'Sidebar tree redesign',
    created_at: '2026-04-30T11:00:00Z',
    updated_at: '2026-04-30T11:05:00Z'
  }
];

describe('sidebar session tree', () => {
  test('groups sessions under flattened runner workspace labels and keeps empty workspaces', () => {
    const tree = buildSessionTree({
      runners: [runnerOffline, runnerOnline],
      workspacesByRunner: {
        'runner-a': [workspaceA],
        'runner-b': [workspaceB]
      },
      sessions
    });

    expect(tree.map((group) => group.label)).toEqual([
      'mac-mini:/Users/maxim/work/agenter',
      'mbp:Psychoville'
    ]);
    expect(tree[0].status).toBe('online');
    expect(tree[0].sessions.map((session) => session.session_id)).toEqual([
      'session-a',
      'session-b'
    ]);
    expect(tree[1].status).toBe('offline');
    expect(tree[1].sessions).toEqual([]);
  });

  test('sorts sessions by most recently updated date with stable fallbacks', () => {
    const tree = buildSessionTree({
      runners: [runnerOnline],
      workspacesByRunner: {
        'runner-a': [workspaceA]
      },
      sessions: [
        {
          ...sessions[0],
          session_id: 'older',
          title: 'Older',
          updated_at: '2026-04-30T09:00:00Z',
          created_at: '2026-04-30T09:00:00Z'
        },
        {
          ...sessions[0],
          session_id: 'newer',
          title: 'Newer',
          updated_at: '2026-04-30T12:00:00Z',
          created_at: '2026-04-30T08:00:00Z'
        },
        {
          ...sessions[0],
          session_id: 'created-only',
          title: 'Created only',
          updated_at: null,
          created_at: '2026-04-30T11:00:00Z'
        }
      ]
    });

    expect(tree[0].sessions.map((session) => session.session_id)).toEqual([
      'newer',
      'created-only',
      'older'
    ]);
  });

  test('maps runner status to online or offline dot tones', () => {
    expect(runnerStatusTone('online')).toBe('online');
    expect(runnerStatusTone('connected')).toBe('online');
    expect(runnerStatusTone('offline')).toBe('offline');
    expect(runnerStatusTone(undefined)).toBe('offline');
    expect(runnerStatusTone('degraded')).toBe('offline');
  });
});
