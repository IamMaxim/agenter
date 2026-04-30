<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import { connectSessionEvents, type BrowserEventSocket } from '../api/events';
  import { sendSessionMessage } from '../api/sessions';
  import type { AppEvent, BrowserServerMessage } from '../api/types';

  export let sessionId: string;

  let socket: BrowserEventSocket | undefined;
  let events: AppEvent[] = [];
  let connectionState = 'Connecting';
  let draft = '';
  let sendError = '';

  onMount(() => {
    socket = connectSessionEvents(sessionId, {
      onOpen: () => {
        connectionState = 'Subscribed';
      },
      onMessage: (message: BrowserServerMessage) => {
        if (message.type === 'app_event') {
          events = [...events, message.event];
        }
        if (message.type === 'error') {
          connectionState = message.message;
        }
      },
      onClose: () => {
        connectionState = 'Disconnected';
      },
      onError: () => {
        connectionState = 'Connection error';
      }
    });
  });

  onDestroy(() => {
    socket?.close();
  });

  async function submit() {
    const content = draft.trim();
    if (!content) {
      return;
    }

    sendError = '';
    try {
      await sendSessionMessage(sessionId, { content });
      draft = '';
    } catch {
      sendError = 'Sending messages is reserved for the next browser UX task.';
    }
  }
</script>

<section class="chat-layout">
  <header class="chat-header">
    <div>
      <h1>Session</h1>
      <p>{sessionId}</p>
    </div>
    <span class="status-pill">{connectionState}</span>
  </header>

  <div class="event-stream">
    {#if events.length === 0}
      <div class="empty-state">
        <strong>No events received</strong>
        <span>Connect a runner and subscribe to an accessible session to stream normalized events.</span>
      </div>
    {:else}
      {#each events as event}
        <article class="event-card">
          <span>{event.type}</span>
          <pre>{JSON.stringify(event.payload, null, 2)}</pre>
        </article>
      {/each}
    {/if}
  </div>

  <form class="composer" on:submit|preventDefault={submit}>
    <label class="sr-only" for="message">Message</label>
    <textarea id="message" bind:value={draft} rows="3" placeholder="Message the agent"></textarea>
    <button type="submit">Send</button>
  </form>
  {#if sendError}
    <p class="error" role="alert">{sendError}</p>
  {/if}
</section>
