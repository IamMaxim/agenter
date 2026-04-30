<script lang="ts">
  import { onMount } from 'svelte';
  import { ApiError } from '../api/http';
  import { listWorkspaces } from '../api/sessions';
  import type { WorkspaceRef } from '../api/types';

  let workspaces: WorkspaceRef[] = [];
  let loading = true;
  let unavailable = false;
  let error = '';

  onMount(async () => {
    try {
      workspaces = await listWorkspaces();
    } catch (err) {
      unavailable = err instanceof ApiError && err.status === 404;
      error = unavailable ? '' : 'Could not load workspaces.';
    } finally {
      loading = false;
    }
  });
</script>

<section class="page-section">
  <div class="section-header">
    <div>
      <h1>Workspaces</h1>
      <p>Runner-advertised directories available for agent sessions.</p>
    </div>
  </div>

  {#if loading}
    <p class="muted">Loading workspaces...</p>
  {:else if error}
    <p class="error" role="alert">{error}</p>
  {:else if unavailable}
    <div class="empty-state">
      <strong>Workspace API pending</strong>
      <span>The frontend route is ready; backend list endpoints arrive in the next browser MVP tasks.</span>
    </div>
  {:else if workspaces.length === 0}
    <div class="empty-state">
      <strong>No workspaces registered</strong>
      <span>Start a runner to advertise workspace directories.</span>
    </div>
  {:else}
    <div class="data-list">
      {#each workspaces as workspace}
        <article class="row-card">
          <div>
            <strong>{workspace.display_name ?? workspace.path}</strong>
            <span>{workspace.path}</span>
          </div>
          <code>{workspace.runner_id}</code>
        </article>
      {/each}
    </div>
  {/if}
</section>
