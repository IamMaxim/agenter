<script lang="ts">
  import AgenterIcon from './AgenterIcon.svelte';

  interface SessionTabSummary {
    sessionId: string;
    title: string;
  }

  export let tabs: SessionTabSummary[] = [];
  export let activeSessionId = '';
  export let onActivateTab: (sessionId: string) => void;
  export let onCloseTab: (sessionId: string) => void;

  function activate(sessionId: string) {
    onActivateTab(sessionId);
  }

  function close(sessionId: string, event: Event) {
    event.stopPropagation();
    onCloseTab(sessionId);
  }

  function onTabKeydown(event: KeyboardEvent, sessionId: string) {
    if (event.key === 'Enter' || event.key === ' ') {
      event.preventDefault();
      activate(sessionId);
    }
  }
</script>

<div class="session-tabs" aria-label="Open sessions">
  {#each tabs as tab (tab.sessionId)}
    <button
      type="button"
      class="session-tab"
      class:active={tab.sessionId === activeSessionId}
      aria-pressed={tab.sessionId === activeSessionId}
      on:click={() => activate(tab.sessionId)}
      on:keydown={(event) => onTabKeydown(event, tab.sessionId)}
      title={tab.title || tab.sessionId}
    >
      <span class="session-tab-title">{tab.title || tab.sessionId}</span>
      <span
        class="session-tab-close"
        role="button"
        tabindex="0"
        aria-label={`Close ${tab.title || tab.sessionId}`}
        on:click={(event) => close(tab.sessionId, event)}
        on:keydown={(event) => {
          if (event.key === 'Enter' || event.key === ' ') {
            event.preventDefault();
            close(tab.sessionId, event);
          }
        }}
      >
        <AgenterIcon name="x" size={11} />
      </span>
    </button>
  {:else}
    <span class="session-tabs-empty">Open sessions to start</span>
  {/each}
</div>
