<script lang="ts">
  import { onMount } from 'svelte';
  import { getCurrentUser, logout } from './api/auth';
  import { ApiError } from './api/http';
  import type { AuthenticatedUser } from './api/types';
  import { parseRoute, routeHref, type AppRoute } from './lib/router';
  import ChatRoute from './routes/ChatRoute.svelte';
  import LoginRoute from './routes/LoginRoute.svelte';
  import SessionListRoute from './routes/SessionListRoute.svelte';
  import WorkspaceListRoute from './routes/WorkspaceListRoute.svelte';

  let route: AppRoute = parseRoute(window.location.hash);
  let user: AuthenticatedUser | null = null;
  let authLoaded = false;

  function syncRoute() {
    route = parseRoute(window.location.hash);
  }

  function afterLogin(nextUser: AuthenticatedUser) {
    user = nextUser;
    window.location.hash = '/sessions';
  }

  async function signOut() {
    await logout();
    user = null;
    window.location.hash = '/login';
  }

  onMount(() => {
    window.addEventListener('hashchange', syncRoute);

    getCurrentUser()
      .then((nextUser) => {
        user = nextUser;
        if (route.name === 'login') {
          window.location.hash = '/sessions';
        }
      })
      .catch((err) => {
        if (!(err instanceof ApiError && err.status === 401)) {
          console.error(err);
        }
        if (route.name !== 'login') {
          window.location.hash = '/login';
        }
      })
      .finally(() => {
        authLoaded = true;
      });

    return () => window.removeEventListener('hashchange', syncRoute);
  });
</script>

{#if !authLoaded}
  <main class="boot-screen">Loading Agenter...</main>
{:else if !user || route.name === 'login'}
  <LoginRoute onLogin={afterLogin} />
{:else}
  <div class="app-shell">
    <aside class="sidebar">
      <div class="brand">
        <strong>Agenter</strong>
        <span>{user.display_name ?? user.email}</span>
      </div>
      <nav>
        <a class:active={route.name === 'sessions' || route.name === 'chat'} href={routeHref({ name: 'sessions' })}>
          Sessions
        </a>
        <a class:active={route.name === 'workspaces'} href={routeHref({ name: 'workspaces' })}>
          Workspaces
        </a>
      </nav>
      <button class="secondary" type="button" on:click={signOut}>Sign out</button>
    </aside>

    <main class="content">
      {#if route.name === 'workspaces'}
        <WorkspaceListRoute />
      {:else if route.name === 'chat'}
        <ChatRoute sessionId={route.sessionId} />
      {:else}
        <SessionListRoute />
      {/if}
    </main>
  </div>
{/if}
