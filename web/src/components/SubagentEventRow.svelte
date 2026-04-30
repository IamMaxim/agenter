<script lang="ts">
  import MarkdownBlock from './MarkdownBlock.svelte';
  import type { ChatItem } from '../lib/chatEvents';

  export let item: Extract<ChatItem, { kind: 'subagent' }>;

  let expanded = false;
  let providerExpanded = false;

  $: agentLabel =
    item.agentIds.length === 0
      ? 'No agent result'
      : item.agentIds.length === 1
        ? item.agentIds[0]
        : `${item.agentIds.length} subagents`;
  $: hasDetail = Boolean(
    item.prompt || item.model || item.reasoningEffort || item.states.length > 0 || item.agentIds.length > 0
  );
  $: providerPayload = item.providerPayload
    ? JSON.stringify(item.providerPayload, null, 2)
    : undefined;

  function toggle() {
    if (hasDetail) {
      expanded = !expanded;
    }
  }
</script>

<article class:expanded class:has-detail={hasDetail} class="subagent-event">
  <button
    aria-expanded={expanded}
    class="subagent-summary"
    disabled={!hasDetail}
    type="button"
    on:click={toggle}
  >
    <span class="subagent-kind">subagent</span>
    <span class="subagent-title">{item.title}</span>
    <span class="subagent-agent">{agentLabel}</span>
    <span class="inline-event-chevron" aria-hidden="true">›</span>
    <span class="spacer"></span>
    <span class:failed={item.status === 'failed'} class="inline-event-status">{item.status}</span>
  </button>

  {#if expanded && hasDetail}
    <div class="subagent-detail">
      {#if item.model || item.reasoningEffort}
        <div class="subagent-badges">
          {#if item.model}
            <span>{item.model}</span>
          {/if}
          {#if item.reasoningEffort}
            <span>{item.reasoningEffort}</span>
          {/if}
        </div>
      {/if}

      {#if item.prompt}
        <section class="subagent-section">
          <span>Prompt</span>
          <div class="subagent-prompt">{item.prompt}</div>
        </section>
      {/if}

      {#if item.states.length > 0}
        <section class="subagent-section">
          <span>Agents</span>
          <div class="subagent-state-list">
            {#each item.states as state}
              <div class="subagent-state">
                <div class="subagent-state-header">
                  <code>{state.agentId}</code>
                  <span class:failed={state.status === 'failed'}>{state.status}</span>
                </div>
                {#if state.message}
                  <MarkdownBlock content={state.message} />
                {/if}
              </div>
            {/each}
          </div>
        </section>
      {:else if item.operation === 'wait'}
        <p class="subagent-empty">No completed subagent result yet.</p>
      {/if}

      {#if providerPayload}
        <button
          class="subagent-provider-toggle"
          type="button"
          on:click={() => (providerExpanded = !providerExpanded)}
        >
          Provider payload
        </button>
        {#if providerExpanded}
          <pre>{providerPayload}</pre>
        {/if}
      {/if}
    </div>
  {/if}
</article>
