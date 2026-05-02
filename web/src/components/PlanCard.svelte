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
</script>

<article class="plan-card">
  <div class="card-heading">
    <span>Implementation Plan</span>
    <code>interactive</code>
  </div>
  <strong>{item.title}</strong>
  <MarkdownBlock content={item.content} />
  {#if pendingHandoff}
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
</style>
