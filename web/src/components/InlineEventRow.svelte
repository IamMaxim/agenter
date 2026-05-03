<script lang="ts">
  import AgenterIcon from './AgenterIcon.svelte';
  import AnsiBlock from './AnsiBlock.svelte';
  import type { ChatItem } from '../lib/chatEvents';

  type IconName =
    | 'bash'
    | 'checklist'
    | 'delete'
    | 'edit'
    | 'file'
    | 'git'
    | 'globe'
    | 'list'
    | 'search'
    | 'spark'
    | 'test'
    | 'generic';

  export let item: Extract<ChatItem, { kind: 'inlineEvent' }>;
  export let expandedByDefault = false;

  let expanded = expandedByDefault;
  let userExpanded = false;

  $: if (!userExpanded) {
    expanded = expandedByDefault;
  }

  $: commandActions = item.eventKind === 'command' ? (item.actions ?? []) : [];
  $: hasCommandMeta =
    item.eventKind === 'command' &&
    (item.exitCode !== undefined || item.durationMs !== undefined || item.processId || item.source);
  $: hasDetail = Boolean(
    item.detail || ('output' in item && item.output) || commandActions.length > 0 || hasCommandMeta
  );
  $: rowKind = toolKind(item.eventKind, item.title);
  $: iconName = toolIconName(rowKind, item.title);
  $: statusTone = item.success === false ? 'error' : rowStatusTone(item.status);
  $: showStatusGlyph = statusTone === 'error' || statusTone === 'waiting' || isActiveToolRow(item.eventKind, item.status);

  function toggle() {
    if (hasDetail) {
      userExpanded = true;
      expanded = !expanded;
    }
  }

  function onKeydown(event: KeyboardEvent) {
    if (event.key === 'Enter' || event.key === ' ') {
      event.preventDefault();
      toggle();
    }
  }

  function toolKind(eventKind: string, title: string) {
    const value = `${eventKind} ${title}`.toLowerCase();
    if (value.includes('read')) return 'read';
    if (value.includes('write')) return 'write';
    if (value.includes('edit') || value.includes('patch')) return 'edit';
    if (value.includes('delete') || value.includes('remove')) return 'delete';
    if (value.includes('glob')) return 'glob';
    if (value.includes('grep') || value.includes('search')) return 'grep';
    if (value.includes('test')) return 'test';
    if (value.includes('git')) return 'git';
    if (eventKind === 'command') return 'bash';
    if (eventKind === 'file') return 'edit';
    return eventKind;
  }

  function toolIconName(kind: string, title: string): IconName {
    const value = `${kind} ${title}`.toLowerCase();
    if (kind === 'bash') return 'bash';
    if (kind === 'git') return 'git';
    if (kind === 'test') return 'test';
    if (kind === 'edit') return 'edit';
    if (kind === 'delete') return 'delete';
    if (kind === 'glob') return 'list';
    if (kind === 'grep' || value.includes('search')) return 'search';
    if (kind === 'read' || kind === 'write' || kind === 'file') return 'file';
    if (value.includes('fetch') || value.includes('web')) return 'globe';
    if (value.includes('plan')) return 'checklist';
    if (value.includes('think')) return 'spark';
    return 'generic';
  }

  function isActiveToolRow(eventKind: string, status: string) {
    return (eventKind === 'command' || eventKind === 'tool') && (status === 'running' || status === 'started');
  }

  function rowStatusTone(status: string) {
    if (status === 'running' || status === 'started') return 'running';
    if (status === 'waiting' || status === 'proposed') return 'waiting';
    if (status === 'failed' || status === 'error' || status === 'rejected') return 'error';
    if (status === 'completed' || status === 'applied') return 'done';
    return 'ok';
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
    <span class="inline-event-chevron" aria-hidden="true"><AgenterIcon name="chevron" size={14} /></span>
    <span class="tool-icon" data-kind={rowKind} aria-hidden="true"><AgenterIcon name={iconName} /></span>
    {#if showStatusGlyph}
      <span class:error={statusTone === 'error'} class:running={isActiveToolRow(item.eventKind, item.status)} class:waiting={statusTone === 'waiting'} class="tool-status-glyph" aria-hidden="true"></span>
    {/if}
    <span class="inline-event-kind">{rowKind}</span>
    <span class="inline-event-title">{item.title}</span>
    <span class="spacer"></span>
    {#if item.durationMs !== undefined}
      <span class="inline-event-meta">{item.durationMs}ms</span>
    {:else if item.exitCode !== undefined}
      <span class="inline-event-meta">exit {item.exitCode}</span>
    {:else}
      <span class:failed={item.success === false} class="inline-event-status">{item.status}</span>
    {/if}
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
        <div class="tool-detail-code"><AnsiBlock content={item.detail} /></div>
      {/if}
      {#if 'output' in item && item.output}
        <div class="tool-detail-code"><AnsiBlock content={item.output} /></div>
      {/if}
    </div>
  {/if}
</article>
