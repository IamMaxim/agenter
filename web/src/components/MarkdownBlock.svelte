<script lang="ts">
  import { renderMarkdown } from '../lib/markdown';

  export let content = '';
</script>

<div class="markdown-content">
  <!-- eslint-disable-next-line svelte/no-at-html-tags -- sanitized by renderMarkdown -->
  {@html renderMarkdown(content)}
</div>
