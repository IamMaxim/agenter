<script lang="ts">
  import MarkdownBlock from './MarkdownBlock.svelte';
  import type { ChatItem } from '../lib/chatEvents';

  export let item: Extract<ChatItem, { kind: 'plan' }>;
</script>

<article class="plan-card">
  <div class="card-heading">
    <span>Implementation Plan</span>
    <code>interactive</code>
  </div>
  <strong>{item.title}</strong>
  <MarkdownBlock content={item.content} />
</article>
