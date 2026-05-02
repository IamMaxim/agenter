<script lang="ts">
  import { onDestroy, onMount, tick } from 'svelte';
  import InlineEventRow from '../components/InlineEventRow.svelte';
  import MarkdownBlock from '../components/MarkdownBlock.svelte';
  import PlanCard from '../components/PlanCard.svelte';
  import SubagentEventRow from '../components/SubagentEventRow.svelte';
  import { ApiError } from '../api/http';
  import { connectSessionEvents, type BrowserEventSocket } from '../api/events';
  import {
    answerQuestion,
    createSession,
    decideApproval,
    getSessionAgentOptions,
    getSessionSettings,
    getSession,
    getSessionHistory,
    listSlashCommands,
    renameSession,
    executeSlashCommand,
    sendSessionMessage,
    updateSessionSettings
  } from '../api/sessions';
  import { routeHref } from '../lib/router';
  import type {
    AgentOptions,
    AgentQuestionAnswer,
    AgentQuestionField,
    AgentReasoningEffort,
    AgentTurnSettings,
    BrowserServerMessage,
    SessionInfo,
    SessionUsageWindow,
    SlashCommandDefinition,
    SlashCommandRequest
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
  import {
    filterSlashCommands,
    isSlashDraft,
    needsSlashConfirmation,
    parseSlashCommand,
    slashRequest,
    slashResultMessage
  } from '../lib/slashCommands';

  export let sessionId: string;

  let socket: BrowserEventSocket | undefined;
  let reconnectTimer: ReturnType<typeof setTimeout> | undefined;
  let closedByRoute = false;
  let mounted = false;
  let activeSessionId = '';
  let connectionGeneration = 0;
  let chatState: ChatState = createChatState();
  let session: SessionInfo | undefined;
  let connectionState = 'Connecting';
  let draft = '';
  let sendError = '';
  let decisionError = '';
  let settingsError = '';
  let submitting = false;
  let editingTitle = false;
  let titleDraft = '';
  let renaming = false;
  let messageTextarea: HTMLTextAreaElement | undefined;
  let agentOptions: AgentOptions = { models: [], collaboration_modes: [] };
  let turnSettings: AgentTurnSettings = {};
  let questionDrafts: Record<string, Record<string, string[]>> = {};
  let slashCommands: SlashCommandDefinition[] = [];
  let slashSelectionIndex = 0;
  let pendingDangerCommand:
    | { command: SlashCommandDefinition; request: SlashCommandRequest }
    | undefined;
  let dismissedPlanIds: Set<string> = new Set();

  // Verbatim copy of Codex TUI's PLAN_IMPLEMENTATION_CLEAR_CONTEXT_PREFIX
  // (`tmp/codex/codex-rs/tui/src/chatwidget/plan_implementation.rs`). Mirrors
  // the exact wording so the model interprets a fresh-thread implementation
  // request the same way Codex does.
  const PLAN_IMPLEMENTATION_CLEAR_CONTEXT_PREFIX =
    "A previous agent produced the plan below to accomplish the user's task. " +
    'Implement the plan in a fresh context. Treat the plan as the source of ' +
    'user intent, re-read files as needed, and carry the work through ' +
    'implementation and verification.';
  const PLAN_IMPLEMENTATION_CODING_MESSAGE = 'Implement the plan.';

  $: items = chatState.items;
  $: turnActivity = chatState.activity;
  $: turnActive = turnActivity?.active ?? false;
  $: sendBusy = submitting || turnActive;
  $: slashSuggestions = filterSlashCommands(draft, slashCommands);
  $: parsedSlash = parseSlashCommand(draft, slashCommands);
  $: slashMenuOpen = isSlashDraft(draft) && slashSuggestions.length > 0;
  $: usage = session?.usage ?? null;
  $: defaultModeId = resolveDefaultModeId(agentOptions.collaboration_modes);
  $: defaultModeAvailable = defaultModeId !== null;
  $: modeValue =
    turnSettings.collaboration_mode ?? usage?.mode_label ?? defaultModeId ?? '';
  $: latestPlanId = chatState.latestPlanId;
  $: planTurnComplete = chatState.planTurnComplete;
  $: modelValue = turnSettings.model ?? usage?.model ?? '';
  $: reasoningValue = turnSettings.reasoning_effort ?? usage?.reasoning_effort ?? '';
  $: contextLabel = percentLabel(usage?.context?.used_percent);
  $: contextTitle = tokenUsageTitle(usage?.context?.used_tokens, usage?.context?.total_tokens);
  $: window5hLabel = `5h ${percentLabel(usage?.window_5h?.remaining_percent)}`;
  $: window5hTitle = resetTitle(usage?.window_5h);
  $: weekLabel = `w ${percentLabel(usage?.week?.remaining_percent)}`;
  $: weekTitle = resetTitle(usage?.week);
  $: if (slashSelectionIndex >= slashSuggestions.length) {
    slashSelectionIndex = 0;
  }
  $: if (mounted && sessionId !== activeSessionId) {
    activeSessionId = sessionId;
    void reloadAndConnect();
  }

  onMount(() => {
    mounted = true;
    activeSessionId = sessionId;
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
    const generation = ++connectionGeneration;
    socket?.close();
    if (reconnectTimer) {
      clearTimeout(reconnectTimer);
      reconnectTimer = undefined;
    }
    chatState = createChatState();
    dismissedPlanIds = new Set();
    session = undefined;
    sendError = '';
    decisionError = '';
    settingsError = '';
    editingTitle = false;
    connectionState = 'Loading history';
    try {
      session = await getSession(sessionId);
      if (generation !== connectionGeneration) {
        return;
      }
      titleDraft = session.title ?? '';
      void loadAgentSettings(generation);
      void loadSlashCommands(generation);
      const history = await getSessionHistory(sessionId);
      if (generation !== connectionGeneration) {
        return;
      }
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
        if (generation !== connectionGeneration) {
          return;
        }
        connectionState = 'Subscribed';
      },
      onMessage: (message: BrowserServerMessage) => {
        if (generation !== connectionGeneration) {
          return;
        }
        if (message.type === 'app_event') {
          try {
            chatState = applyChatEnvelope(chatState, message);
            if (
              message.event.type === 'provider_event' &&
              ['token_usage', 'rate_limits'].includes(String(message.event.payload.category ?? ''))
            ) {
              void refreshSessionUsage(generation);
            }
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
        if (!closedByRoute && generation === connectionGeneration) {
          connectionState = 'Reconnecting';
          reconnectTimer = setTimeout(() => void reloadAndConnect(), 900);
        }
      },
      onError: () => {
        if (generation !== connectionGeneration) {
          return;
        }
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

    if (isSlashDraft(draft)) {
      await submitSlashCommand();
      return;
    }

    sendError = '';
    submitting = true;
    try {
      await sendSessionMessage(sessionId, { content });
      draft = '';
      await tick();
      resizeMessageTextarea();
    } catch (error) {
      sendError = apiErrorMessage(error, 'Could not send message. Check that the runner is connected.');
      pushToast({ severity: 'error', message: sendError });
    } finally {
      submitting = false;
    }
  }

  async function loadSlashCommands(generation = connectionGeneration) {
    try {
      const commands = await listSlashCommands(sessionId);
      if (generation !== connectionGeneration) {
        return;
      }
      slashCommands = commands;
      slashSelectionIndex = 0;
    } catch {
      if (generation !== connectionGeneration) {
        return;
      }
      slashCommands = [];
      pushToast({ severity: 'warning', message: 'Slash commands are unavailable.' });
    }
  }

  async function submitSlashCommand(confirmed = false) {
    const parsed = parseSlashCommand(draft, slashCommands);
    if (!parsed.command) {
      sendError = parsed.error ?? 'Unknown slash command.';
      pushToast({ severity: 'error', message: sendError });
      return;
    }
    if (parsed.missingRequired.length > 0) {
      sendError = `Missing argument: ${parsed.missingRequired.join(', ')}.`;
      pushToast({ severity: 'warning', message: sendError });
      return;
    }
    const request = slashRequest(draft, parsed.command, confirmed);
    if (
      needsSlashConfirmation(parsed.command) &&
      !confirmed &&
      pendingDangerCommand?.request.raw_input !== request.raw_input
    ) {
      pendingDangerCommand = { command: parsed.command, request };
      sendError = '';
      return;
    }
    await runSlashCommand({ ...request, confirmed: confirmed || needsSlashConfirmation(parsed.command) });
  }

  async function runSlashCommand(request: SlashCommandRequest) {
    sendError = '';
    submitting = true;
    try {
      const result = await executeSlashCommand(sessionId, request);
      if (result.session) {
        session = result.session;
        titleDraft = session.title ?? '';
        window.dispatchEvent(new CustomEvent('agenter:sessions-changed'));
      }
      draft = '';
      pendingDangerCommand = undefined;
      pushToast({
        severity: result.accepted ? 'info' : 'warning',
        message: slashResultMessage(result)
      });
      await tick();
      resizeMessageTextarea();
    } catch (error) {
      sendError = apiErrorMessage(error, 'Could not execute slash command.');
      pushToast({ severity: 'error', message: sendError });
    } finally {
      submitting = false;
    }
  }

  function apiErrorMessage(error: unknown, fallback: string): string {
    if (error instanceof ApiError) {
      return error.detail ?? error.message;
    }
    return fallback;
  }

  async function saveTitle() {
    if (!session || renaming) {
      return;
    }
    renaming = true;
    try {
      session = await renameSession(session.session_id, {
        title: titleDraft.trim() || null
      });
      titleDraft = session.title ?? '';
      editingTitle = false;
      window.dispatchEvent(new CustomEvent('agenter:sessions-changed'));
    } catch {
      pushToast({ severity: 'error', message: 'Could not rename session.' });
    } finally {
      renaming = false;
    }
  }

  function cancelTitleEdit() {
    titleDraft = session?.title ?? '';
    editingTitle = false;
  }

  function handleTitleKeydown(event: KeyboardEvent) {
    if (event.key === 'Enter') {
      event.preventDefault();
      void saveTitle();
    }
    if (event.key === 'Escape') {
      event.preventDefault();
      cancelTitleEdit();
    }
  }

  function handleMessageKeydown(event: KeyboardEvent) {
    if (slashMenuOpen) {
      if (event.key === 'ArrowDown') {
        event.preventDefault();
        slashSelectionIndex = (slashSelectionIndex + 1) % slashSuggestions.length;
        return;
      }
      if (event.key === 'ArrowUp') {
        event.preventDefault();
        slashSelectionIndex =
          (slashSelectionIndex - 1 + slashSuggestions.length) % slashSuggestions.length;
        return;
      }
      if (event.key === 'Tab') {
        event.preventDefault();
        selectSlashCommand(slashSuggestions[slashSelectionIndex]);
        return;
      }
      if (
        event.key === 'Enter' &&
        !event.shiftKey &&
        !event.isComposing &&
        parsedSlash.command?.id !== slashSuggestions[slashSelectionIndex]?.id
      ) {
        event.preventDefault();
        selectSlashCommand(slashSuggestions[slashSelectionIndex]);
        return;
      }
      if (event.key === 'Escape') {
        event.preventDefault();
        slashSelectionIndex = 0;
        return;
      }
    }
    if (event.key === 'Enter' && !event.shiftKey && !event.isComposing) {
      event.preventDefault();
      void submit();
    }
  }

  function selectSlashCommand(command: SlashCommandDefinition) {
    draft = `/${command.name}${command.arguments.length > 0 ? ' ' : ''}`;
    slashSelectionIndex = 0;
    pendingDangerCommand = undefined;
    void tick().then(() => {
      messageTextarea?.focus();
      resizeMessageTextarea();
    });
  }

  function argumentHint(command: SlashCommandDefinition): string {
    return command.arguments
      .map((argument) => `${argument.required ? '<' : '['}${argument.name}${argument.required ? '>' : ']'}`)
      .join(' ');
  }

  function confirmPendingDangerCommand() {
    if (!pendingDangerCommand) {
      return;
    }
    void runSlashCommand({ ...pendingDangerCommand.request, confirmed: true });
  }

  function resizeMessageTextarea() {
    if (!messageTextarea) {
      return;
    }
    messageTextarea.style.height = 'auto';
    const lineHeight = Number.parseFloat(getComputedStyle(messageTextarea).lineHeight) || 20;
    const maxHeight = lineHeight * 20 + 24;
    const nextHeight = Math.min(messageTextarea.scrollHeight, maxHeight);
    messageTextarea.style.height = `${nextHeight}px`;
    messageTextarea.style.overflowY = messageTextarea.scrollHeight > maxHeight ? 'auto' : 'hidden';
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

  async function loadAgentSettings(generation = connectionGeneration) {
    settingsError = '';
    try {
      const [options, settings] = await Promise.all([
        getSessionAgentOptions(sessionId),
        getSessionSettings(sessionId)
      ]);
      if (generation !== connectionGeneration) {
        return;
      }
      agentOptions = normalizeAgentOptions(options);
      turnSettings = normalizeTurnSettings(settings);
    } catch {
      if (generation !== connectionGeneration) {
        return;
      }
      settingsError = 'Agent options are unavailable.';
      agentOptions = { models: [], collaboration_modes: [] };
      pushToast({ severity: 'warning', message: 'Agent options are unavailable. Using defaults.' });
    }
  }

  async function refreshSessionUsage(generation = connectionGeneration) {
    try {
      const refreshed = await getSession(sessionId);
      if (generation !== connectionGeneration) {
        return;
      }
      session = refreshed;
      titleDraft = session.title ?? '';
    } catch {
      // Usage metrics are informational; retain the last-known snapshot across transient refresh errors.
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

  function resolveDefaultModeId(
    modes: AgentOptions['collaboration_modes']
  ): string | null {
    if (modes.length === 0) {
      return null;
    }
    const explicit = modes.find((mode) => mode.id === 'default');
    if (explicit) {
      return explicit.id;
    }
    return modes[0]?.id ?? null;
  }

  async function handleImplementPlan() {
    if (!session) {
      return;
    }
    if (!defaultModeId) {
      sendError = 'Default mode unavailable.';
      pushToast({ severity: 'error', message: sendError });
      return;
    }
    const override: AgentTurnSettings = {
      ...turnSettings,
      collaboration_mode: defaultModeId
    };
    turnSettings = override;
    sendError = '';
    submitting = true;
    try {
      await sendSessionMessage(sessionId, {
        content: PLAN_IMPLEMENTATION_CODING_MESSAGE,
        settings_override: override
      });
    } catch (error) {
      sendError = apiErrorMessage(error, 'Could not implement plan.');
      pushToast({ severity: 'error', message: sendError });
    } finally {
      submitting = false;
    }
  }

  async function handleClearContextImplement(planContent: string) {
    if (!session) {
      return;
    }
    if (!defaultModeId) {
      sendError = 'Default mode unavailable.';
      pushToast({ severity: 'error', message: sendError });
      return;
    }
    const trimmed = planContent.trim();
    if (!trimmed) {
      sendError = 'No approved plan available.';
      pushToast({ severity: 'error', message: sendError });
      return;
    }
    sendError = '';
    submitting = true;
    try {
      const newSession = await createSession({
        workspace_id: session.workspace_id,
        provider_id: session.provider_id,
        title: session.title ?? undefined,
        initial_message: `${PLAN_IMPLEMENTATION_CLEAR_CONTEXT_PREFIX}\n\n${trimmed}`,
        settings_override: { collaboration_mode: defaultModeId }
      });
      window.dispatchEvent(new CustomEvent('agenter:sessions-changed'));
      window.location.hash = routeHref({
        name: 'chat',
        sessionId: newSession.session_id
      }).slice(1);
    } catch (error) {
      sendError = apiErrorMessage(
        error,
        'Could not start a fresh thread to implement the plan.'
      );
      pushToast({ severity: 'error', message: sendError });
    } finally {
      submitting = false;
    }
  }

  function handleStayInPlan(planId: string) {
    dismissedPlanIds = new Set([...dismissedPlanIds, planId]);
  }

  function setCollaborationMode(collaboration_mode: string) {
    const resolvedId = collaboration_mode || defaultModeId;
    if (!resolvedId) {
      return;
    }
    const mode = agentOptions.collaboration_modes.find((option) => option.id === resolvedId);
    void saveSettings({
      ...turnSettings,
      collaboration_mode: resolvedId,
      model: mode?.model ?? turnSettings.model ?? null,
      reasoning_effort: mode?.reasoning_effort ?? turnSettings.reasoning_effort ?? null
    });
  }

  function effortsForSelectedModel(): string[] {
    return effortsForModel(agentOptions, turnSettings);
  }

  function percentLabel(value: number | null | undefined): string {
    return typeof value === 'number' && Number.isFinite(value) ? `${Math.round(value)}%` : '--';
  }

  function tokenUsageTitle(used: number | null | undefined, total: number | null | undefined): string | undefined {
    if (used === null || used === undefined || total === null || total === undefined) {
      return undefined;
    }
    return `${formatTokenCount(used)}/${formatTokenCount(total)}`;
  }

  function formatTokenCount(value: number): string {
    if (value >= 1_000_000) {
      return `${Math.round(value / 100_000) / 10}M`;
    }
    if (value >= 1_000) {
      return `${Math.round(value / 1_000)}K`;
    }
    return `${Math.round(value)}`;
  }

  function resetTitle(window: SessionUsageWindow | null | undefined): string | undefined {
    if (!window?.resets_at) {
      return undefined;
    }
    const reset = new Date(window.resets_at);
    if (Number.isNaN(reset.getTime())) {
      return undefined;
    }
    return `Resets in ${relativeReset(reset)}; ${localResetTime(reset)}`;
  }

  function relativeReset(reset: Date): string {
    const minutes = Math.max(0, Math.round((reset.getTime() - Date.now()) / 60_000));
    if (minutes < 60) {
      return `${minutes}min`;
    }
    const hours = Math.round(minutes / 60);
    if (hours < 48) {
      return `${hours}h`;
    }
    return `${Math.round(hours / 24)}d`;
  }

  function localResetTime(reset: Date): string {
    const now = new Date();
    const sameYear = reset.getFullYear() === now.getFullYear();
    const sameDay = reset.toDateString() === now.toDateString();
    if (sameDay) {
      return reset.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
    }
    return reset.toLocaleString([], {
      month: 'short',
      day: 'numeric',
      hour: '2-digit',
      minute: '2-digit',
      ...(sameYear ? {} : { year: 'numeric' })
    });
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
    {#if editingTitle}
      <div class="title-editor">
        <input
          aria-label="Session title"
          bind:value={titleDraft}
          disabled={renaming}
          on:keydown={handleTitleKeydown}
        />
        <button class="secondary compact" disabled={renaming} type="button" on:click={saveTitle}>
          Save
        </button>
        <button class="secondary compact" disabled={renaming} type="button" on:click={cancelTitleEdit}>
          Cancel
        </button>
      </div>
    {:else}
      <button class="chat-title-button" type="button" on:click={() => (editingTitle = true)}>
        <span class="chat-title">{session?.title ?? 'New session'}</span>
      </button>
    {/if}
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
          <PlanCard
            {item}
            pendingHandoff={item.id === latestPlanId &&
              planTurnComplete &&
              !dismissedPlanIds.has(item.id)}
            {turnActive}
            {defaultModeAvailable}
            onImplement={() => handleImplementPlan()}
            onClearContextImplement={() => handleClearContextImplement(item.content)}
            onStayInPlan={() => handleStayInPlan(item.id)}
          />
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
              <details>
                <summary>Details</summary>
                <pre>{item.detail}</pre>
              </details>
            {/if}
          </article>
        {/if}
      {/each}
    {/if}
    {#if turnActive}
      <div class="working-row" aria-live="polite">
        <span class="working-dot"></span>
        <span>{turnActivity?.label ?? 'Working'}</span>
        <span class="working-shimmer"></span>
      </div>
    {/if}
  </div>

  <form class="composer" on:submit|preventDefault={submit}>
    {#if slashMenuOpen}
      <div class="slash-menu">
        {#each slashSuggestions as command, index (command.id)}
          <button
            class:selected={index === slashSelectionIndex}
            type="button"
            on:click={() => selectSlashCommand(command)}
          >
            <span class="slash-command-name">/{command.name}</span>
            {#if command.provider_id}
              <code>{command.provider_id}</code>
            {/if}
            {#if command.danger_level !== 'safe'}
              <strong>{command.danger_level}</strong>
            {/if}
            <span>{command.description}</span>
            {#if argumentHint(command)}
              <small>{argumentHint(command)}</small>
            {/if}
          </button>
        {/each}
      </div>
    {/if}
    {#if pendingDangerCommand}
      <div class="slash-confirm">
        <span>
          Confirm /{pendingDangerCommand.command.name}: {pendingDangerCommand.command.description}
        </span>
        <button
          class="danger compact"
          type="button"
          on:click={confirmPendingDangerCommand}
        >
          Run
        </button>
        <button class="secondary compact" type="button" on:click={() => (pendingDangerCommand = undefined)}>
          Cancel
        </button>
      </div>
    {/if}
    <label class="sr-only" for="message">Message</label>
    <textarea
      id="message"
      bind:this={messageTextarea}
      bind:value={draft}
      rows="1"
      placeholder="Message the agent"
      on:input={resizeMessageTextarea}
      on:keydown={handleMessageKeydown}
    ></textarea>
    <button class:busy={sendBusy} type="submit" disabled={submitting} aria-label={sendBusy ? 'Working' : 'Send'}>
      {#if sendBusy}
        <span class="button-spinner" aria-hidden="true"></span>
      {:else}
        Send
      {/if}
    </button>
    <div class="composer-bottom-bar">
      <select
        aria-label="Collaboration mode"
        class="composer-inline-select"
        value={modeValue}
        disabled={agentOptions.collaboration_modes.length === 0}
        on:change={(event) => setCollaborationMode(event.currentTarget.value)}
      >
        {#if agentOptions.collaboration_modes.length === 0}
          <option value="">default</option>
        {:else}
          {#each agentOptions.collaboration_modes as mode}
            <option value={mode.id}>{mode.label}</option>
          {/each}
        {/if}
      </select>
      <span class="composer-dot" aria-hidden="true">·</span>
      <select
        aria-label="Model"
        class="composer-inline-select model-select"
        value={modelValue}
        on:change={(event) => setModel(event.currentTarget.value)}
      >
        <option value="">model</option>
        {#each agentOptions.models as model}
          <option value={model.id}>{model.display_name}</option>
        {/each}
      </select>
      <select
        aria-label="Thinking level"
        class="composer-inline-select"
        value={reasoningValue}
        on:change={(event) => setReasoningEffort(event.currentTarget.value)}
      >
        <option value="">thinking</option>
        {#each effortsForSelectedModel() as effort}
          <option value={effort}>{effort}</option>
        {/each}
      </select>
      <span class="composer-dot" aria-hidden="true">·</span>
      <span
        class:unknown={contextLabel === '--'}
        class="composer-metric"
        title={contextTitle}
      >
        {contextLabel}
      </span>
      <span class="composer-spacer"></span>
      <span
        class:unknown={window5hLabel.endsWith('--')}
        class="composer-metric"
        title={window5hTitle}
      >
        {window5hLabel}
      </span>
      <span class="composer-dot" aria-hidden="true">·</span>
      <span
        class:unknown={weekLabel.endsWith('--')}
        class="composer-metric"
        title={weekTitle}
      >
        {weekLabel}
      </span>
      {#if settingsError}
        <small class="composer-settings-error">{settingsError}</small>
      {/if}
    </div>
  </form>
  {#if sendError || decisionError}
    <p class="error" role="alert">{sendError || decisionError}</p>
  {/if}
</section>
