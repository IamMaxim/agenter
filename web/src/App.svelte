<script lang="ts">
  import { onMount } from 'svelte';
  import { getCurrentUser, logout } from './api/auth';
  import { ApiError } from './api/http';
  import type { AuthenticatedUser } from './api/types';
  import SessionTabsBar from './components/SessionTabsBar.svelte';
  import SessionTreeSidebar from './components/SessionTreeSidebar.svelte';
  import ToastHost from './components/ToastHost.svelte';
  import { parseRoute, routeHref, type AppRoute } from './lib/router';
  import { MAX_OPEN_TABS, parseSavedTabs, serializeTabs, TAB_STORAGE_KEY, type PersistedSessionTab } from './lib/sessionTabs';
  import { pushToast } from './lib/toasts';
  import ChatRoute from './routes/ChatRoute.svelte';
  import LoginRoute from './routes/LoginRoute.svelte';

  let route: AppRoute = parseRoute(window.location.hash);
  let user: AuthenticatedUser | null = null;
  let authLoaded = false;
  let tabs: PersistedSessionTab[] = [];
  let activeSessionId = '';

  interface TabMetaUpdate {
    sessionId: string;
    title: string;
    status?: string;
    workspaceId?: string;
    providerId?: string;
  }

  function loadTabs() {
    if (!window.localStorage) {
      return;
    }
    try {
      tabs = parseSavedTabs(window.localStorage.getItem(TAB_STORAGE_KEY));
    } catch {
      tabs = [];
    }
  }

  function persistTabs() {
    if (!window.localStorage) {
      return;
    }
    try {
      window.localStorage.setItem(TAB_STORAGE_KEY, serializeTabs(tabs));
    } catch {
      // Ignore quota and private storage failures.
    }
  }

  function touchTab(sessionId: string) {
    if (!sessionId) {
      return;
    }
    activeSessionId = sessionId;
  }

  function ensureTab(sessionId: string) {
    if (!sessionId) {
      return;
    }

    const exists = tabs.find((tab) => tab.sessionId === sessionId);
    if (!exists) {
      if (tabs.length >= MAX_OPEN_TABS) {
        tabs = tabs.slice(tabs.length - (MAX_OPEN_TABS - 1));
      }
      tabs = [...tabs, { sessionId, title: sessionId }];
      persistTabs();
    }
    touchTab(sessionId);
  }

  function syncRoute() {
    route = parseRoute(window.location.hash);
    if (route.name === 'chat') {
      ensureTab(route.sessionId);
      return;
    }
    activeSessionId = '';
  }

  function afterLogin(nextUser: AuthenticatedUser) {
    user = nextUser;
    window.location.hash = '#/';
  }

  async function signOut() {
    try {
      await logout();
      user = null;
      window.location.hash = '/login';
    } catch {
      pushToast({ severity: 'error', message: 'Could not sign out. Check the control plane and try again.' });
    }
  }

  function activateTab(sessionId: string) {
    window.location.hash = routeHref({
      name: 'chat',
      sessionId
    }).slice(1);
  }

  function closeTab(sessionId: string) {
    const closedAt = tabs.findIndex((tab) => tab.sessionId === sessionId);
    if (closedAt === -1) {
      return;
    }

    const wasActive = sessionId === activeSessionId;
    const nextTabs = tabs.filter((tab) => tab.sessionId !== sessionId);
    tabs = nextTabs;
    persistTabs();

    if (!wasActive) {
      return;
    }

    const fallbackTab = nextTabs[closedAt - 1] ?? nextTabs[closedAt];
    if (fallbackTab) {
      activateTab(fallbackTab.sessionId);
      return;
    }
    window.location.hash = '#/';
  }

  function updateTabMeta(detail: TabMetaUpdate) {
    const index = tabs.findIndex((tab) => tab.sessionId === detail.sessionId);
    if (index === -1) {
      return;
    }
    const normalizedTitle = detail.title?.trim() || detail.sessionId;
    const next: PersistedSessionTab = {
      ...tabs[index],
      title: normalizedTitle
    };
    if (detail.status) {
      next.status = detail.status;
    }
    if (detail.workspaceId) {
      next.workspaceId = detail.workspaceId;
    }
    if (detail.providerId) {
      next.providerId = detail.providerId;
    }
    if (
      tabs[index].title === next.title &&
      tabs[index].status === next.status &&
      tabs[index].workspaceId === next.workspaceId &&
      tabs[index].providerId === next.providerId
    ) {
      return;
    }
    tabs = [...tabs.slice(0, index), next, ...tabs.slice(index + 1)];
    persistTabs();
  }

  onMount(() => {
    window.addEventListener('hashchange', syncRoute);
    const showRuntimeError = () => {
      pushToast({ severity: 'error', message: 'Unexpected frontend error. The app shell is still running.' });
    };
    const showUnhandledRejection = () => {
      pushToast({ severity: 'error', message: 'Unexpected async error. The app shell is still running.' });
    };
    window.addEventListener('error', showRuntimeError);
    window.addEventListener('unhandledrejection', showUnhandledRejection);

    loadTabs();
    syncRoute();

    getCurrentUser()
      .then((nextUser) => {
        user = nextUser;
        if (route.name === 'login') {
          window.location.hash = '#/';
        }
      })
      .catch((err) => {
        if (!(err instanceof ApiError && err.status === 401)) {
          console.error(err);
          pushToast({ severity: 'error', message: 'Could not load the current user.' });
        }
        if (route.name !== 'login') {
          window.location.hash = '/login';
        }
      })
      .finally(() => {
        authLoaded = true;
      });

    return () => {
      window.removeEventListener('hashchange', syncRoute);
      window.removeEventListener('error', showRuntimeError);
      window.removeEventListener('unhandledrejection', showUnhandledRejection);
    };
  });
</script>

{#if !authLoaded}
  <main class="boot-screen">Loading Agenter...</main>
{:else if !user || route.name === 'login'}
  <LoginRoute onLogin={afterLogin} />
{:else}
  <div class="app-shell" class:mobile-chat={route.name === 'chat'}>
    <SessionTreeSidebar {user} {route} onSignOut={signOut} />

    <main class="content" class:with-tabs={route.name === 'chat'}>
      {#if route.name === 'chat'}
        <SessionTabsBar
          tabs={tabs}
          activeSessionId={activeSessionId}
          onActivateTab={activateTab}
          onCloseTab={closeTab}
        />
        <div class="chat-route-stack">
          {#each tabs as tab (tab.sessionId)}
            <div class="chat-route-slot" class:hidden={tab.sessionId !== activeSessionId}>
              <ChatRoute
                sessionId={tab.sessionId}
                on:sessionMeta={(event) => updateTabMeta(event.detail)}
              />
            </div>
          {/each}
        </div>
      {:else}
        <section class="content-home empty-state" aria-label="Home">
          <p class="muted">Choose a session in the sidebar, or start one with <strong>+</strong> next to a workspace.</p>
        </section>
      {/if}
    </main>
  </div>
{/if}

<ToastHost />
