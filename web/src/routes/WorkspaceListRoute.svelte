<script lang="ts">
  import { onMount } from 'svelte';
  import { listRunners, listRunnerWorkspaces } from '../api/sessions';
  import type { RunnerInfo, WorkspaceRef } from '../api/types';

  let runners: RunnerInfo[] = [];
  let workspacesByRunner: Record<string, WorkspaceRef[]> = {};
  let loading = true;
  let error = '';

  onMount(async () => {
    try {
      runners = await listRunners();
      const workspacePairs = await Promise.all(
        runners.map(async (runner) => [
          runner.runner_id,
          await listRunnerWorkspaces(runner.runner_id)
        ] as const)
      );
      workspacesByRunner = Object.fromEntries(workspacePairs);
    } catch {
      error = 'Could not load workspaces.';
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
  {:else if runners.length === 0}
    <div class="empty-state">
      <strong>No runners registered</strong>
      <span>Start a runner to advertise workspace directories.</span>
    </div>
  {:else}
    <div class="data-list">
      {#each runners as runner}
        <article class="row-card">
          <div>
            <strong>{runner.name}</strong>
            <span>{runner.status ?? 'registered'} · {runner.runner_id}</span>
          </div>
          <code>{workspacesByRunner[runner.runner_id]?.length ?? 0} workspaces</code>
        </article>
        {#each workspacesByRunner[runner.runner_id] ?? [] as workspace}
          <article class="row-card inset">
            <div>
              <strong>{workspace.display_name ?? workspace.path}</strong>
              <span>{workspace.path}</span>
            </div>
            <code>{workspace.workspace_id}</code>
          </article>
        {/each}
      {/each}
    </div>
  {/if}
</section>
