import type {
  ApprovalRequest,
  BrowserServerMessage,
  CapabilitySet,
  ContentBlock,
  DiffState,
  ItemState,
  PlanState,
  SessionSnapshot,
  UniversalEventEnvelope
} from '../api/types';
import {
  applyChatEnvelope,
  approvalChoiceFromOption,
  createChatState,
  type ChatActivity,
  type ChatItem,
  type ChatState
} from './chatEvents';
import { applyUniversalEvent, cloneSnapshot, compareSeq, universalEventKey } from './universalEvents';

export interface UniversalClientState {
  chat: ChatState;
  legacyOverlay: ChatState;
  snapshot?: SessionSnapshot;
  latestSeq?: string;
  usingUniversal: boolean;
  snapshotIncomplete: boolean;
  seenUniversalEvents: Set<string>;
  rowOrder: Map<string, RowOrder>;
}

interface RowOrder {
  seq: string;
  ts: string;
  index: number;
}

export function createUniversalClientState(): UniversalClientState {
  return {
    chat: createChatState(),
    legacyOverlay: createChatState(),
    latestSeq: undefined,
    snapshot: undefined,
    usingUniversal: false,
    snapshotIncomplete: false,
    seenUniversalEvents: new Set(),
    rowOrder: new Map()
  };
}

export function applyUniversalClientMessage(
  state: UniversalClientState,
  message: BrowserServerMessage
): UniversalClientState {
  if (message.type === 'session_snapshot') {
    return applySessionSnapshotMessage(state, message);
  }
  if (message.type === 'universal_event') {
    return applyLiveUniversalEvent(state, message);
  }
  if (message.type === 'app_event') {
    const legacyOverlay = applyChatEnvelope(state.legacyOverlay, message);
    return {
      ...state,
      legacyOverlay,
      chat: mergeChatStates(state.chat, legacyOverlay)
    };
  }
  return state;
}

export function materializeSnapshotChatState(
  snapshot: SessionSnapshot,
  rowOrder: Map<string, RowOrder> = new Map()
): ChatState {
  const items: ChatItem[] = [];

  for (const item of Object.values(snapshot.items)) {
    const chatItem = materializeItem(item);
    if (chatItem) {
      items.push(chatItem);
    }
  }
  for (const plan of Object.values(snapshot.plans)) {
    const chatItem = materializePlan(plan);
    if (chatItem) {
      items.push(chatItem);
    }
  }
  for (const approval of Object.values(snapshot.approvals)) {
    items.push(materializeApproval(approval));
  }
  for (const diff of Object.values(snapshot.diffs)) {
    items.push(materializeDiff(diff));
  }
  for (const artifact of Object.values(snapshot.artifacts)) {
    items.push(materializeArtifact(artifact));
  }

  const sortedItems = sortByUniversalOrder(items, rowOrder);
  const latestPlan = [...sortedItems].reverse().find((item) => item.kind === 'plan');
  return {
    seenEventIds: new Set(),
    items: sortedItems,
    activity: snapshotActivity(snapshot),
    latestPlanId: latestPlan?.id,
    planTurnComplete: !snapshot.active_turns.some((turnId) => {
      const status = snapshot.turns[turnId]?.status;
      return status === 'running' || status === 'waiting_for_approval' || status === 'waiting_for_input';
    })
  };
}

export function hasCapabilitySignal(capabilities: CapabilitySet | undefined): boolean {
  if (!capabilities) {
    return false;
  }
  return Object.values(capabilities).some((group) =>
    Object.values(group as Record<string, unknown>).some((value) => value === true)
  );
}

function applySessionSnapshotMessage(
  state: UniversalClientState,
  message: Extract<BrowserServerMessage, { type: 'session_snapshot' }>
): UniversalClientState {
  if (message.has_more) {
    return {
      ...state,
      snapshotIncomplete: true
    };
  }

  let snapshot = cloneSnapshot(message.snapshot);
  let latestSeq = snapshot.latest_seq ?? undefined;
  const seenUniversalEvents = new Set(state.seenUniversalEvents);
  const rowOrder = new Map(state.rowOrder);

  for (const event of message.events) {
    seenUniversalEvents.add(universalEventKey(event));
    recordRowOrder(rowOrder, event);
    if (compareSeq(event.seq, latestSeq) <= 0) {
      continue;
    }
    snapshot = applyUniversalEvent(snapshot, event);
    latestSeq = event.seq;
  }

  if (message.latest_seq && compareSeq(message.latest_seq, latestSeq) > 0) {
    latestSeq = message.latest_seq;
    snapshot.latest_seq = message.latest_seq;
  }

  return {
    chat: mergeChatStates(materializeSnapshotChatState(snapshot, rowOrder), state.legacyOverlay),
    legacyOverlay: state.legacyOverlay,
    snapshot,
    latestSeq,
    usingUniversal: true,
    snapshotIncomplete: false,
    seenUniversalEvents,
    rowOrder
  };
}

function applyLiveUniversalEvent(
  state: UniversalClientState,
  event: UniversalEventEnvelope
): UniversalClientState {
  const key = universalEventKey(event);
  if (state.seenUniversalEvents.has(key)) {
    return state;
  }
  if (compareSeq(event.seq, state.latestSeq) <= 0) {
    return state;
  }
  const rowOrder = new Map(state.rowOrder);
  recordRowOrder(rowOrder, event);
  const snapshot = applyUniversalEvent(
    state.snapshot ?? emptySnapshot(event.session_id, state.latestSeq),
    event
  );
  const seenUniversalEvents = new Set(state.seenUniversalEvents);
  seenUniversalEvents.add(key);
  return {
    chat: mergeChatStates(materializeSnapshotChatState(snapshot, rowOrder), state.legacyOverlay),
    legacyOverlay: state.legacyOverlay,
    snapshot,
    latestSeq: compareSeq(event.seq, state.latestSeq) > 0 ? event.seq : state.latestSeq,
    usingUniversal: true,
    snapshotIncomplete: false,
    seenUniversalEvents,
    rowOrder
  };
}

function mergeChatStates(base: ChatState, overlay: ChatState): ChatState {
  const overlayById = new Map(overlay.items.map((item) => [item.id, item]));
  const baseIds = new Set(base.items.map((item) => item.id));
  const items = base.items.map((item) => mergeChatItem(item, overlayById.get(item.id)));
  const overlayItems = overlay.items.filter((item) => !baseIds.has(item.id));
  return {
    ...base,
    items: [...items, ...overlayItems],
    seenEventIds: new Set([...base.seenEventIds, ...overlay.seenEventIds]),
    activity: base.activity ?? overlay.activity,
    latestPlanId: base.latestPlanId ?? overlay.latestPlanId,
    planTurnComplete: base.latestPlanId ? base.planTurnComplete : overlay.planTurnComplete
  };
}

function mergeChatItem(base: ChatItem, overlay: ChatItem | undefined): ChatItem {
  if (!overlay) {
    return base;
  }
  if (base.kind === 'approval' && overlay.kind === 'approval') {
    return {
      ...base,
      ...overlay,
      options: overlay.resolvedDecision ? [] : (overlay.options ?? base.options),
      resolutionState: overlay.resolvedDecision ? undefined : overlay.resolutionState ?? base.resolutionState,
      resolvingDecision: overlay.resolvedDecision ? undefined : overlay.resolvingDecision ?? base.resolvingDecision
    };
  }
  if (base.kind === 'question' && overlay.kind === 'question') {
    return {
      ...base,
      ...overlay,
      fields: overlay.fields.length > 0 ? overlay.fields : base.fields
    };
  }
  return overlay;
}

function emptySnapshot(sessionId: string, latestSeq?: string): SessionSnapshot {
  return {
    session_id: sessionId,
    latest_seq: latestSeq,
    turns: {},
    items: {},
    approvals: {},
    plans: {},
    diffs: {},
    artifacts: {},
    active_turns: []
  };
}

function materializeItem(item: ItemState): ChatItem | undefined {
  const content = itemText(item.content);
  if (item.role === 'user') {
    return {
      id: `user:${item.item_id}`,
      kind: 'user',
      messageId: item.item_id,
      content,
      markdown: true
    };
  }
  if (item.role === 'assistant') {
    return {
      id: `agent:${item.item_id}`,
      kind: 'assistant',
      messageId: item.item_id,
      content,
      markdown: true,
      completed: item.status === 'completed'
    };
  }
  if (item.tool) {
    return materializeToolItem(item, content);
  }
  const first = item.content[0];
  if (first?.kind === 'command_output') {
    return {
      id: `event:item:${item.item_id}`,
      kind: 'inlineEvent',
      eventKind: 'command',
      title: item.native?.summary ?? item.native?.method ?? 'Command output',
      detail: undefined,
      output: content,
      status: item.status,
      source: item.native?.protocol ?? undefined
    };
  }
  return {
    id: `event:item:${item.item_id}`,
    kind: 'inlineEvent',
    eventKind: first?.kind === 'file_diff' ? 'file' : 'tool',
    title: item.native?.summary ?? item.native?.method ?? first?.kind ?? 'Tool activity',
    detail: content || undefined,
    status: item.status,
    source: item.native?.protocol ?? undefined
  };
}

function materializeToolItem(item: ItemState, content: string): ChatItem {
  const tool = item.tool!;
  if (tool.kind === 'subagent' && tool.subagent) {
    return {
      id: `subagent:${item.item_id}`,
      kind: 'subagent',
      operation:
        tool.subagent.operation === 'wait'
          ? 'wait'
          : tool.subagent.operation === 'close'
            ? 'close'
            : 'spawn',
      title: tool.title,
      status: tool.status ?? item.status,
      agentIds: tool.subagent.agent_ids ?? [],
      model: tool.subagent.model ?? undefined,
      reasoningEffort: tool.subagent.reasoning_effort ?? undefined,
      prompt: tool.subagent.prompt ?? undefined,
      states: (tool.subagent.states ?? []).map((state) => ({
        agentId: state.agent_id,
        status: state.status,
        message: state.message ?? undefined
      }))
    };
  }

  if (tool.kind === 'command' && tool.command) {
    return {
      id: `event:item:${item.item_id}`,
      kind: 'inlineEvent',
      eventKind: 'command',
      title: tool.command.command || tool.title,
      detail: commandProjectionDetail(tool.command),
      output: commandOutputText(item.content),
      status: tool.status ?? item.status,
      success: tool.command.success ?? undefined,
      exitCode: tool.command.exit_code ?? undefined,
      durationMs: tool.command.duration_ms ?? undefined,
      processId: tool.command.process_id ?? undefined,
      source: tool.command.source ?? undefined,
      actions: (tool.command.actions ?? []).map((action) => ({
        kind: action.kind,
        label: action.label,
        detail: action.detail ?? undefined,
        path: action.path ?? undefined
      }))
    };
  }

  const detail =
    tool.detail ??
    tool.mcp?.arguments_summary ??
    tool.mcp?.result_summary ??
    tool.output_summary ??
    tool.input_summary ??
    (content || undefined);
  return {
    id: `event:item:${item.item_id}`,
    kind: 'inlineEvent',
    eventKind: 'tool',
    title: tool.title || tool.name || 'Tool',
    detail: detail ?? undefined,
    status: tool.status ?? item.status,
    source: item.native?.protocol ?? undefined
  };
}

function commandProjectionDetail(command: NonNullable<ItemState['tool']>['command']): string | undefined {
  if (!command) {
    return undefined;
  }
  const parts = [
    command.cwd ?? undefined,
    command.source ?? undefined,
    command.process_id ? `pid ${command.process_id}` : undefined
  ].filter(Boolean);
  return parts.length > 0 ? parts.join(' · ') : undefined;
}

function commandOutputText(blocks: ContentBlock[]): string {
  return blocks
    .filter((block) => block.kind === 'command_output' && !block.block_id.endsWith('-status'))
    .map((block) => block.text ?? '')
    .filter(Boolean)
    .join('');
}

function materializePlan(plan: PlanState): Extract<ChatItem, { kind: 'plan' }> | undefined {
  const entryMarkdown = plan.entries.map(planEntryMarkdown).join('\n');
  const content = [plan.content, entryMarkdown].filter(Boolean).join(plan.content && entryMarkdown ? '\n\n' : '');
  if (!content.trim() && plan.entries.length === 0) {
    return undefined;
  }
  return {
    id: `plan:${plan.plan_id}`,
    kind: 'plan',
    title: plan.title ?? 'Implementation plan',
    content,
    status: plan.status,
    source: plan.source,
    entries: plan.entries.map((entry) => ({
      id: entry.entry_id,
      label: entry.label,
      status: entry.status
    }))
  };
}

function materializeApproval(approval: ApprovalRequest): Extract<ChatItem, { kind: 'approval' }> {
  const terminal = terminalApprovalStatus(approval.status);
  const resolvedDecision =
    approval.status === 'approved'
      ? 'accept'
      : approval.status === 'denied'
        ? 'decline'
        : approval.status === 'cancelled'
          ? 'cancel'
          : terminal
            ? approval.status
            : undefined;
  return {
    id: `approval:${approval.approval_id}`,
    kind: 'approval',
    approvalId: approval.approval_id,
    title: approval.title,
    detail: approval.details ?? undefined,
    options: terminal
      ? []
      : approval.options.map(approvalChoiceFromOption).filter((x): x is NonNullable<typeof x> => Boolean(x)),
    status: approval.status,
    risk: approval.risk ?? undefined,
    subject: approval.subject ?? undefined,
    resolutionState: approval.status === 'resolving' ? 'resolving' : approval.status === 'pending' ? 'pending' : undefined,
    resolvedDecision
  };
}

function terminalApprovalStatus(status: string): boolean {
  return ['approved', 'denied', 'cancelled', 'expired', 'orphaned'].includes(status);
}

function materializeDiff(diff: DiffState): Extract<ChatItem, { kind: 'inlineEvent' }> {
  const detail = diff.files
    .map((file) => [file.path, file.diff].filter(Boolean).join('\n'))
    .join('\n\n');
  return {
    id: `event:diff:${diff.diff_id}`,
    kind: 'inlineEvent',
    eventKind: 'file',
    title: diff.title ?? 'Diff updated',
    detail: detail || undefined,
    status: 'updated'
  };
}

function materializeArtifact(
  artifact: { artifact_id: string; kind: string; title: string; uri?: string | null }
): Extract<ChatItem, { kind: 'inlineEvent' }> {
  return {
    id: `event:artifact:${artifact.artifact_id}`,
    kind: 'inlineEvent',
    eventKind: 'event',
    title: artifact.title,
    detail: artifact.uri ?? undefined,
    status: artifact.kind
  };
}

function itemText(blocks: ContentBlock[]): string {
  return blocks
    .map((block) => block.text ?? '')
    .filter(Boolean)
    .join('');
}

function planEntryMarkdown(entry: { label: string; status: string }): string {
  const checked = entry.status === 'completed' ? 'x' : ' ';
  return `- [${checked}] ${entry.label}`;
}

function snapshotActivity(snapshot: SessionSnapshot): ChatActivity | undefined {
  for (const turnId of snapshot.active_turns) {
    const turn = snapshot.turns[turnId];
    if (!turn) {
      continue;
    }
    return {
      status: turn.status,
      active: turn.status === 'running' || turn.status === 'waiting_for_approval' || turn.status === 'waiting_for_input',
      label:
        turn.status === 'waiting_for_approval'
          ? 'Waiting for approval'
          : turn.status === 'waiting_for_input'
            ? 'Waiting for input'
            : 'Working'
    };
  }
  return undefined;
}

function sortByUniversalOrder(items: ChatItem[], rowOrder: Map<string, RowOrder>): ChatItem[] {
  return [...items].sort((left, right) => {
    const leftOrder = rowOrder.get(left.id);
    const rightOrder = rowOrder.get(right.id);
    if (!leftOrder && !rightOrder) {
      return 0;
    }
    if (!leftOrder) {
      return 1;
    }
    if (!rightOrder) {
      return -1;
    }
    const seqOrder = compareSeq(leftOrder.seq, rightOrder.seq);
    if (seqOrder !== 0) {
      return seqOrder;
    }
    const tsOrder = leftOrder.ts.localeCompare(rightOrder.ts);
    if (tsOrder !== 0) {
      return tsOrder;
    }
    return leftOrder.index - rightOrder.index;
  });
}

function recordRowOrder(rowOrder: Map<string, RowOrder>, event: UniversalEventEnvelope) {
  for (const rowId of rowIdsForEvent(event)) {
    if (rowOrder.has(rowId)) {
      continue;
    }
    rowOrder.set(rowId, {
      seq: event.seq,
      ts: event.ts,
      index: rowOrder.size
    });
  }
}

function rowIdsForEvent(event: UniversalEventEnvelope): string[] {
  switch (event.event.type) {
    case 'item.created':
      return [rowIdForItem(event.event.data.item)];
    case 'content.delta':
    case 'content.completed':
      return [`agent:${event.item_id ?? event.event.data.block_id}`];
    case 'approval.requested':
      return [`approval:${event.event.data.approval.approval_id}`];
    case 'plan.updated':
      return [`plan:${event.event.data.plan.plan_id}`];
    case 'diff.updated':
      return [`event:diff:${event.event.data.diff.diff_id}`];
    case 'artifact.created':
      return [`event:artifact:${event.event.data.artifact.artifact_id}`];
    default:
      return [];
  }
}

function rowIdForItem(item: ItemState): string {
  if (item.role === 'user') {
    return `user:${item.item_id}`;
  }
  if (item.role === 'assistant') {
    return `agent:${item.item_id}`;
  }
  if (item.tool?.kind === 'subagent') {
    return `subagent:${item.item_id}`;
  }
  return `event:item:${item.item_id}`;
}
