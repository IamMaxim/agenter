import { afterEach, describe, expect, test, vi } from 'vitest';

import {
  createSession,
  decideApproval,
  executeSlashCommand,
  getWorkspaceProviderSessionRefreshStatus,
  interruptSessionTurn,
  listSlashCommands,
  refreshWorkspaceProviderSessions,
  sendSessionMessage
} from './sessions';

describe('session APIs', () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  test('decideApproval preserves canonical universal option ids when present', async () => {
    const fetch = vi.fn().mockResolvedValue({
      ok: true,
      status: 200,
      text: () =>
        Promise.resolve(
          JSON.stringify({
            status: 'accepted'
          })
        )
    });
    vi.stubGlobal('fetch', fetch);

    await decideApproval('approval 1', {
      decision: 'decline',
      option_id: 'deny_with_feedback',
      feedback: 'Needs a safer command.'
    });

    expect(fetch).toHaveBeenCalledWith(
      '/api/approvals/approval%201/decision',
      expect.objectContaining({
        method: 'POST',
        body: JSON.stringify({
          decision: 'decline',
          option_id: 'deny_with_feedback',
          feedback: 'Needs a safer command.'
        })
      })
    );
  });

  test('interruptSessionTurn posts the runner interrupt slash command', async () => {
    const fetch = vi.fn().mockResolvedValue({
      ok: true,
      status: 200,
      text: () =>
        Promise.resolve(
          JSON.stringify({
            accepted: true,
            message: 'Interrupt requested.',
            session: null,
            provider_payload: null
          })
        )
    });
    vi.stubGlobal('fetch', fetch);

    await expect(interruptSessionTurn('session 1')).resolves.toEqual({
      accepted: true,
      message: 'Interrupt requested.',
      session: undefined,
      provider_payload: null
    });

    expect(fetch).toHaveBeenCalledWith(
      '/api/sessions/session%201/slash-commands',
      expect.objectContaining({
        method: 'POST',
        body: JSON.stringify({
          command_id: 'runner.interrupt',
          arguments: {},
          raw_input: '/interrupt',
          confirmed: true
        })
      })
    );
  });

  test('refreshes provider sessions for a workspace', async () => {
    const fetch = vi.fn().mockResolvedValue({
      ok: true,
      status: 200,
      text: () =>
        Promise.resolve(
          JSON.stringify({
            refresh_id: 'refresh-1',
            status: 'queued'
          })
        )
    });
    vi.stubGlobal('fetch', fetch);

    await expect(refreshWorkspaceProviderSessions('workspace 1', 'codex')).resolves.toEqual({
      refresh_id: 'refresh-1',
      status: 'queued'
    });
    expect(fetch).toHaveBeenCalledWith(
      '/api/workspaces/workspace%201/providers/codex/sessions/refresh',
      expect.objectContaining({ method: 'POST' })
    );
  });

  test('loads provider session refresh status', async () => {
    const fetch = vi.fn().mockResolvedValue({
      ok: true,
      status: 200,
      text: () =>
        Promise.resolve(
          JSON.stringify({
            refresh_id: 'refresh-1',
            status: 'reading_history',
            progress: {
              current: 2,
              total: 4,
              percent: 50
            },
            log: [
              {
                ts: '2026-05-03T00:00:00Z',
                level: 'info',
                status: 'reading_history',
                message: 'Read 2 of 4 sessions',
                progress: { current: 2, total: 4, percent: 50 }
              }
            ],
            summary: {
              discovered_count: 3,
              refreshed_cache_count: 2,
              skipped_failed_count: 1
            },
            updated_at: '2026-05-03T00:00:00Z'
          })
        )
    });
    vi.stubGlobal('fetch', fetch);

    await expect(
      getWorkspaceProviderSessionRefreshStatus('workspace 1', 'codex', 'refresh-1')
    ).resolves.toEqual({
      refresh_id: 'refresh-1',
      status: 'reading_history',
      progress: {
        current: 2,
        total: 4,
        percent: 50
      },
      log: [
        {
          ts: '2026-05-03T00:00:00Z',
          level: 'info',
          status: 'reading_history',
          message: 'Read 2 of 4 sessions',
          progress: { current: 2, total: 4, percent: 50 }
        }
      ],
      summary: {
        discovered_count: 3,
        refreshed_cache_count: 2,
        skipped_failed_count: 1
      },
      error: undefined,
      updated_at: '2026-05-03T00:00:00Z'
    });
    expect(fetch).toHaveBeenCalledWith(
      '/api/workspaces/workspace%201/providers/codex/sessions/refresh/refresh-1',
      expect.objectContaining({ credentials: 'include' })
    );
  });

  test('lists and executes slash commands', async () => {
    const fetch = vi
      .fn()
      .mockResolvedValueOnce({
        ok: true,
        status: 200,
        text: () =>
          Promise.resolve(
            JSON.stringify([
              {
                id: 'codex.shell',
                name: 'shell',
                aliases: ['sh'],
                description: 'Run shell',
                category: 'provider',
                provider_id: 'codex',
                target: 'provider',
                danger_level: 'dangerous',
                arguments: [
                  {
                    name: 'command',
                    kind: 'rest',
                    required: true,
                    description: 'Command',
                    choices: []
                  }
                ],
                examples: ['/shell pwd']
              }
            ])
          )
      })
      .mockResolvedValueOnce({
        ok: true,
        status: 200,
        text: () =>
          Promise.resolve(
            JSON.stringify({
              accepted: true,
              message: 'Codex shell command submitted.',
              session: null,
              provider_payload: { id: 1 }
            })
          )
      });
    vi.stubGlobal('fetch', fetch);

    await expect(listSlashCommands('session 1')).resolves.toEqual([
      expect.objectContaining({
        id: 'codex.shell',
        name: 'shell',
        danger_level: 'dangerous',
        arguments: [expect.objectContaining({ name: 'command', kind: 'rest' })]
      })
    ]);
    await expect(
      executeSlashCommand('session 1', {
        command_id: 'codex.shell',
        arguments: { command: 'pwd' },
        raw_input: '/shell pwd',
        confirmed: true
      })
    ).resolves.toEqual({
      accepted: true,
      message: 'Codex shell command submitted.',
      session: undefined,
      provider_payload: { id: 1 }
    });
    expect(fetch).toHaveBeenLastCalledWith(
      '/api/sessions/session%201/slash-commands',
      expect.objectContaining({
        method: 'POST',
        body: JSON.stringify({
          command_id: 'codex.shell',
          arguments: { command: 'pwd' },
          raw_input: '/shell pwd',
          confirmed: true
        })
      })
    );
  });

  test('sendSessionMessage forwards content and settings_override to the control plane', async () => {
    const fetch = vi.fn().mockResolvedValue({
      ok: true,
      status: 202,
      text: () => Promise.resolve('')
    });
    vi.stubGlobal('fetch', fetch);

    await sendSessionMessage('session 1', {
      content: 'Implement the plan.',
      settings_override: { collaboration_mode: 'default' }
    });

    expect(fetch).toHaveBeenCalledWith(
      '/api/sessions/session%201/messages',
      expect.objectContaining({
        method: 'POST',
        body: JSON.stringify({
          content: 'Implement the plan.',
          settings_override: { collaboration_mode: 'default' }
        })
      })
    );
  });

  test('createSession forwards initial_message and settings_override for the fresh-thread handoff', async () => {
    const fetch = vi.fn().mockResolvedValue({
      ok: true,
      status: 201,
      text: () =>
        Promise.resolve(
          JSON.stringify({
            session_id: 's2',
            owner_user_id: 'u1',
            runner_id: 'r1',
            workspace_id: 'w1',
            provider_id: 'codex',
            status: 'starting',
            external_session_id: 'codex-thread-2',
            title: 'Implement plan'
          })
        )
    });
    vi.stubGlobal('fetch', fetch);

    await createSession({
      workspace_id: 'w1',
      provider_id: 'codex',
      title: 'Implement plan',
      initial_message: 'PREAMBLE\n\nplan body',
      settings_override: { collaboration_mode: 'default' }
    });

    expect(fetch).toHaveBeenCalledWith(
      '/api/sessions',
      expect.objectContaining({
        method: 'POST',
        body: JSON.stringify({
          workspace_id: 'w1',
          provider_id: 'codex',
          title: 'Implement plan',
          initial_message: 'PREAMBLE\n\nplan body',
          settings_override: { collaboration_mode: 'default' }
        })
      })
    );
  });
});
