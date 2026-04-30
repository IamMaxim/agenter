<script lang="ts">
  import type { ChatItem } from '../lib/chatEvents';

  export let item: Extract<ChatItem, { kind: 'inlineEvent' }>;

  let expanded = false;

  $: commandActions = item.eventKind === 'command' ? (item.actions ?? []) : [];
  $: hasCommandMeta =
    item.eventKind === 'command' &&
    (item.exitCode !== undefined || item.durationMs !== undefined || item.processId || item.source);
  $: hasDetail = Boolean(
    item.detail || ('output' in item && item.output) || commandActions.length > 0 || hasCommandMeta
  );

  function toggle() {
    if (hasDetail) {
      expanded = !expanded;
    }
  }

  function onKeydown(event: KeyboardEvent) {
    if (event.key === 'Enter' || event.key === ' ') {
      event.preventDefault();
      toggle();
    }
  }
</script>

<article class:expanded class:has-detail={hasDetail} class="inline-event">
  <button
    aria-expanded={expanded}
    class="inline-event-summary"
    disabled={!hasDetail}
    type="button"
    on:click={toggle}
    on:keydown={onKeydown}
  >
    <span class="inline-event-kind">{item.eventKind}</span>
    <span class="inline-event-title">{item.title}</span>
    <span class="inline-event-chevron" aria-hidden="true">›</span>
    <span class="spacer"></span>
    <span class:failed={item.success === false} class="inline-event-status">{item.status}</span>
  </button>

  {#if expanded && hasDetail}
    <div class="inline-event-detail">
      {#if hasCommandMeta}
        <div class="command-meta">
          {#if item.exitCode !== undefined}
            <span>exit {item.exitCode}</span>
          {/if}
          {#if item.durationMs !== undefined}
            <span>{item.durationMs}ms</span>
          {/if}
          {#if item.processId}
            <span>pid {item.processId}</span>
          {/if}
          {#if item.source}
            <span>{item.source}</span>
          {/if}
        </div>
      {/if}
      {#if commandActions.length > 0}
        <div class="command-actions">
          {#each commandActions as action}
            <div class="command-action">
              <span>{action.label}</span>
              {#if action.detail}
                <code>{action.detail}</code>
              {/if}
            </div>
          {/each}
        </div>
      {/if}
      {#if item.detail}
        <pre>{item.detail}</pre>
      {/if}
      {#if 'output' in item && item.output}
        <pre>{item.output}</pre>
      {/if}
    </div>
  {/if}
</article>
