<script lang="ts">
  export let name:
    | 'attach'
    | 'bash'
    | 'checklist'
    | 'chevron'
    | 'delete'
    | 'edit'
    | 'file'
    | 'git'
    | 'globe'
    | 'list'
    | 'plus'
    | 'search'
    | 'spark'
    | 'subagent'
    | 'test'
    | 'x'
    | 'generic' = 'generic';
  export let size = 12;
  export let decorative = true;
</script>

<svg
  aria-hidden={decorative}
  aria-label={decorative ? undefined : name}
  class="agenter-icon"
  width={size}
  height={size}
  viewBox="0 0 12 12"
  fill="none"
  stroke="currentColor"
  stroke-width="1.2"
  stroke-linecap="round"
  stroke-linejoin="round"
>
  {#if name === 'attach'}
    <path d="M8.6 3.7 4.7 7.6a1.6 1.6 0 0 0 2.3 2.3l3.5-3.5a2.8 2.8 0 0 0-4-4L2.7 6.2a3.8 3.8 0 0 0 5.4 5.4" />
  {:else if name === 'bash'}
    <path d="M2 3.2 4.8 6 2 8.8" />
    <path d="M5.4 9h4.4" />
  {:else if name === 'checklist'}
    <path d="M2 3.2 2.8 4 4.2 2.4" />
    <path d="M2 6.2 2.8 7 4.2 5.4" />
    <path d="M2 9.2 2.8 10 4.2 8.4" />
    <path d="M5.8 3.5h4" />
    <path d="M5.8 6.5h4" />
    <path d="M5.8 9.5h4" />
  {:else if name === 'chevron'}
    <path d="M4.8 3.2 7.1 6 4.8 8.8" />
  {:else if name === 'delete'}
    <path d="M2.5 3h7" />
    <path d="M4.5 3V2h3v1" />
    <path d="M3.5 3v7a.7.7 0 0 0 .7.7h3.6a.7.7 0 0 0 .7-.7V3" />
  {:else if name === 'edit'}
    <path d="m2 9.7.5-1.9 5.8-5.8 1.7 1.7-5.8 5.8-2.2.2z" />
    <path d="m7.5 2.8 1.7 1.7" />
  {:else if name === 'file'}
    <path d="M2.5 1.5h4L9.5 4.5v6a.5.5 0 0 1-.5.5H3a.5.5 0 0 1-.5-.5v-9z" />
    <path d="M6.5 1.5v3h3" />
  {:else if name === 'git'}
    <circle cx="3.2" cy="3" r="1" />
    <circle cx="3.2" cy="9" r="1" />
    <circle cx="8.8" cy="6" r="1" />
    <path d="M3.2 4v4" />
    <path d="M4.2 3h2.4c.9 0 1.4.5 1.4 1.4V5" />
  {:else if name === 'globe'}
    <circle cx="6" cy="6" r="4.5" />
    <path d="M1.7 6h8.6" />
    <path d="M6 1.5c1.2 1.2 1.8 2.7 1.8 4.5S7.2 9.3 6 10.5C4.8 9.3 4.2 7.8 4.2 6S4.8 2.7 6 1.5z" />
  {:else if name === 'list'}
    <path d="M2.2 3h7.6" />
    <path d="M2.2 6h7.6" />
    <path d="M2.2 9h4.8" />
  {:else if name === 'plus'}
    <path d="M6 2.5v7" />
    <path d="M2.5 6h7" />
  {:else if name === 'search'}
    <circle cx="5" cy="5" r="3.2" />
    <path d="m7.4 7.4 2.4 2.4" />
  {:else if name === 'spark'}
    <circle cx="6" cy="5.2" r="3.2" />
    <path d="m4.9 9.1-.4 1.4" />
    <path d="m7.1 9.1.4 1.4" />
  {:else if name === 'subagent'}
    <path d="M4.5 3.5 2 6l2.5 2.5" />
    <path d="M7.5 3.5 10 6 7.5 8.5" />
    <path d="M5.2 9.5 6.8 2.5" />
  {:else if name === 'test'}
    <path d="M4 1.5v3L2 8.7a1.2 1.2 0 0 0 1.1 1.8h5.8A1.2 1.2 0 0 0 10 8.7L8 4.5v-3" />
    <path d="M3.5 1.5h5" />
    <path d="M3.4 7.5h5.2" />
  {:else if name === 'x'}
    <path d="M3 3l6 6" />
    <path d="M9 3 3 9" />
  {:else}
    <circle cx="6" cy="6" r="4" />
    <circle cx="6" cy="6" r="1.1" />
  {/if}
</svg>
