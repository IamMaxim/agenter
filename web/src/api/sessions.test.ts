import { afterEach, describe, expect, test, vi } from 'vitest';

import { executeSlashCommand, listSlashCommands, refreshWorkspaceProviderSessions } from './sessions';

describe('session APIs', () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  test('refreshes provider sessions for a workspace', async () => {
    const fetch = vi.fn().mockResolvedValue({
      ok: true,
      status: 200,
      text: () =>
        Promise.resolve(
          JSON.stringify({
            discovered_count: 3,
            refreshed_cache_count: 2,
            skipped_failed_count: 1
          })
        )
    });
    vi.stubGlobal('fetch', fetch);

    await expect(refreshWorkspaceProviderSessions('workspace 1', 'codex')).resolves.toEqual({
      discovered_count: 3,
      refreshed_cache_count: 2,
      skipped_failed_count: 1
    });
    expect(fetch).toHaveBeenCalledWith(
      '/api/workspaces/workspace%201/providers/codex/sessions/refresh',
      expect.objectContaining({ method: 'POST' })
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
});
