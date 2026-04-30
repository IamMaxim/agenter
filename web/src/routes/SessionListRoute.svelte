<script lang="ts">
  import { onMount } from 'svelte';
  import { ApiError } from '../api/http';
  import { listSessions } from '../api/sessions';
  import type { SessionInfo } from '../api/types';
  import { routeHref } from '../lib/router';

  let sessions: SessionInfo[] = [];
  let loading = true;
  let unavailable = false;
  let error = '';

  onMount(async () => {
    try {
      sessions = await listSessions();
    } catch (err) {
      unavailable = err instanceof ApiError && err.status === 404;
      error = unavailable ? '' : 'Could not load sessions.';
    } finally {
      loading = false;
    }
  });
</script>

<section class="page-section">
  <div class="section-header">
    <div>
      <h1>Sessions</h1>
      <p>Persistent agent conversations across browser and future connector projections.</p>
    </div>
    <button type="button" disabled>New session</button>
  </div>

  {#if loading}
    <p class="muted">Loading sessions...</p>
  {:else if error}
    <p class="error" role="alert">{error}</p>
  {:else if unavailable}
    <div class="empty-state">
      <strong>Session API pending</strong>
      <span>The route and API client are scaffolded; Task 2.3 will wire creation and chat UX.</span>
    </div>
  {:else if sessions.length === 0}
    <div class="empty-state">
      <strong>No sessions yet</strong>
      <span>Create a session once backend session endpoints are enabled.</span>
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
