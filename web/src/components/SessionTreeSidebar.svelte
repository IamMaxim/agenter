<script lang="ts">
  import { onMount } from 'svelte';
  import AgenterIcon from './AgenterIcon.svelte';
  import {
    createSession,
    listRunners,
    listRunnerWorkspaces,
    listSessions,
    refreshWorkspaceProviderSessions
  } from '../api/sessions';
  import type { AuthenticatedUser, RunnerInfo, SessionInfo, SessionStatus, WorkspaceRef } from '../api/types';
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
  let query = '';
  let collapsedGroups: Record<string, boolean> = {};
  let searchFocused = false;
  let lastRouteKey = '';
  let mounted = false;
  let contextMenuX = 0;
  let contextMenuY = 0;
  let contextMenuGroupId = '';
  let contextMenuVisible = false;
  const FALLBACK_REFRESH_PROVIDER = 'codex';

  $: groups = buildSessionTree({ runners, workspacesByRunner, sessions });
  $: activeSessionId = route.name === 'chat' ? route.sessionId : undefined;
  $: filteredGroups = filterGroups(groups, query);
  $: totalRunning = sessions.filter((session) => session.status === 'running').length;
  $: firstOnlineGroup = groups.find((group) => group.status === 'online');
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
    window.addEventListener('pointerdown', closeContextMenuOutside);
    window.addEventListener('keydown', closeContextMenuOnEscape);
    return () => {
      window.removeEventListener('agenter:sessions-changed', refresh);
      window.removeEventListener('pointerdown', closeContextMenuOutside);
      window.removeEventListener('keydown', closeContextMenuOnEscape);
    };
  });

  async function refreshSidebar() {
    closeContextMenu();
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

  function filterGroups(input: SessionTreeGroup[], rawQuery: string): SessionTreeGroup[] {
    const needle = rawQuery.trim().toLowerCase();
    if (!needle) {
      return input;
    }
    return input
      .map((group) => ({
        ...group,
        sessions: group.sessions.filter((session) =>
          `${sessionTitle(session)} ${session.provider_id} ${session.status}`.toLowerCase().includes(needle)
        )
      }))
      .filter((group) => group.sessions.length > 0);
  }

  function toggleGroup(groupId: string) {
    collapsedGroups = {
      ...collapsedGroups,
      [groupId]: !collapsedGroups[groupId]
    };
  }

  function isGroupOpen(group: SessionTreeGroup) {
    return query.trim() ? true : !collapsedGroups[group.id];
  }

  function workspaceName(group: SessionTreeGroup) {
    return group.workspace.display_name ?? group.workspace.path.split('/').filter(Boolean).at(-1) ?? group.workspace.path;
  }

  function groupTitle(group: SessionTreeGroup) {
    return `${group.runner.name}:${group.workspace.display_name ?? group.workspace.path}`;
  }

  function statusLabel(status: SessionStatus | string) {
    switch (status) {
      case 'running':
        return 'running';
      case 'waiting_for_approval':
        return 'needs approval';
      case 'waiting_for_input':
        return 'waiting';
      case 'failed':
      case 'degraded':
        return 'error';
      case 'completed':
        return 'done';
      case 'starting':
        return 'starting';
      default:
        return String(status).replaceAll('_', ' ');
    }
  }

  function statusTone(status: SessionStatus | string) {
    if (status === 'running' || status === 'starting') {
      return 'running';
    }
    if (status === 'waiting_for_approval' || status === 'waiting_for_input') {
      return 'waiting';
    }
    if (status === 'failed' || status === 'degraded' || status === 'interrupted') {
      return 'error';
    }
    if (status === 'completed') {
      return 'done';
    }
    return 'idle';
  }

  function statusCount(group: SessionTreeGroup, tone: string) {
    return group.sessions.filter((session) => statusTone(session.status) === tone).length;
  }

  function relativeTime(session: SessionInfo) {
    const raw = session.updated_at ?? session.created_at;
    if (!raw) {
      return '';
    }
    const timestamp = Date.parse(raw);
    if (!Number.isFinite(timestamp)) {
      return '';
    }
    const minutes = Math.max(0, Math.round((Date.now() - timestamp) / 60_000));
    if (minutes < 1) {
      return 'now';
    }
    if (minutes < 60) {
      return `${minutes}m`;
    }
    const hours = Math.round(minutes / 60);
    if (hours < 24) {
      return `${hours}h`;
    }
    const days = Math.round(hours / 24);
    return days === 1 ? 'yest' : `${days}d`;
  }

  async function newSessionForFirstOnlineGroup() {
    if (!firstOnlineGroup) {
      pushToast({ severity: 'warning', message: 'No online workspace is available.' });
      return;
    }
    await newSession(firstOnlineGroup);
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

  function getGroupProviderIds(group: SessionTreeGroup): string[] {
    return [...new Set(group.sessions.map((session) => session.provider_id))];
  }

  function openContextMenu(event: MouseEvent, groupId: string) {
    event.preventDefault();
    event.stopPropagation();
    contextMenuVisible = true;
    contextMenuX = event.pageX;
    contextMenuY = event.pageY;
    contextMenuGroupId = groupId;
  }

  function closeContextMenu() {
    contextMenuVisible = false;
    contextMenuGroupId = '';
  }

  function closeContextMenuOutside(event: PointerEvent) {
    if (!contextMenuVisible) {
      return;
    }
    const target = event.target as Element | null;
    if (!target || !target.closest('.runner-context-menu')) {
      closeContextMenu();
    }
  }

  function closeContextMenuOnEscape(event: KeyboardEvent) {
    if (event.key === 'Escape') {
      closeContextMenu();
    }
  }

  async function reloadRunnerWorkspaceSessions(group: SessionTreeGroup) {
    const providers = getGroupProviderIds(group);
    const providerIds = providers.length > 0 ? providers : [FALLBACK_REFRESH_PROVIDER];
    const startedProviderCount = providerIds.length;
    const refreshResults = await Promise.allSettled(
      providerIds.map((providerId) =>
        refreshWorkspaceProviderSessions(group.workspace.workspace_id, providerId)
      )
    );

    const summary = {
      discovered_count: 0,
      refreshed_cache_count: 0,
      skipped_failed_count: 0
    };
    const failedProviderIds: string[] = [];

    refreshResults.forEach((result, index) => {
      if (result.status === 'fulfilled') {
        summary.discovered_count += result.value.discovered_count;
        summary.refreshed_cache_count += result.value.refreshed_cache_count;
        summary.skipped_failed_count += result.value.skipped_failed_count;
      } else {
        failedProviderIds.push(providerIds[index]);
      }
    });

    closeContextMenu();
    if (failedProviderIds.length === 0) {
      if (startedProviderCount === 1) {
        pushToast({
          severity: 'info',
          message:
            `Reloaded sessions from ${providerIds[0]}: discovered ${summary.discovered_count}, refreshed ${summary.refreshed_cache_count}, skipped ${summary.skipped_failed_count}.`
        });
      } else {
        pushToast({
          severity: 'info',
          message: `Reloaded sessions across ${startedProviderCount} providers: discovered ${summary.discovered_count}, refreshed ${summary.refreshed_cache_count}, skipped ${summary.skipped_failed_count}.`
        });
      }
    } else if (failedProviderIds.length === startedProviderCount) {
      pushToast({
        severity: 'error',
        message: `Could not reload sessions for provider${startedProviderCount > 1 ? 's' : ''}: ${failedProviderIds.join(', ')}.`
      });
    } else {
      pushToast({
        severity: 'warning',
        message: `Reloaded sessions with partial success; ${failedProviderIds.length} provider${failedProviderIds.length > 1 ? 's' : ''} failed: ${failedProviderIds.join(', ')}.`
      });
    }
    await refreshSidebar();
  }
</script>

<aside class="sidebar">
  <div class="brand">
    <div class="brand-mark" aria-hidden="true">&gt;_</div>
    <div class="brand-copy">
      <strong>Agenter</strong>
      <span>local · {user.display_name ?? user.email}</span>
    </div>
    <button
      aria-label="New session"
      class="brand-action"
      disabled={!firstOnlineGroup || Boolean(creatingGroupId)}
      title="New session"
      type="button"
      on:click={newSessionForFirstOnlineGroup}
    >
      <AgenterIcon name="plus" size={13} />
    </button>
  </div>

  <div class:focused={searchFocused} class="sidebar-search">
    <span aria-hidden="true" class="search-icon"><AgenterIcon name="search" size={11} /></span>
    <input
      bind:value={query}
      aria-label="Find session"
      placeholder="find session..."
      on:blur={() => (searchFocused = false)}
      on:focus={() => (searchFocused = true)}
    />
    {#if !query}
      <span class="search-shortcut">⌘K</span>
    {/if}
  </div>

  <nav class="session-tree" aria-label="Sessions by runner and workspace">
    {#if loading}
      <span class="tree-muted">Loading sessions...</span>
    {:else if error}
      <span class="tree-error">{error}</span>
    {:else if groups.length === 0}
      <span class="tree-muted">Start a runner to advertise a workspace.</span>
    {:else if filteredGroups.length === 0}
      <span class="tree-muted">no sessions match "{query}"</span>
    {:else}
      {#each filteredGroups as group (group.id)}
        <section class="tree-group" aria-label={groupTitle(group)}>
          <button
            class="tree-group-row"
            type="button"
            on:click={() => toggleGroup(group.id)}
            on:contextmenu={(event) => openContextMenu(event, group.id)}
          >
            <span class:open={isGroupOpen(group)} class="tree-chevron" aria-hidden="true">
              <AgenterIcon name="chevron" size={8} />
            </span>
            <span class="tree-group-label" title={groupTitle(group)}>{workspaceName(group)}</span>
            <span class:online={group.status === 'online'} class="runner-dot" aria-hidden="true"></span>
            {#if statusCount(group, 'running') > 0}
              <span class="tree-status-count running">
                <span class="status-dot running" aria-hidden="true"></span>
                {statusCount(group, 'running')}
              </span>
            {/if}
            {#if statusCount(group, 'waiting') > 0}
              <span class="tree-status-count waiting">
                <span class="status-dot waiting" aria-hidden="true"></span>
                {statusCount(group, 'waiting')}
              </span>
            {/if}
            <span class="tree-count">{group.sessions.length}</span>
          </button>

          {#if isGroupOpen(group)}
            <div class="tree-session-list">
              {#each group.sessions as session (session.session_id)}
                <a
                  class="tree-session"
                  class:active={activeSessionId === session.session_id}
                  href={routeHref({ name: 'chat', sessionId: session.session_id })}
                  title={sessionTitle(session)}
                >
                  <span class="tree-session-title">{sessionTitle(session)}</span>
                  <small>
                    <span class:done={statusTone(session.status) === 'done'} class:error={statusTone(session.status) === 'error'} class:idle={statusTone(session.status) === 'idle'} class:running={statusTone(session.status) === 'running'} class:waiting={statusTone(session.status) === 'waiting'} class="status-dot" aria-hidden="true"></span>
                    <span class:done={statusTone(session.status) === 'done'} class:error={statusTone(session.status) === 'error'} class:idle={statusTone(session.status) === 'idle'} class:running={statusTone(session.status) === 'running'} class:waiting={statusTone(session.status) === 'waiting'} class="session-status-label">{statusLabel(session.status)}</span>
                    <span class="tree-separator">·</span>
                    <span>{session.provider_id}</span>
                    <span class="tree-session-time">{relativeTime(session)}</span>
                  </small>
                </a>
              {:else}
                <span class="tree-empty">No sessions</span>
              {/each}
            </div>
          {/if}

          <button
            aria-label={`New session in ${group.label}`}
            class="tree-new-session"
            disabled={creatingGroupId === group.id || group.status !== 'online'}
            title="New session"
            type="button"
            on:click={() => newSession(group)}
          >
            <AgenterIcon name="plus" size={11} />
          </button>
          {#if contextMenuVisible && contextMenuGroupId === group.id}
            <div
              aria-label="Session actions"
              class="runner-context-menu"
              role="menu"
              style={`left: ${contextMenuX}px; top: ${contextMenuY}px;`}
            >
              <button
                class="runner-context-menu-item"
                type="button"
                role="menuitem"
                on:click={() => void reloadRunnerWorkspaceSessions(group)}
              >
                Reload sessions
              </button>
            </div>
          {/if}
        </section>
      {/each}
    {/if}
  </nav>

  <div class="sidebar-footer">
    <span class="footer-status">
      <span>browser</span>
      <span class="tree-separator">·</span>
      <span class="running"><span class="status-dot running" aria-hidden="true"></span>{totalRunning} running</span>
    </span>
    <button class="sidebar-signout" type="button" on:click={onSignOut}>sign out</button>
  </div>
</aside>
