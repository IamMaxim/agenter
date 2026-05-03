<script lang="ts">
  import AgenterIcon from './AgenterIcon.svelte';
  import { FALLBACK_TAB_TITLE } from '../lib/sessionTabs';

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

  function tabLabel(title: string) {
    return title.trim() || FALLBACK_TAB_TITLE;
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
      title={tabLabel(tab.title)}
    >
      <span class="session-tab-title">{tabLabel(tab.title)}</span>
      <span
        class="session-tab-close"
        role="button"
        tabindex="0"
        aria-label={`Close ${tabLabel(tab.title)}`}
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
