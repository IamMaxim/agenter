<script lang="ts">
  import { onMount } from 'svelte';
  import { createSession, listRunners, listRunnerWorkspaces, listSessions } from '../api/sessions';
  import type { AuthenticatedUser, RunnerInfo, SessionInfo, WorkspaceRef } from '../api/types';
  import { routeHref, type AppRoute } from '../lib/router';
  import { buildSessionTree, type SessionTreeGroup } from '../lib/sidebarTree';
  import { pushToast } from '../lib/toasts';

  export let user: AuthenticatedUser;
  export let route: AppRoute;
  export let onSignOut: () => void | Promise<void>;

  let runners: RunnerInfo[] = [];
  let sessions: SessionInfo[] = [];
  let workspacesByRunner: Record<string, WorkspaceRef[]> = {};
  let groups: SessionTreeGroup[] = [];
  let loading = true;
  let error = '';
  let creatingGroupId = '';
  let lastRouteKey = '';
  let mounted = false;

  $: groups = buildSessionTree({ runners, workspacesByRunner, sessions });
  $: activeSessionId = route.name === 'chat' ? route.sessionId : undefined;
  $: routeKey = route.name === 'chat' ? `${route.name}:${route.sessionId}` : route.name;
  $: if (mounted && routeKey !== lastRouteKey) {
    lastRouteKey = routeKey;
    void refreshSidebar();
  }

  onMount(() => {
    mounted = true;
    lastRouteKey = routeKey;
    void refreshSidebar();
    const refresh = () => void refreshSidebar();
    window.addEventListener('agenter:sessions-changed', refresh);
    return () => window.removeEventListener('agenter:sessions-changed', refresh);
  });

  async function refreshSidebar() {
    error = '';
    try {
      runners = await listRunners();
      const workspacePairs = await Promise.all(
        runners.map(async (runner) => [
          runner.runner_id,
          await listRunnerWorkspaces(runner.runner_id)
        ] as const)
      );
      workspacesByRunner = Object.fromEntries(workspacePairs);
      sessions = await listSessions();
    } catch {
      error = 'Could not load sessions.';
      pushToast({ severity: 'error', message: error });
    } finally {
      loading = false;
    }
  }

  function sessionTitle(session: SessionInfo) {
    return session.title?.trim() || 'Untitled session';
  }

  async function newSession(group: SessionTreeGroup) {
    if (creatingGroupId) {
      return;
    }
    creatingGroupId = group.id;
    try {
      const session = await createSession({
        workspace_id: group.workspace.workspace_id,
        provider_id: 'codex',
        title: `Chat in ${group.workspace.display_name ?? group.workspace.path}`
      });
      window.location.hash = routeHref({ name: 'chat', sessionId: session.session_id }).slice(1);
      window.dispatchEvent(new CustomEvent('agenter:sessions-changed'));
      await refreshSidebar();
    } catch {
      pushToast({ severity: 'error', message: 'Could not create session for this workspace.' });
    } finally {
      creatingGroupId = '';
    }
  }
</script>

<aside class="sidebar">
  <div class="brand">
    <strong>Agenter</strong>
    <span>{user.display_name ?? user.email}</span>
  </div>

  <nav class="session-tree" aria-label="Sessions by runner and workspace">
    <a
      class="tree-heading"
      class:active={route.name === 'sessions'}
      href={routeHref({ name: 'sessions' })}
    >
      Sessions
    </a>

    {#if loading}
      <span class="tree-muted">Loading sessions...</span>
    {:else if error}
      <span class="tree-error">{error}</span>
    {:else if groups.length === 0}
      <span class="tree-muted">Start a runner to advertise a workspace.</span>
    {:else}
      {#each groups as group (group.id)}
        <section class="tree-group" aria-label={group.label}>
          <div class="tree-group-row">
            <span class:online={group.status === 'online'} class="runner-dot" aria-hidden="true"></span>
            <span class="tree-group-label" title={group.label}>{group.label}</span>
            <span class="tree-count">{group.sessions.length}</span>
            <button
              aria-label={`New session in ${group.label}`}
              class="tree-new-session"
              disabled={creatingGroupId === group.id || group.status !== 'online'}
              title="New session"
              type="button"
              on:click={() => newSession(group)}
            >
              +
            </button>
          </div>

          <div class="tree-session-list">
            {#each group.sessions as session (session.session_id)}
              <a
                class="tree-session"
                class:active={activeSessionId === session.session_id}
                href={routeHref({ name: 'chat', sessionId: session.session_id })}
                title={sessionTitle(session)}
              >
                <span>{sessionTitle(session)}</span>
                <small>{session.provider_id} · {session.status}</small>
              </a>
            {:else}
              <span class="tree-empty">No sessions</span>
            {/each}
          </div>
        </section>
      {/each}
    {/if}
  </nav>

  <div class="sidebar-footer">
    <a class="utility-link" class:active={route.name === 'workspaces'} href={routeHref({ name: 'workspaces' })}>
      Workspaces
    </a>
    <button class="secondary" type="button" on:click={onSignOut}>Sign out</button>
  </div>
</aside>
