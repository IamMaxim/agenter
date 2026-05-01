<script lang="ts">
  import { onMount } from 'svelte';
  import { getCurrentUser, logout } from './api/auth';
  import { ApiError } from './api/http';
  import type { AuthenticatedUser } from './api/types';
  import SessionTreeSidebar from './components/SessionTreeSidebar.svelte';
  import ToastHost from './components/ToastHost.svelte';
  import { parseRoute, type AppRoute } from './lib/router';
  import { pushToast } from './lib/toasts';
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
    try {
      await logout();
      user = null;
      window.location.hash = '/login';
    } catch {
      pushToast({ severity: 'error', message: 'Could not sign out. Check the control plane and try again.' });
    }
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
  <div class="app-shell">
    <SessionTreeSidebar {user} {route} onSignOut={signOut} />

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

<ToastHost />
