<script lang="ts">
  import { createEventDispatcher, onDestroy, onMount, tick } from 'svelte';
  import AgenterIcon from '../components/AgenterIcon.svelte';
  import InlineEventRow from '../components/InlineEventRow.svelte';
  import MarkdownBlock from '../components/MarkdownBlock.svelte';
  import PlanCard from '../components/PlanCard.svelte';
  import RawPayloadDetails from '../components/RawPayloadDetails.svelte';
  import SubagentEventRow from '../components/SubagentEventRow.svelte';
  import { ApiError } from '../api/http';
  import { disableApprovalRule, listApprovalRules } from '../api/approvalRules';
  import { connectSessionEvents, type BrowserEventSocket } from '../api/events';
  import {
    answerQuestion,
    createSession,
    decideApproval,
    getSessionAgentOptions,
    getSessionSettings,
    getSession,
    listSlashCommands,
    markPlanHandoff,
    renameSession,
    interruptSessionTurn,
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
    ApprovalDecisionName,
    ApprovalDecision,
    ApprovalPolicyRule,
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
    approvalUiButtonLabel,
    approvalUiChoices,
    commandApprovalPresentation,
    createChatState,
    fileChangeApprovalFiles,
    type ChatItem,
    type ChatState
  } from '../lib/chatEvents';
  import {
    buildChatDebugExport,
    summarizeChatDebugState,
    type ChatDebugWsEntry
  } from '../lib/chatDebugExport';
  import {
    applyUniversalClientMessage,
    createUniversalClientState,
    hasCapabilitySignal,
    type UniversalClientState
  } from '../lib/sessionSnapshot';
  import {
    filterSlashCommands,
    isSlashDraft,
    needsSlashConfirmation,
    parseSlashCommand,
    slashRequest,
    slashResultMessage
  } from '../lib/slashCommands';

  export let sessionId: string;
  type SessionMetaEvent = {
    sessionId: string;
    title: string;
    status?: string;
    workspaceId?: string;
    providerId?: string;
  };
  const dispatch = createEventDispatcher<{ sessionMeta: SessionMetaEvent }>();

  let socket: BrowserEventSocket | undefined;
  let reconnectTimer: ReturnType<typeof setTimeout> | undefined;
  let closedByRoute = false;
  let suppressNextReconnect = false;
  let mounted = false;
  let activeSessionId = '';
  let connectionGeneration = 0;
  let chatState: ChatState = createChatState();
  let universalState: UniversalClientState = createUniversalClientState();
  let session: SessionInfo | undefined;
  let connectionState = 'Connecting';
  let draft = '';
  let sendError = '';
  let decisionError = '';
  let settingsError = '';
  let approvalRulesError = '';
  let submitting = false;
  let stoppingTurn = false;
  let editingTitle = false;
  let titleDraft = '';
  let renaming = false;
  let messageTextarea: HTMLTextAreaElement | undefined;
  let agentOptions: AgentOptions = { models: [], collaboration_modes: [] };
  let turnSettings: AgentTurnSettings = {};
  let questionDrafts: Record<string, Record<string, string[]>> = {};
  let customQuestionSelections: Record<string, boolean> = {};
  let slashCommands: SlashCommandDefinition[] = [];
  let slashSelectionIndex = 0;
  let pendingDangerCommand:
    | { command: SlashCommandDefinition; request: SlashCommandRequest }
    | undefined;
  let openComposerMenu: 'mode' | 'model' | 'reasoning' | 'verbosity' | null = null;
  type VerbosityMode = 'compact' | 'normal' | 'detailed' | 'debug';
  const VERBOSITY_OPTIONS: VerbosityMode[] = ['compact', 'normal', 'detailed', 'debug'];
  const DEFAULT_VERBOSITY: VerbosityMode = 'debug';
  const VERBOSITY_STORAGE_KEY = 'agenter.chat.verbosity.v1';
  const CHAT_DEBUG_WS_LIMIT = 500;
  let verbosity: VerbosityMode = DEFAULT_VERBOSITY;
  let dismissedPlanIds: Set<string> = new Set();
  let eventStream: HTMLDivElement | undefined;
  const EVENT_STREAM_BOTTOM_EPSILON_PX = 8;
  let sessionMetaSignature = '';
  let approvalRules: ApprovalPolicyRule[] = [];
  let loadingApprovalRules = false;
  let debugWsMessages: ChatDebugWsEntry[] = [];

  function emitSessionMeta() {
    if (!session) {
      return;
    }
    const title = session.title?.trim() || 'New session';
    const signature = `${session.session_id}|${title}|${session.status}|${session.workspace_id}|${session.provider_id}`;
    if (signature === sessionMetaSignature) {
      return;
    }
    sessionMetaSignature = signature;
    dispatch('sessionMeta', {
      sessionId: session.session_id,
      title,
      status: session.status,
      workspaceId: session.workspace_id,
      providerId: session.provider_id
    });
  }

  function exitSession() {
    window.location.hash = '#/';
  }

  const PLAN_IMPLEMENTATION_CLEAR_CONTEXT_PREFIX =
    "A previous agent produced the plan below to accomplish the user's task. " +
    'Implement the plan in a fresh context. Treat the plan as the source of ' +
    'user intent, re-read files as needed, and carry the work through ' +
    'implementation and verification.';
  const PLAN_IMPLEMENTATION_CODING_MESSAGE = 'Implement the plan.';

  $: items = chatState.items;
  $: turnActivity = chatState.activity;
  $: turnActive = turnActivity?.active ?? false;
  $: visibleItems = items.filter((item) => shouldShowItem(item, verbosity));
  $: shouldExpandThinkingByDefault = verbosity === 'detailed' || verbosity === 'debug';
  $: sendBusy = submitting || stoppingTurn || turnActive;
  $: sendButtonLabel = turnActive ? 'Stop turn' : 'Send';
  $: slashSuggestions = filterSlashCommands(draft, slashCommands);
  $: parsedSlash = parseSlashCommand(draft, slashCommands);
  $: slashMenuOpen = isSlashDraft(draft) && slashSuggestions.length > 0;
  $: usage = universalState.latestUsage ?? universalState.snapshot?.info?.usage ?? session?.usage ?? null;
  $: defaultModeId = resolveDefaultModeId(agentOptions.collaboration_modes);
  $: defaultModeAvailable = defaultModeId !== null;
  $: universalCapabilities = universalState.snapshot?.capabilities;
  $: capabilitySignal = hasCapabilitySignal(universalCapabilities);
  $: modeControlEnabled =
    !capabilitySignal || universalCapabilities?.modes.collaboration_modes === true;
  $: modelControlEnabled =
    !capabilitySignal || universalCapabilities?.modes.model_selection === true;
  $: reasoningControlEnabled =
    !capabilitySignal || universalCapabilities?.modes.reasoning_effort === true;
  $: approvalControlEnabled =
    !capabilitySignal || universalCapabilities?.approvals.enabled === true;
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
  $: sessionStatusTone = session ? statusTone(session.status) : 'idle';
  $: sessionStatusLabel = session ? sessionStatusLabelFor(session.status) : connectionState;
  $: if (slashSelectionIndex >= slashSuggestions.length) {
    slashSelectionIndex = 0;
  }
  $: emitSessionMeta();
  $: if (mounted && sessionId !== activeSessionId) {
    universalState = createUniversalClientState();
    activeSessionId = sessionId;
    void reloadAndConnect();
  }

  onMount(() => {
    mounted = true;
    activeSessionId = sessionId;
    verbosity = readVerbosityFromStorage();
    window.addEventListener('pointerdown', closeComposerMenuOnOutsideClick);
    window.addEventListener('keydown', closeComposerMenuOnEscape);
    window.__agenterExportChatDebug = exportChatDebugState;
    void reloadAndConnect();
  });

  onDestroy(() => {
    closedByRoute = true;
    window.removeEventListener('pointerdown', closeComposerMenuOnOutsideClick);
    window.removeEventListener('keydown', closeComposerMenuOnEscape);
    if (window.__agenterExportChatDebug === exportChatDebugState) {
      delete window.__agenterExportChatDebug;
    }
    if (reconnectTimer) {
      clearTimeout(reconnectTimer);
    }
    socket?.close();
  });

  async function reloadAndConnect() {
    const generation = ++connectionGeneration;
    const replayCursor = universalState.latestSeq;
    openComposerMenu = null;
    socket?.close();
    if (reconnectTimer) {
      clearTimeout(reconnectTimer);
      reconnectTimer = undefined;
    }
    chatState = createChatState();
    universalState = {
      ...createUniversalClientState(),
      latestSeq: replayCursor
    };
    dismissedPlanIds = new Set();
    session = undefined;
    sendError = '';
    decisionError = '';
    settingsError = '';
    approvalRulesError = '';
    approvalRules = [];
    debugWsMessages = [];
    editingTitle = false;
    stoppingTurn = false;
    connectionState = 'Loading history';
    try {
      session = await getSession(sessionId);
      if (generation !== connectionGeneration) {
        return;
      }
      titleDraft = session.title ?? '';
      void loadAgentSettings(generation);
      void loadSlashCommands(generation);
      void loadApprovalRules(generation);
      universalState = { ...createUniversalClientState(), latestSeq: replayCursor };
      await tick();
      scrollEventStreamToBottom();
    } catch {
      connectionState = 'History unavailable';
      pushToast({ severity: 'error', message: 'Could not load session history.' });
    }

    if (closedByRoute) {
      return;
    }

    connectionState = 'Connecting';
    socket = connectSessionEvents(sessionId, {
      afterSeq: universalState.latestSeq,
      includeSnapshot: true
    }, {
      onOpen: () => {
        if (generation !== connectionGeneration) {
          return;
        }
        connectionState = 'Subscribed';
      },
      onMessage: (message: BrowserServerMessage) => {
        const shouldScrollToBottom = isEventStreamAtBottom();
        if (generation !== connectionGeneration) {
          return;
        }
        if (message.type === 'session_snapshot' || message.type === 'universal_event') {
          try {
            universalState = applyUniversalClientMessage(universalState, message);
            chatState = universalState.chat;
            if (message.type === 'session_snapshot' && !message.replay_complete) {
              connectionState = 'Loaded snapshot';
            }
            if (universalState.snapshot?.info) {
              session = universalState.snapshot.info;
              titleDraft = session.title ?? '';
              void loadApprovalRules(generation);
            }
            void tick().then(() => {
              if (shouldScrollToBottom) {
                scrollEventStreamToBottom();
              }
            });
          } catch {
            pushToast({ severity: 'warning', message: 'Skipped a malformed live event.' });
          }
        }
        if (message.type === 'error') {
          connectionState = message.message;
          if (message.code === 'snapshot_replay_incomplete') {
            suppressNextReconnect = true;
            socket?.close();
          }
          pushToast({ severity: 'error', message: message.message });
        }
        recordDebugWsMessage(message);
      },
      onClose: () => {
        if (suppressNextReconnect) {
          suppressNextReconnect = false;
          return;
        }
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

  async function loadApprovalRules(generation = connectionGeneration) {
    if (!session?.workspace_id || !session.provider_id) {
      approvalRules = [];
      return;
    }
    loadingApprovalRules = true;
    approvalRulesError = '';
    try {
      const rules = await listApprovalRules(session.workspace_id, session.provider_id);
      if (generation === connectionGeneration) {
        approvalRules = rules;
      }
    } catch {
      if (generation === connectionGeneration) {
        approvalRulesError = 'Could not load approval rules.';
      }
    } finally {
      if (generation === connectionGeneration) {
        loadingApprovalRules = false;
      }
    }
  }

  async function revokeApprovalRule(ruleId: string) {
    try {
      await disableApprovalRule(ruleId);
      approvalRules = approvalRules.filter((rule) => rule.rule_id !== ruleId);
    } catch {
      approvalRulesError = 'Could not revoke approval rule.';
    }
  }

  async function submit() {
    if (stoppingTurn) {
      return;
    }

    if (turnActive) {
      await stopActiveTurn();
      return;
    }

    if (submitting) {
      return;
    }

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

  async function stopActiveTurn() {
    if (!turnActive || stoppingTurn) {
      return;
    }

    sendError = '';
    stoppingTurn = true;
    try {
      const result = await interruptSessionTurn(sessionId);
      if (!result.accepted) {
        sendError = result.message || 'Could not stop the current turn.';
        pushToast({ severity: 'warning', message: sendError });
      }
    } catch (error) {
      sendError = apiErrorMessage(error, 'Could not stop the current turn.');
      pushToast({ severity: 'error', message: sendError });
    } finally {
      stoppingTurn = false;
    }
  }

  function isEventStreamAtBottom(): boolean {
    if (!eventStream) {
      return true;
    }
    return (
      eventStream.scrollTop + eventStream.clientHeight >=
      eventStream.scrollHeight - EVENT_STREAM_BOTTOM_EPSILON_PX
    );
  }

  function scrollEventStreamToBottom() {
    if (!eventStream) {
      return;
    }
    eventStream.scrollTop = eventStream.scrollHeight;
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

  function closeComposerMenu() {
    openComposerMenu = null;
  }

  function closeComposerMenuOnOutsideClick(event: PointerEvent) {
    if (openComposerMenu === null) {
      return;
    }
    const target = event.target as Element | null;
    if (!target || !target.closest('.composer-chip') && !target.closest('.composer-chip-menu')) {
      closeComposerMenu();
    }
  }

  function closeComposerMenuOnEscape(event: KeyboardEvent) {
    if (event.key === 'Escape' && openComposerMenu !== null) {
      event.preventDefault();
      closeComposerMenu();
    }
  }

  function readVerbosityFromStorage(): VerbosityMode {
    if (typeof window === 'undefined' || !window.localStorage) {
      return DEFAULT_VERBOSITY;
    }
    const raw = localStorage.getItem(VERBOSITY_STORAGE_KEY);
    return raw === 'compact' || raw === 'normal' || raw === 'detailed' || raw === 'debug'
      ? raw
      : DEFAULT_VERBOSITY;
  }

  function writeVerbosityToStorage(next: VerbosityMode) {
    if (typeof window === 'undefined' || !window.localStorage) {
      return;
    }
    localStorage.setItem(VERBOSITY_STORAGE_KEY, next);
  }

  function isThinkingItem(item: ChatItem): boolean {
    return item.kind === 'inlineEvent' && item.displayLevel === 'thinking';
  }

  function shouldShowItem(item: ChatItem, level: VerbosityMode): boolean {
    if (item.kind !== 'inlineEvent') {
      return true;
    }
    if (item.displayLevel === 'raw' && level !== 'debug') {
      return false;
    }
    return true;
  }

  function shouldExpandInlineEvent(item: ChatItem): boolean {
    return shouldShowItem(item, verbosity) && isThinkingItem(item) && shouldExpandThinkingByDefault;
  }

  function setVerbosity(level: VerbosityMode) {
    openComposerMenu = null;
    verbosity = level;
    writeVerbosityToStorage(level);
  }

  function recordDebugWsMessage(message: BrowserServerMessage) {
    const entry: ChatDebugWsEntry = {
      receivedAt: new Date().toISOString(),
      message,
      stateAfter: summarizeChatDebugState({
        chatState,
        visibleItems,
        universalState
      })
    };
    debugWsMessages = [...debugWsMessages, entry].slice(-CHAT_DEBUG_WS_LIMIT);
  }

  function buildCurrentChatDebugExport() {
    return buildChatDebugExport({
      activeSessionId,
      connectionState,
      session,
      verbosity,
      chatState,
      visibleItems,
      universalState,
      wsMessages: debugWsMessages
    });
  }

  function exportChatDebugState() {
    const exported = buildCurrentChatDebugExport();
    const text = JSON.stringify(exported, null, 2);
    void navigator.clipboard?.writeText(text).then(
      () => pushToast({ severity: 'info', message: 'Chat debug export copied to clipboard.' }),
      () => pushToast({ severity: 'warning', message: 'Chat debug export returned in console helper; clipboard unavailable.' })
    );
    return exported;
  }

  function handleExportChatDebug() {
    const exported = exportChatDebugState();
    // Keep a direct console handle for large payloads when clipboard access is blocked.
    console.debug('[agenter] chat-debug-export', exported);
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

  async function resolveApproval(item: ChatItem, decision: ApprovalDecisionName, optionId?: string) {
    if (item.kind !== 'approval' || item.resolvedDecision) {
      return;
    }

    decisionError = '';
    try {
      const request: ApprovalDecision = {
        decision,
        ...(optionId ? { option_id: optionId } : {})
      };
      await decideApproval(item.approvalId, request);
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

  function toggleComposerMenu(menu: 'mode' | 'model' | 'reasoning' | 'verbosity') {
    openComposerMenu = openComposerMenu === menu ? null : menu;
  }

  function modeLabel(modeId: string) {
    return (agentOptions.collaboration_modes.find((mode) => mode.id === modeId)?.label ?? modeId) || 'default';
  }

  function modelLabel(modelId: string) {
    return (agentOptions.models.find((model) => model.id === modelId)?.display_name ?? modelId) || 'model';
  }

  function reasoningLabel(reasoning: string) {
    return reasoning || 'thinking';
  }

  function chooseMode(value: string) {
    openComposerMenu = null;
    setCollaborationMode(value);
    void tick().then(() => messageTextarea?.focus());
  }

  function chooseModel(value: string) {
    openComposerMenu = null;
    setModel(value);
    void tick().then(() => messageTextarea?.focus());
  }

  function chooseReasoning(value: string) {
    openComposerMenu = null;
    setReasoningEffort(value);
    void tick().then(() => messageTextarea?.focus());
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

  async function handleImplementPlan(planId: string) {
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
        settings_override: override,
        plan_handoff: {
          plan_id: planId.replace(/^plan:/, ''),
          action: 'same_thread'
        }
      });
      dismissedPlanIds = new Set([...dismissedPlanIds, planId]);
    } catch (error) {
      sendError = apiErrorMessage(error, 'Could not implement plan.');
      pushToast({ severity: 'error', message: sendError });
    } finally {
      submitting = false;
    }
  }

  async function handleClearContextImplement(planId: string, planContent: string) {
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
        settings_override: { collaboration_mode: defaultModeId },
        source_plan_handoff: {
          session_id: session.session_id,
          plan_id: planId.replace(/^plan:/, ''),
          action: 'fresh_thread'
        }
      });
      dismissedPlanIds = new Set([...dismissedPlanIds, planId]);
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

  async function handleStayInPlan(planId: string) {
    dismissedPlanIds = new Set([...dismissedPlanIds, planId]);
    try {
      await markPlanHandoff(sessionId, {
        plan_id: planId.replace(/^plan:/, ''),
        action: 'stay_in_plan'
      });
    } catch (error) {
      sendError = apiErrorMessage(error, 'Could not persist plan handoff choice.');
      pushToast({ severity: 'error', message: sendError });
    }
  }

  function planHandoffAvailable(item: ChatItem) {
    return item.kind === 'plan' && (!item.handoff || item.handoff.state === 'available');
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

  function statusTone(status: string | undefined): string {
    if (status === 'running' || status === 'starting' || status === 'interrupting') {
      return 'running';
    }
    if (status === 'waiting_for_approval' || status === 'waiting_for_input') {
      return 'waiting';
    }
    if (status === 'failed' || status === 'degraded' || status === 'interrupted') {
      return 'error';
    }
    if (status === 'completed') {
      return 'done';
    }
    if (status === 'idle' || status === 'stopped') {
      return 'idle';
    }
    return 'idle';
  }

  function sessionStatusLabelFor(status: string | undefined): string {
    switch (status) {
      case 'running':
        return 'running';
      case 'waiting_for_approval':
        return 'needs approval';
      case 'waiting_for_input':
        return 'waiting';
      case 'interrupting':
        return 'stopping';
      case 'failed':
      case 'degraded':
        return 'error';
      case 'completed':
        return 'done';
      case 'idle':
        return 'idle';
      case 'stopped':
        return 'stopped';
      case 'starting':
        return 'starting';
      default:
        return status ? status.replaceAll('_', ' ') : 'idle';
    }
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

  function presentationString(
    presentation: Record<string, unknown> | undefined,
    key: string
  ): string | undefined {
    const value = presentation?.[key];
    return typeof value === 'string' && value ? value : undefined;
  }

  function resolvedApprovalLabel(decision: string) {
    return decision.replaceAll('_', ' ');
  }

  function approvalKindLabel(kind: string | undefined) {
    return (kind ?? 'approval').replaceAll('_', ' ');
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

  function questionFieldKey(questionId: string, fieldId: string) {
    return `${questionId}:${fieldId}`;
  }

  function setCustomQuestionSelected(questionId: string, fieldId: string, selected: boolean) {
    customQuestionSelections = {
      ...customQuestionSelections,
      [questionFieldKey(questionId, fieldId)]: selected
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

  function isSingleChoiceField(field: AgentQuestionField) {
    return (field.kind === 'single_select' || field.kind === 'choice') && field.choices.length > 0;
  }

  function isCustomQuestionAnswer(item: ChatItem, field: AgentQuestionField) {
    if (item.kind !== 'question') {
      return false;
    }
    const answer = questionAnswers(item, field)[0];
    return (
      customQuestionSelections[questionFieldKey(item.questionId, field.id)] === true ||
      (typeof answer === 'string' && answer.length > 0 && !field.choices.some((choice) => choice.value === answer))
    );
  }

  function customQuestionAnswer(item: ChatItem, field: AgentQuestionField) {
    if (item.kind !== 'question') {
      return '';
    }
    return isCustomQuestionAnswer(item, field) ? questionAnswers(item, field)[0] : '';
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
      await answerQuestion(item.questionId, answers);
    } catch {
      decisionError = 'Could not answer question.';
      pushToast({ severity: 'error', message: decisionError });
    }
  }
</script>

<section class="chat-layout">
  <header class="chat-header">
    <div class="chat-heading">
      <button class="chat-back-button" type="button" on:click={exitSession} aria-label="Back to sessions list">
        <AgenterIcon name="chevron" size={12} />
        <span>Sessions</span>
      </button>
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
      <div class="chat-subtitle">
        <span>{session?.workspace_id ?? 'workspace'}</span>
        <span>·</span>
        <span>{session?.provider_id ?? 'provider'}</span>
        <span>·</span>
        <span>{connectionState}</span>
      </div>
      {#if session}
        <details class="approval-rules-panel">
          <summary>
            Approval rules
            {#if loadingApprovalRules}
              <span class="muted">loading</span>
            {:else}
              <span class="muted">{approvalRules.length}</span>
            {/if}
          </summary>
          {#if approvalRulesError}
            <p class="error-text">{approvalRulesError}</p>
          {:else if approvalRules.length === 0}
            <p class="muted">No persistent approval rules.</p>
          {:else}
            <div class="approval-rule-list">
              {#each approvalRules as rule}
                <div class="approval-rule-row">
                  <span>{rule.label}</span>
                  <button class="secondary compact" type="button" on:click={() => revokeApprovalRule(rule.rule_id)}>
                    Revoke
                  </button>
                </div>
              {/each}
            </div>
          {/if}
        </details>
      {/if}
    </div>
    <div class="chat-header-actions">
      {#if verbosity === 'debug'}
        <button class="secondary compact chat-debug-export" type="button" on:click={handleExportChatDebug}>
          Export debug
        </button>
      {/if}
      <span class:done={sessionStatusTone === 'done'} class:error={sessionStatusTone === 'error'} class:idle={sessionStatusTone === 'idle'} class:running={sessionStatusTone === 'running'} class:waiting={sessionStatusTone === 'waiting'} class="status-pill">
        <span class="status-dot" aria-hidden="true"></span>
        {sessionStatusLabel}
      </span>
    </div>
  </header>

  <div class="event-stream" bind:this={eventStream}>
    {#if visibleItems.length === 0}
      <div class="empty-state">
        <strong>No events yet</strong>
        <span>Send a message or wait for the connected runner to stream universal events.</span>
      </div>
    {:else}
      {#each visibleItems as item (item.id)}
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
          <InlineEventRow expandedByDefault={shouldExpandInlineEvent(item)} {item} />
        {:else if item.kind === 'subagent'}
          <SubagentEventRow {item} />
        {:else if item.kind === 'plan'}
          <PlanCard
            {item}
            pendingHandoff={item.id === latestPlanId &&
              planTurnComplete &&
              !dismissedPlanIds.has(item.id) &&
              planHandoffAvailable(item)}
            {turnActive}
            {defaultModeAvailable}
            onImplement={() => handleImplementPlan(item.id)}
            onClearContextImplement={() => handleClearContextImplement(item.id, item.content)}
            onStayInPlan={() => handleStayInPlan(item.id)}
          />
        {:else if item.kind === 'approval'}
          {#if item.resolvedDecision}
            <article class="approval-resolved-row">
              <span class="approval-resolved-icon" aria-hidden="true"><AgenterIcon name="checklist" size={12} /></span>
              <span>Approval answered</span>
              <code>{resolvedApprovalLabel(item.resolvedDecision)}</code>
              <RawPayloadDetails payload={item.rawPayload} />
            </article>
          {:else}
            <article class="event-card approval-card log-card">
              <div class="card-heading log-card-heading">
                <span class="log-eyebrow">! {approvalKindLabel(item.approvalKind)} approval</span>
              </div>
              <strong>{item.title}</strong>
              {#if item.approvalKind || item.nativeRequestId || item.nativeBlocking}
                <div class="approval-meta-grid">
                  {#if item.approvalKind}
                    <span>kind</span>
                    <strong>{approvalKindLabel(item.approvalKind)}</strong>
                  {/if}
                  {#if item.nativeRequestId}
                    <span>request</span>
                    <strong>{item.nativeRequestId}</strong>
                  {/if}
                  {#if item.nativeBlocking}
                    <span>blocking</span>
                    <strong>yes</strong>
                  {/if}
                </div>
              {/if}
              {#if commandApprovalPresentation(item.presentation)}
                <div class="approval-meta-grid">
                  <span>command</span>
                  <strong>{commandApprovalPresentation(item.presentation)}</strong>
                  {#if presentationString(item.presentation, 'cwd')}
                    <span>cwd</span>
                    <strong>{presentationString(item.presentation, 'cwd')}</strong>
                  {/if}
                </div>
                <pre class="approval-command">{commandApprovalPresentation(item.presentation)}</pre>
              {/if}
              {#each fileChangeApprovalFiles(item.presentation) as file}
                <details class="approval-file-diff">
                  <summary>
                    <AgenterIcon name="file" size={12} />
                    <span>{file.path}</span>
                    {#if file.changeKind}<span class="muted"> ({file.changeKind})</span>{/if}
                  </summary>
                  {#if file.diff}<pre>{file.diff}</pre>{:else}<span class="muted">No diff text</span>{/if}
                </details>
              {/each}
              {#if item.detail}
                {#if fileChangeApprovalFiles(item.presentation).length === 0}
                  <pre class="approval-detail-plain">{item.detail}</pre>
                {:else}
                  <details class="approval-extra-detail">
                    <summary>Extra context</summary>
                    <pre>{item.detail}</pre>
                  </details>
                {/if}
              {/if}
              {#if item.resolutionState === 'resolving'}
                <p class="muted approval-pending-copy">
                  Resolving {item.resolvingDecision ? resolvedApprovalLabel(item.resolvingDecision) : 'decision'}...
                </p>
              {/if}
              <div class="inline-actions approval-actions">
                {#each approvalUiChoices(item) as choice}
                  <button
                    class:secondary={choice.decision === 'decline' || choice.decision === 'cancel'}
                    class="compact approval-decision"
                    type="button"
                    title={choice.description}
                    disabled={!approvalControlEnabled || item.resolutionState === 'resolving'}
                    on:click={() => resolveApproval(item, choice.decision, choice.optionId)}
                  >
                    {choice.label || approvalUiButtonLabel(choice.decision)}
                  </button>
                {/each}
              </div>
              <RawPayloadDetails payload={item.rawPayload} />
            </article>
          {/if}
        {:else if item.kind === 'question'}
          <article class="event-card question-card log-card">
            <div class="card-heading log-card-heading">
              <span class="log-eyebrow">? question</span>
              {#if item.answered}
                <code>answered</code>
              {/if}
            </div>
            <strong>{item.title}</strong>
            {#if item.description}
              <p>{item.description}</p>
            {/if}
            {#if item.nativeRequestId || item.nativeBlocking}
              <div class="approval-meta-grid">
                {#if item.nativeRequestId}
                  <span>request</span>
                  <strong>{item.nativeRequestId}</strong>
                {/if}
                {#if item.nativeBlocking}
                  <span>blocking</span>
                  <strong>yes</strong>
                {/if}
              </div>
            {/if}
            {#each item.fields as field}
              <div class="question-field">
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
                        <span class="choice-text">
                          <strong>{choice.label}</strong>
                          {#if choice.description}
                            <small>{choice.description}</small>
                          {/if}
                        </span>
                      </label>
                    {/each}
                  </div>
                {:else if isSingleChoiceField(field)}
                  <div class="choice-list single-choice-list">
                    {#each field.choices as choice}
                      <label class:active={questionAnswers(item, field)[0] === choice.value}>
                        <input
                          type="radio"
                          name={`${item.questionId}-${field.id}`}
                          disabled={item.answered}
                          checked={questionAnswers(item, field)[0] === choice.value}
                          on:change={() => {
                            setCustomQuestionSelected(item.questionId, field.id, false);
                            setQuestionAnswers(item.questionId, field.id, [choice.value]);
                          }}
                        />
                        <span class="choice-text">
                          <strong>{choice.label}</strong>
                          {#if choice.description}
                            <small>{choice.description}</small>
                          {/if}
                        </span>
                      </label>
                    {/each}
                    <label class:active={isCustomQuestionAnswer(item, field)}>
                      <input
                        type="radio"
                        name={`${item.questionId}-${field.id}`}
                        disabled={item.answered}
                        checked={isCustomQuestionAnswer(item, field)}
                        on:change={() => {
                          setCustomQuestionSelected(item.questionId, field.id, true);
                          setQuestionAnswers(item.questionId, field.id, [customQuestionAnswer(item, field)]);
                        }}
                      />
                      <span class="choice-text">
                        <strong>Custom response</strong>
                        <small>Send plain text instead of one of these options.</small>
                      </span>
                    </label>
                    <input
                      class="question-custom-input"
                      type={field.secret ? 'password' : 'text'}
                      disabled={item.answered}
                      placeholder="Type custom response..."
                      value={customQuestionAnswer(item, field)}
                      on:focus={() => {
                        if (!isCustomQuestionAnswer(item, field)) {
                          setCustomQuestionSelected(item.questionId, field.id, true);
                          setQuestionAnswers(item.questionId, field.id, ['']);
                        }
                      }}
                      on:input={(event) => setQuestionAnswers(item.questionId, field.id, [event.currentTarget.value])}
                    />
                  </div>
                {:else}
                  <input
                    type={field.secret ? 'password' : field.kind === 'number' ? 'number' : 'text'}
                    disabled={item.answered}
                    value={questionAnswers(item, field)[0] ?? ''}
                    on:input={(event) => setQuestionAnswers(item.questionId, field.id, [event.currentTarget.value])}
                  />
                {/if}
                <RawPayloadDetails payload={field.schema} summary="Field schema" />
              </div>
            {/each}
            {#if !item.answered}
              <div class="inline-actions">
                <button type="button" on:click={() => submitQuestion(item)}>Answer</button>
              </div>
            {/if}
            <RawPayloadDetails payload={item.rawPayload} />
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
            <RawPayloadDetails payload={item.rawPayload} />
          </article>
        {/if}
      {/each}
    {/if}
    {#if turnActive}
      <div class="working-row" aria-live="polite">
        <!-- <span class="working-dot"></span> -->
        <span class="working-shimmer-text">{turnActivity?.label ?? 'Working'}</span>
        <!-- <span class="working-shimmer"></span> -->
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
    <button
      class:busy={sendBusy}
      type="submit"
      disabled={stoppingTurn || (submitting && !turnActive)}
      aria-label={sendButtonLabel}
      title={sendButtonLabel}
    >
      {#if sendBusy}
        <span class="button-spinner" aria-hidden="true"></span>
      {:else}
        Send
      {/if}
    </button>
    <div class="composer-bottom-bar">
      <span class="composer-chip-wrap">
        <button
          aria-expanded={openComposerMenu === 'mode'}
          aria-label="Collaboration mode"
          class="composer-chip"
          disabled={!modeControlEnabled || agentOptions.collaboration_modes.length === 0}
          type="button"
          on:click={() => toggleComposerMenu('mode')}
        >
          {modeLabel(modeValue)}
          <AgenterIcon name="chevron" size={14} />
        </button>
        {#if openComposerMenu === 'mode'}
          <span class="composer-chip-menu">
            {#if agentOptions.collaboration_modes.length === 0}
              <button type="button" disabled>default</button>
            {:else}
              {#each agentOptions.collaboration_modes as mode}
                <button
                  class:active={mode.id === modeValue}
                  type="button"
                  on:click={() => chooseMode(mode.id)}
                >
                  {mode.label}
                </button>
              {/each}
            {/if}
          </span>
        {/if}
      </span>
      <span class="composer-dot" aria-hidden="true">·</span>
      <span class="composer-chip-wrap model-chip-wrap">
        <button
          aria-expanded={openComposerMenu === 'model'}
          aria-label="Model"
          class="composer-chip"
          disabled={!modelControlEnabled}
          type="button"
          on:click={() => toggleComposerMenu('model')}
        >
          {modelLabel(modelValue)}
          <AgenterIcon name="chevron" size={14} />
        </button>
        {#if openComposerMenu === 'model'}
          <span class="composer-chip-menu">
            <button class:active={modelValue === ''} type="button" on:click={() => chooseModel('')}>model</button>
            {#each agentOptions.models as model}
              <button
                class:active={model.id === modelValue}
                type="button"
                on:click={() => chooseModel(model.id)}
              >
                {model.display_name}
              </button>
            {/each}
          </span>
        {/if}
      </span>
      <span class="composer-chip-wrap">
        <button
          aria-expanded={openComposerMenu === 'reasoning'}
          aria-label="Thinking level"
          class="composer-chip"
          disabled={!reasoningControlEnabled}
          type="button"
          on:click={() => toggleComposerMenu('reasoning')}
        >
          {reasoningLabel(reasoningValue)}
          <AgenterIcon name="chevron" size={14} />
        </button>
        {#if openComposerMenu === 'reasoning'}
          <span class="composer-chip-menu">
            <button class:active={reasoningValue === ''} type="button" on:click={() => chooseReasoning('')}>thinking</button>
            {#each effortsForSelectedModel() as effort}
              <button
                class:active={effort === reasoningValue}
                type="button"
                on:click={() => chooseReasoning(effort)}
              >
                {effort}
              </button>
            {/each}
          </span>
        {/if}
      </span>
      <span class="composer-dot" aria-hidden="true">·</span>
      <span class="composer-chip-wrap">
        <button
          aria-expanded={openComposerMenu === 'verbosity'}
          aria-label="Transcript verbosity"
          class="composer-chip"
          type="button"
          on:click={() => toggleComposerMenu('verbosity')}
        >
          {verbosity}
          <AgenterIcon name="chevron" size={14} />
        </button>
        {#if openComposerMenu === 'verbosity'}
          <span class="composer-chip-menu">
            {#each VERBOSITY_OPTIONS as option}
              <button class:active={verbosity === option} type="button" on:click={() => setVerbosity(option)}>
                {option}
              </button>
            {/each}
          </span>
        {/if}
      </span>
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
