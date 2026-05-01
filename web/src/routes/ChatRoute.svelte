<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import InlineEventRow from '../components/InlineEventRow.svelte';
  import MarkdownBlock from '../components/MarkdownBlock.svelte';
  import PlanCard from '../components/PlanCard.svelte';
  import SubagentEventRow from '../components/SubagentEventRow.svelte';
  import { connectSessionEvents, type BrowserEventSocket } from '../api/events';
  import {
    answerQuestion,
    decideApproval,
    getSessionAgentOptions,
    getSessionSettings,
    getSession,
    getSessionHistory,
    sendSessionMessage,
    updateSessionSettings
  } from '../api/sessions';
  import type {
    AgentOptions,
    AgentQuestionAnswer,
    AgentQuestionField,
    AgentReasoningEffort,
    AgentTurnSettings,
    BrowserServerMessage,
    SessionInfo
  } from '../api/types';
  import {
    effortsForSelectedModel as effortsForModel,
    normalizeAgentOptions,
    normalizeTurnSettings
  } from '../lib/normalizers';
  import { pushToast } from '../lib/toasts';
  import {
    applyChatEnvelope,
    createChatState,
    type ChatItem,
    type ChatState
  } from '../lib/chatEvents';

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
  let settingsError = '';
  let settingsOpen = false;
  let agentOptions: AgentOptions = { models: [], collaboration_modes: [] };
  let turnSettings: AgentTurnSettings = {};
  let questionDrafts: Record<string, Record<string, string[]>> = {};

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
      void loadAgentSettings();
      const history = await getSessionHistory(sessionId);
      let nextState = createChatState();
      for (const envelope of history) {
        try {
          nextState = applyChatEnvelope(nextState, envelope);
        } catch {
          pushToast({ severity: 'warning', message: 'Skipped a malformed history event.' });
        }
      }
      chatState = nextState;
    } catch {
      connectionState = 'History unavailable';
      pushToast({ severity: 'error', message: 'Could not load session history.' });
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
          try {
            chatState = applyChatEnvelope(chatState, message);
          } catch {
            pushToast({ severity: 'warning', message: 'Skipped a malformed live event.' });
          }
        }
        if (message.type === 'error') {
          connectionState = message.message;
          pushToast({ severity: 'error', message: message.message });
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
        pushToast({ severity: 'error', message: 'Browser event stream encountered an error.' });
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
      pushToast({ severity: 'error', message: sendError });
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
      pushToast({ severity: 'error', message: decisionError });
    }
  }

  async function loadAgentSettings() {
    settingsError = '';
    try {
      const [options, settings] = await Promise.all([
        getSessionAgentOptions(sessionId),
        getSessionSettings(sessionId)
      ]);
      agentOptions = normalizeAgentOptions(options);
      turnSettings = normalizeTurnSettings(settings);
    } catch {
      settingsError = 'Agent options are unavailable.';
      agentOptions = { models: [], collaboration_modes: [] };
      pushToast({ severity: 'warning', message: 'Agent options are unavailable. Using defaults.' });
    }
  }

  async function saveSettings(next: AgentTurnSettings) {
    turnSettings = next;
    settingsError = '';
    try {
      turnSettings = await updateSessionSettings(sessionId, next);
    } catch {
      settingsError = 'Could not save agent settings.';
      pushToast({ severity: 'error', message: settingsError });
    }
  }

  function setModel(model: string) {
    const selected = agentOptions.models.find((option) => option.id === model);
    void saveSettings({
      ...turnSettings,
      model: model || null,
      reasoning_effort:
        selected?.default_reasoning_effort ?? turnSettings.reasoning_effort ?? null
    });
  }

  function setReasoningEffort(reasoning_effort: string) {
    void saveSettings({
      ...turnSettings,
      reasoning_effort: (reasoning_effort || null) as AgentReasoningEffort | null
    });
  }

  function setCollaborationMode(collaboration_mode: string) {
    const mode = agentOptions.collaboration_modes.find((option) => option.id === collaboration_mode);
    void saveSettings({
      ...turnSettings,
      collaboration_mode: collaboration_mode || null,
      model: mode?.model ?? turnSettings.model ?? null,
      reasoning_effort: mode?.reasoning_effort ?? turnSettings.reasoning_effort ?? null
    });
  }

  function effortsForSelectedModel(): string[] {
    return effortsForModel(agentOptions, turnSettings);
  }

  function questionAnswers(item: ChatItem, field: AgentQuestionField): string[] {
    if (item.kind !== 'question') {
      return [];
    }
    return questionDrafts[item.questionId]?.[field.id] ?? field.default_answers ?? [];
  }

  function setQuestionAnswers(questionId: string, fieldId: string, answers: string[]) {
    questionDrafts = {
      ...questionDrafts,
      [questionId]: {
        ...(questionDrafts[questionId] ?? {}),
        [fieldId]: answers
      }
    };
  }

  function toggleQuestionChoice(item: ChatItem, field: AgentQuestionField, value: string, checked: boolean) {
    if (item.kind !== 'question') {
      return;
    }
    const current = questionAnswers(item, field);
    const answers = checked ? [...new Set([...current, value])] : current.filter((answer) => answer !== value);
    setQuestionAnswers(item.questionId, field.id, answers);
  }

  async function submitQuestion(item: ChatItem) {
    if (item.kind !== 'question' || item.answered) {
      return;
    }
    const answers: AgentQuestionAnswer = {
      question_id: item.questionId,
      answers: Object.fromEntries(
        item.fields.map((field) => [field.id, questionAnswers(item, field)])
      )
    };
    decisionError = '';
    try {
      const envelope = await answerQuestion(item.questionId, answers);
      chatState = applyChatEnvelope(chatState, envelope);
    } catch {
      decisionError = 'Could not answer question.';
      pushToast({ severity: 'error', message: decisionError });
    }
  }
</script>

<section class="chat-layout">
  <header class="chat-header">
    <span class="chat-title">{session?.title ?? 'New session'}</span>
    <div>
      <!-- <a class="back-link" href={routeHref({ name: 'sessions' })}>Sessions</a> -->
      <!-- <p>{sessionId}</p> -->
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
            <!-- <span>You</span> -->
            <MarkdownBlock content={item.content} />
          </article>
        {:else if item.kind === 'assistant'}
          <article class="message-row assistant-message">
            <!-- <span>Agent</span> -->
            <MarkdownBlock content={item.content} />
          </article>
        {:else if item.kind === 'inlineEvent'}
          <InlineEventRow {item} />
        {:else if item.kind === 'subagent'}
          <SubagentEventRow {item} />
        {:else if item.kind === 'plan'}
          <PlanCard {item} />
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
        {:else if item.kind === 'question'}
          <article class="event-card question-card">
            <div class="card-heading">
              <span>Question</span>
              {#if item.answered}
                <code>answered</code>
              {/if}
            </div>
            <strong>{item.title}</strong>
            {#if item.description}
              <p>{item.description}</p>
            {/if}
            {#each item.fields as field}
              <label class="question-field">
                <span>{field.label}</span>
                {#if field.prompt}
                  <small>{field.prompt}</small>
                {/if}
                {#if field.kind === 'multi_select'}
                  <div class="choice-list">
                    {#each field.choices as choice}
                      <label>
                        <input
                          type="checkbox"
                          disabled={item.answered}
                          checked={questionAnswers(item, field).includes(choice.value)}
                          on:change={(event) =>
                            toggleQuestionChoice(item, field, choice.value, event.currentTarget.checked)}
                        />
                        <span>{choice.label}</span>
                      </label>
                    {/each}
                  </div>
                {:else if field.kind === 'single_select' && field.choices.length > 0}
                  <select
                    disabled={item.answered}
                    value={questionAnswers(item, field)[0] ?? ''}
                    on:change={(event) => setQuestionAnswers(item.questionId, field.id, [event.currentTarget.value])}
                  >
                    <option value="">Choose...</option>
                    {#each field.choices as choice}
                      <option value={choice.value}>{choice.label}</option>
                    {/each}
                  </select>
                {:else}
                  <input
                    type={field.secret ? 'password' : field.kind === 'number' ? 'number' : 'text'}
                    disabled={item.answered}
                    value={questionAnswers(item, field)[0] ?? ''}
                    on:input={(event) => setQuestionAnswers(item.questionId, field.id, [event.currentTarget.value])}
                  />
                {/if}
              </label>
            {/each}
            {#if !item.answered}
              <div class="inline-actions">
                <button type="button" on:click={() => submitQuestion(item)}>Answer</button>
              </div>
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
        {/if}
      {/each}
    {/if}
  </div>

  <form class="composer" on:submit|preventDefault={submit}>
    <div class="composer-settings">
      <button class="secondary compact" type="button" on:click={() => (settingsOpen = !settingsOpen)}>
        Settings
      </button>
      {#if settingsOpen}
        <div class="settings-popover">
          <label>
            <span>Model</span>
            <select value={turnSettings.model ?? ''} on:change={(event) => setModel(event.currentTarget.value)}>
              <option value="">Default</option>
              {#each agentOptions.models as model}
                <option value={model.id}>{model.display_name}</option>
              {/each}
            </select>
          </label>
          <label>
            <span>Mode</span>
            <select
              value={turnSettings.collaboration_mode ?? ''}
              on:change={(event) => setCollaborationMode(event.currentTarget.value)}
            >
              <option value="">Default</option>
              {#each agentOptions.collaboration_modes as mode}
                <option value={mode.id}>{mode.label}</option>
              {/each}
            </select>
          </label>
          <label>
            <span>Reasoning</span>
            <select
              value={turnSettings.reasoning_effort ?? ''}
              on:change={(event) => setReasoningEffort(event.currentTarget.value)}
            >
              <option value="">Default</option>
              {#each effortsForSelectedModel() as effort}
                <option value={effort}>{effort}</option>
              {/each}
            </select>
          </label>
          {#if settingsError}
            <small class="error">{settingsError}</small>
          {/if}
        </div>
      {/if}
    </div>
    <label class="sr-only" for="message">Message</label>
    <textarea id="message" bind:value={draft} rows="1" placeholder="Message the agent"></textarea>
    <button type="submit">Send</button>
  </form>
  {#if sendError || decisionError}
    <p class="error" role="alert">{sendError || decisionError}</p>
  {/if}
</section>
