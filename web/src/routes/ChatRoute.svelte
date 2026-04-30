<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import { connectSessionEvents, type BrowserEventSocket } from '../api/events';
  import {
    decideApproval,
    getSession,
    getSessionHistory,
    sendSessionMessage
  } from '../api/sessions';
  import type { BrowserServerMessage, SessionInfo } from '../api/types';
  import {
    applyChatEnvelope,
    createChatState,
    type ChatItem,
    type ChatState
  } from '../lib/chatEvents';
  import { routeHref } from '../lib/router';

  export let sessionId: string;

  let socket: BrowserEventSocket | undefined;
  let reconnectTimer: ReturnType<typeof setTimeout> | undefined;
  let closedByRoute = false;
  let chatState: ChatState = createChatState();
  let session: SessionInfo | undefined;
  let connectionState = 'Connecting';
  let draft = '';
  let sendError = '';
  let decisionError = '';

  $: items = chatState.items;

  onMount(() => {
    void reloadAndConnect();
  });

  onDestroy(() => {
    closedByRoute = true;
    if (reconnectTimer) {
      clearTimeout(reconnectTimer);
    }
    socket?.close();
  });

  async function reloadAndConnect() {
    socket?.close();
    connectionState = 'Loading history';
    try {
      session = await getSession(sessionId);
      const history = await getSessionHistory(sessionId);
      let nextState = createChatState();
      for (const envelope of history) {
        nextState = applyChatEnvelope(nextState, envelope);
      }
      chatState = nextState;
    } catch {
      connectionState = 'History unavailable';
    }

    if (closedByRoute) {
      return;
    }

    connectionState = 'Connecting';
    socket = connectSessionEvents(sessionId, {
      onOpen: () => {
        connectionState = 'Subscribed';
      },
      onMessage: (message: BrowserServerMessage) => {
        if (message.type === 'app_event') {
          chatState = applyChatEnvelope(chatState, message);
        }
        if (message.type === 'error') {
          connectionState = message.message;
        }
      },
      onClose: () => {
        if (!closedByRoute) {
          connectionState = 'Reconnecting';
          reconnectTimer = setTimeout(() => void reloadAndConnect(), 900);
        }
      },
      onError: () => {
        connectionState = 'Connection error';
      }
    });
  }

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
      sendError = 'Could not send message. Check that the runner is connected.';
    }
  }

  async function resolveApproval(item: ChatItem, decision: 'accept' | 'decline') {
    if (item.kind !== 'approval' || item.resolvedDecision) {
      return;
    }

    decisionError = '';
    try {
      const envelope = await decideApproval(item.approvalId, { decision });
      chatState = applyChatEnvelope(chatState, envelope);
    } catch {
      decisionError = 'Could not resolve approval.';
    }
  }
</script>

<section class="chat-layout">
  <header class="chat-header">
    <div>
      <a class="back-link" href={routeHref({ name: 'sessions' })}>Sessions</a>
      <h1>{session?.title ?? 'Session'}</h1>
      <p>{sessionId}</p>
    </div>
    <span class="status-pill">{connectionState}</span>
  </header>

  <div class="event-stream">
    {#if items.length === 0}
      <div class="empty-state">
        <strong>No events yet</strong>
        <span>Send a message or wait for the connected runner to stream normalized events.</span>
      </div>
    {:else}
      {#each items as item (item.id)}
        {#if item.kind === 'user'}
          <article class="message-row user-message">
            <span>You</span>
            <p>{item.content}</p>
          </article>
        {:else if item.kind === 'assistant'}
          <article class="message-row assistant-message">
            <span>Agent</span>
            <p>{item.content}</p>
          </article>
        {:else if item.kind === 'approval'}
          <article class="event-card approval-card">
            <div class="card-heading">
              <span>Approval</span>
              {#if item.resolvedDecision}
                <code>{item.resolvedDecision}</code>
              {/if}
            </div>
            <strong>{item.title}</strong>
            {#if item.detail}
              <pre>{item.detail}</pre>
            {/if}
            {#if !item.resolvedDecision}
              <div class="inline-actions">
                <button type="button" on:click={() => resolveApproval(item, 'accept')}>Accept</button>
                <button class="secondary compact" type="button" on:click={() => resolveApproval(item, 'decline')}>
                  Decline
                </button>
              </div>
            {/if}
          </article>
        {:else if item.kind === 'command'}
          <article class="event-card command-card">
            <div class="card-heading">
              <span>Command</span>
              <code>{item.status}{item.success === false ? ' failed' : ''}</code>
            </div>
            <strong>{item.title}</strong>
            {#if item.detail}
              <code>{item.detail}</code>
            {/if}
            {#if item.output}
              <pre>{item.output}</pre>
            {/if}
          </article>
        {:else if item.kind === 'file'}
          <article class="event-card file-card">
            <div class="card-heading">
              <span>File</span>
              <code>{item.status}</code>
            </div>
            <strong>{item.title}</strong>
            {#if item.detail}
              <pre>{item.detail}</pre>
            {/if}
          </article>
        {:else if item.kind === 'tool'}
          <article class="event-card tool-card">
            <div class="card-heading">
              <span>Tool</span>
              <code>{item.status}</code>
            </div>
            <strong>{item.title}</strong>
            {#if item.detail}
              <pre>{item.detail}</pre>
            {/if}
          </article>
        {:else if item.kind === 'error'}
          <article class="event-card error-card">
            <span>Error</span>
            <strong>{item.title}</strong>
            {#if item.detail}
              <code>{item.detail}</code>
            {/if}
          </article>
        {:else if item.kind === 'event'}
          <article class="event-card">
            <span>{item.title}</span>
            {#if item.detail}
              <pre>{item.detail}</pre>
            {/if}
          </article>
        {/if}
      {/each}
    {/if}
  </div>

  <form class="composer" on:submit|preventDefault={submit}>
    <label class="sr-only" for="message">Message</label>
    <textarea id="message" bind:value={draft} rows="3" placeholder="Message the agent"></textarea>
    <button type="submit">Send</button>
  </form>
  {#if sendError || decisionError}
    <p class="error" role="alert">{sendError || decisionError}</p>
  {/if}
</section>
