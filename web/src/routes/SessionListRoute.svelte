<script lang="ts">
  import { onMount } from 'svelte';
  import { createSession, listRunners, listRunnerWorkspaces, listSessions } from '../api/sessions';
  import type { SessionInfo, WorkspaceRef } from '../api/types';
  import { routeHref } from '../lib/router';
  import { sortSessionsByDate } from '../lib/sidebarTree';
  import { pushToast } from '../lib/toasts';

  let sessions: SessionInfo[] = [];
  let firstWorkspace: WorkspaceRef | undefined;
  let loading = true;
  let error = '';
  let creating = false;

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
</script>

<section class="page-section">
  <div class="section-header">
    <div>
      <h1>Sessions</h1>
      <p>Persistent agent conversations across browser and future connector projections.</p>
    </div>
    <button type="button" disabled={!firstWorkspace || creating} on:click={newSession}>
      {creating ? 'Creating...' : 'New session'}
    </button>
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
