<script lang="ts">
  import { dismissToast, toasts } from '../lib/toasts';
</script>

{#if $toasts.length > 0}
  <section class="toast-host" aria-label="Notifications">
    {#each $toasts as toast (toast.id)}
      <article class:error={toast.severity === 'error'} class:warning={toast.severity === 'warning'} class="toast">
        <span>{toast.message}</span>
        <button type="button" aria-label="Dismiss notification" on:click={() => dismissToast(toast.id)}>
          x
        </button>
      </article>
    {/each}
  </section>
{/if}
