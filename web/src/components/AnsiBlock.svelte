<script lang="ts">
  import { ansiToHtml } from '../lib/ansi';

  export let content = '';
</script>

<!-- eslint-disable svelte/no-at-html-tags -- ansiToHtml escapes all text before adding spans -->
<pre class="ansi-block"><code>{@html ansiToHtml(content)}</code></pre>
<!-- eslint-enable svelte/no-at-html-tags -->
