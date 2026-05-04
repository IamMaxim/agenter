import { afterEach, describe, expect, test, vi } from 'vitest';

import { disableApprovalRule, listApprovalRules } from './approvalRules';

describe('approval rule APIs', () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  test('lists approval rules for a workspace provider', async () => {
    const fetch = vi.fn().mockResolvedValue({
      ok: true,
      status: 200,
      text: () =>
        Promise.resolve(
          JSON.stringify([
            {
              rule_id: 'rule-1',
              workspace_id: 'workspace-1',
              provider_id: 'codex',
              kind: 'command',
              label: 'Approve commands starting with `cargo test`',
              matcher: { type: 'command_prefix', prefix: ['cargo', 'test'] },
              decision: { decision: 'accept_for_session' },
              disabled_at: null,
              created_at: '2026-05-04T00:00:00Z',
              updated_at: '2026-05-04T00:00:00Z'
            }
          ])
        )
    });
    vi.stubGlobal('fetch', fetch);

    await expect(listApprovalRules('workspace 1', 'codex')).resolves.toHaveLength(1);
    expect(fetch).toHaveBeenCalledWith(
      '/api/approval-rules?workspace_id=workspace+1&provider_id=codex',
      expect.objectContaining({ credentials: 'include' })
    );
  });

  test('disables an approval rule', async () => {
    const fetch = vi.fn().mockResolvedValue({
      ok: true,
      status: 200,
      text: () =>
        Promise.resolve(
          JSON.stringify({
            rule_id: 'rule-1',
            workspace_id: 'workspace-1',
            provider_id: 'codex',
            kind: 'command',
            label: 'Approve commands starting with `cargo test`',
            matcher: { type: 'command_prefix', prefix: ['cargo', 'test'] },
            decision: { decision: 'accept_for_session' },
            disabled_at: '2026-05-04T00:01:00Z',
            created_at: '2026-05-04T00:00:00Z',
            updated_at: '2026-05-04T00:01:00Z'
          })
        )
    });
    vi.stubGlobal('fetch', fetch);

    await expect(disableApprovalRule('rule 1')).resolves.toMatchObject({
      rule_id: 'rule-1',
      disabled_at: '2026-05-04T00:01:00Z'
    });
    expect(fetch).toHaveBeenCalledWith(
      '/api/approval-rules/rule%201/disable',
      expect.objectContaining({ method: 'POST' })
    );
  });
});
