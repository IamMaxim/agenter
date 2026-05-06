<script lang="ts">
  import MarkdownBlock from './MarkdownBlock.svelte';
  import type { ChatItem } from '../lib/chatEvents';

  export let item: Extract<ChatItem, { kind: 'plan' }>;
  export let pendingHandoff = false;
  export let turnActive = false;
  export let defaultModeAvailable = true;
  export let onImplement: (() => void) | undefined = undefined;
  export let onClearContextImplement: (() => void) | undefined = undefined;
  export let onStayInPlan: (() => void) | undefined = undefined;

  $: handoffDisabled = turnActive || !defaultModeAvailable;
  $: handoffDisabledReason = !defaultModeAvailable
    ? 'Default mode unavailable'
    : turnActive
      ? 'Wait for the current turn to finish'
      : '';
  $: handoffState = item.handoff?.state ?? 'available';
  $: handoffAvailable = handoffState === 'available';
  $: handoffStatusLabel =
    handoffState === 'implementing'
      ? 'Implementation started'
      : handoffState === 'implemented'
        ? 'Implementation completed'
        : handoffState === 'dismissed'
          ? 'Plan handoff dismissed'
          : '';
</script>

<article class="plan-card">
  <div class="card-heading">
    <span>Implementation Plan</span>
    <code>interactive</code>
  </div>
  <strong>{item.title}</strong>
  <MarkdownBlock content={item.content} />
  {#if item.entries && item.entries.length > 0}
    <ol class="plan-entry-list">
      {#each item.entries as entry}
        <li class:done={entry.status === 'completed'} class:active={entry.status === 'in_progress' || entry.status === 'implementing'} class:error={entry.status === 'failed' || entry.status === 'cancelled'}>
          <span>{entry.label}</span>
          <code>{entry.status}</code>
        </li>
      {/each}
    </ol>
  {/if}
  {#if pendingHandoff && handoffAvailable}
    <div class="plan-handoff" role="group" aria-label="Implement this plan?">
      <span class="plan-handoff-title">Implement this plan?</span>
      <div class="plan-handoff-actions">
        <button
          type="button"
          disabled={handoffDisabled}
          title={handoffDisabledReason}
          on:click={() => onImplement?.()}
        >
          Implement plan
        </button>
        <button
          class="secondary compact"
          type="button"
          disabled={handoffDisabled}
          title={handoffDisabledReason}
          on:click={() => onClearContextImplement?.()}
        >
          Implement in fresh thread
        </button>
        <button
          class="secondary compact"
          type="button"
          disabled={turnActive}
          on:click={() => onStayInPlan?.()}
        >
          Stay in Plan mode
        </button>
      </div>
    </div>
  {:else if pendingHandoff && handoffStatusLabel}
    <div class="plan-handoff status" role="status">
      <span class="plan-handoff-title">{handoffStatusLabel}</span>
    </div>
  {/if}
</article>

<style>
  .plan-handoff {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    margin-top: 0.75rem;
    padding-top: 0.75rem;
    border-top: 1px solid var(--surface-border, rgba(255, 255, 255, 0.08));
  }

  .plan-handoff-title {
    font-size: 0.85rem;
    opacity: 0.85;
  }

  .plan-handoff-actions {
    display: flex;
    flex-wrap: wrap;
    gap: 0.5rem;
  }

  .plan-entry-list {
    display: grid;
    gap: 0.35rem;
    margin: 0.75rem 0 0;
    padding-left: 1.25rem;
  }

  .plan-entry-list li {
    padding-left: 0.25rem;
  }

  .plan-entry-list li::marker {
    color: var(--text-muted, rgba(255, 255, 255, 0.55));
  }

  .plan-entry-list li.done span {
    text-decoration: line-through;
    opacity: 0.75;
  }

  .plan-entry-list li.active span {
    font-weight: 600;
  }

  .plan-entry-list li.error span {
    color: var(--danger, #ff8a8a);
  }

  .plan-entry-list code {
    margin-left: 0.4rem;
    opacity: 0.7;
  }
</style>
