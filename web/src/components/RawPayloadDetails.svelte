<script lang="ts">
  import AnsiBlock from './AnsiBlock.svelte';

  export let payload: unknown;
  export let summary = 'Raw payload';

  $: rawPayload =
    payload !== undefined
      ? JSON.stringify(payload, null, 2)
      : undefined;
</script>

{#if rawPayload}
  <details class="tool-detail-raw">
    <summary>{summary}</summary>
    <div class="tool-detail-code"><AnsiBlock content={rawPayload} /></div>
  </details>
{/if}
