<script lang="ts">
  import { onMount } from 'svelte';
  import {
    createSession,
    listRunners,
    listRunnerWorkspaces,
    listSessions,
    refreshWorkspaceProviderSessions
  } from '../api/sessions';
  import type { SessionInfo, WorkspaceRef } from '../api/types';
  import { routeHref } from '../lib/router';
  import { sortSessionsByDate } from '../lib/sidebarTree';
  import { pushToast } from '../lib/toasts';

  let sessions: SessionInfo[] = [];
  let firstWorkspace: WorkspaceRef | undefined;
  let loading = true;
  let error = '';
  let creating = false;
  let refreshingProvider = false;

  async function refresh() {
    try {
      sessions = sortSessionsByDate(await listSessions());
      const runners = await listRunners();
      for (const runner of runners) {
        const workspaces = await listRunnerWorkspaces(runner.runner_id);
        if (workspaces[0]) {
          firstWorkspace = workspaces[0];
          break;
        }
      }
    } catch {
      error = 'Could not load sessions.';
      pushToast({ severity: 'error', message: error });
    } finally {
      loading = false;
    }
  }

  onMount(() => {
    void refresh();
  });

  async function newSession() {
    if (!firstWorkspace || creating) {
      return;
    }
    creating = true;
    error = '';
    try {
      const session = await createSession({
        workspace_id: firstWorkspace.workspace_id,
        provider_id: 'codex',
        title: `Chat in ${firstWorkspace.display_name ?? firstWorkspace.path}`
      });
      window.location.hash = routeHref({ name: 'chat', sessionId: session.session_id }).slice(1);
    } catch {
      error = 'Could not create session.';
      pushToast({ severity: 'error', message: error });
    } finally {
      creating = false;
    }
  }

  async function refreshProviderSessions() {
    if (!firstWorkspace || refreshingProvider) {
      return;
    }
    refreshingProvider = true;
    error = '';
    try {
      const summary = await refreshWorkspaceProviderSessions(firstWorkspace.workspace_id, 'codex');
      sessions = sortSessionsByDate(await listSessions());
      pushToast({
        severity: summary.skipped_failed_count > 0 ? 'warning' : 'info',
        message: `Provider refresh: ${summary.refreshed_cache_count}/${summary.discovered_count} session caches refreshed.`
      });
    } catch {
      error = 'Could not refresh provider sessions.';
      pushToast({ severity: 'error', message: error });
    } finally {
      refreshingProvider = false;
    }
  }
</script>

<section class="page-section">
  <div class="section-header">
    <div>
      <h1>Sessions</h1>
      <p>Persistent agent conversations across browser and future connector projections.</p>
    </div>
    <div class="header-actions">
      <button type="button" disabled={!firstWorkspace || refreshingProvider} on:click={refreshProviderSessions}>
        {refreshingProvider ? 'Refreshing...' : 'Refresh provider'}
      </button>
      <button type="button" disabled={!firstWorkspace || creating} on:click={newSession}>
        {creating ? 'Creating...' : 'New session'}
      </button>
    </div>
  </div>

  {#if loading}
    <p class="muted">Loading sessions...</p>
  {:else if error}
    <p class="error" role="alert">{error}</p>
  {:else if sessions.length === 0}
    <div class="empty-state">
      <strong>No sessions yet</strong>
      <span>{firstWorkspace ? 'Create a session from the connected runner.' : 'Start a runner to advertise a workspace.'}</span>
    </div>
  {:else}
    <div class="data-list">
      {#each sessions as session}
        <a class="row-card row-link" href={routeHref({ name: 'chat', sessionId: session.session_id })}>
          <div>
            <strong>{session.title ?? 'Untitled session'}</strong>
            <span>{session.provider_id} · {session.status}</span>
          </div>
          <code>{session.session_id}</code>
        </a>
      {/each}
    </div>
  {/if}
</section>
