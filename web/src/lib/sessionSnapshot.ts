import type {
  ApprovalRequest,
  BrowserServerMessage,
  CapabilitySet,
  ContentBlock,
  DiffState,
  ItemState,
  PlanState,
  QuestionState,
  SessionSnapshot,
  SessionUsageSnapshot,
  UniversalEventEnvelope
} from '../api/types';
import {
  approvalChoiceFromOption,
  createChatState,
  type ChatActivity,
  type ChatItem,
  type ChatState
} from './chatEvents';
import { applyUniversalEvent, cloneSnapshot, compareSeq, universalEventKey } from './universalEvents';

export interface UniversalClientState {
  chat: ChatState;
  snapshot?: SessionSnapshot;
  latestSeq?: string;
  latestUsage?: SessionUsageSnapshot | null;
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
    latestSeq: undefined,
    latestUsage: null,
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
  for (const question of Object.values(snapshot.questions ?? {})) {
    items.push(materializeQuestion(question));
  }
  for (const diff of Object.values(snapshot.diffs)) {
    items.push(materializeDiff(diff));
  }
  for (const artifact of Object.values(snapshot.artifacts)) {
    items.push(materializeArtifact(artifact));
  }

  ensureSnapshotRowOrder(items, rowOrder, snapshot);

  const sortedItems = sortByUniversalOrder(items, rowOrder);
  const latestPlan = [...sortedItems].reverse().find((item) => item.kind === 'plan');
  return {
    seenEventIds: new Set(),
    items: sortedItems,
    activity: snapshotActivity(snapshot),
    latestPlanId: latestPlan?.id,
    planTurnComplete: !snapshot.active_turns.some((turnId) => {
      const status = snapshot.turns[turnId]?.status;
      return status === 'running' || status === 'waiting_for_approval' || status === 'waiting_for_input' || status === 'interrupting';
    })
  };
}

export function hasCapabilitySignal(capabilities: CapabilitySet | undefined): boolean {
  if (!capabilities) {
    return false;
  }
  return Object.entries(capabilities).some(([key, group]) => {
    if (key === 'provider_details') {
      return false;
    }
    if (!group || typeof group !== 'object' || Array.isArray(group)) {
      return false;
    }
    return Object.values(group as Record<string, unknown>).some((value) => value === true);
  });
}

function applySessionSnapshotMessage(
  state: UniversalClientState,
  message: Extract<BrowserServerMessage, { type: 'session_snapshot' }>
): UniversalClientState {
  let snapshot = cloneSnapshot(message.snapshot);
  if (message.has_more) {
    const rowOrder = new Map(state.rowOrder);
    const seenUniversalEvents = new Set(state.seenUniversalEvents);
    for (const event of message.events) {
      seenUniversalEvents.add(universalEventKey(event));
      recordRowOrder(rowOrder, event);
    }
    return {
      chat: materializeSnapshotChatState(snapshot, rowOrder),
      snapshot,
      latestSeq: snapshot.latest_seq ?? undefined,
      latestUsage: snapshot.info?.usage ?? state.latestUsage ?? null,
      usingUniversal: true,
      snapshotIncomplete: true,
      seenUniversalEvents,
      rowOrder
    };
  }

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
    chat: materializeSnapshotChatState(snapshot, rowOrder),
    snapshot,
    latestSeq,
    latestUsage: snapshot.info?.usage ?? state.latestUsage ?? null,
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
    chat: materializeSnapshotChatState(snapshot, rowOrder),
    snapshot,
    latestSeq: compareSeq(event.seq, state.latestSeq) > 0 ? event.seq : state.latestSeq,
    latestUsage:
      event.event.type === 'usage.updated'
        ? event.event.data.usage
        : snapshot.info?.usage ?? state.latestUsage ?? null,
    usingUniversal: true,
    snapshotIncomplete: false,
    seenUniversalEvents,
    rowOrder
  };
}

function emptySnapshot(sessionId: string, latestSeq?: string): SessionSnapshot {
  return {
    session_id: sessionId,
    latest_seq: latestSeq,
    turns: {},
    items: {},
    approvals: {},
    questions: {},
    plans: {},
    diffs: {},
    artifacts: {},
    active_turns: []
  };
}

function materializeQuestion(question: QuestionState): Extract<ChatItem, { kind: 'question' }> {
  const terminal = terminalQuestionStatus(question.status);
  return {
    id: `question:${question.question_id}`,
    kind: 'question',
    questionId: question.question_id,
    title: question.title,
    description: question.description ?? undefined,
    fields: question.fields.map((field) => ({
      id: field.id,
      label: field.label,
      prompt: field.prompt ?? undefined,
      kind: field.kind,
      required: field.required,
      secret: field.secret,
      choices: field.choices ?? [],
      default_answers: field.default_answers ?? []
    })),
    answered: question.status === 'answered',
    status: question.status,
    resolvedState: terminal ? question.status : undefined
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
  if (item.role === 'assistant' && !hasSemanticEventContent(item.content)) {
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
  const nativeMethod = item.native?.method;
  const isNativeCommand =
    isNativeCommandMethod(nativeMethod) ||
    (first?.kind === 'tool_call' &&
      item.content.some((block) => block.block_id.startsWith('command-')));
  const isNativeFileChange =
    isFileChangeMethod(nativeMethod) ||
    item.content.some((block) => isFileDiffBlock(block.kind));
  if (item.content.some((block) => block.kind === 'warning')) {
    return {
      id: `event:item:${item.item_id}`,
      kind: 'inlineEvent',
      eventKind: 'event',
      title: item.native?.summary ?? 'Warning',
      detail: content || undefined,
      status: item.status,
      source: item.native?.protocol ?? undefined
    };
  }
  if (item.content.some((block) => block.kind === 'provider_status')) {
    return {
      id: `event:item:${item.item_id}`,
      kind: 'inlineEvent',
      eventKind: 'event',
      title: item.native?.summary ?? 'Provider status',
      detail: content || undefined,
      status: item.status,
      source: item.native?.protocol ?? undefined
    };
  }
  if (item.content.some((block) => block.kind === 'reasoning')) {
    return {
      id: `event:item:${item.item_id}`,
      kind: 'inlineEvent',
      eventKind: 'tool',
      displayLevel: 'thinking',
      title: item.native?.summary ?? 'Reasoning',
      detail: content || undefined,
      status: item.status,
      source: item.native?.protocol ?? undefined
    };
  }
  if (item.content.some((block) => block.kind === 'image')) {
    return {
      id: `event:item:${item.item_id}`,
      kind: 'inlineEvent',
      eventKind: 'event',
      title: item.native?.summary ?? 'Image',
      detail: imageDetail(item.content) || content || undefined,
      status: item.status,
      source: item.native?.protocol ?? undefined
    };
  }
  if (item.content.some((block) => block.kind === 'native')) {
    return {
      id: `event:item:${item.item_id}`,
      kind: 'inlineEvent',
      eventKind: 'event',
      displayLevel: 'raw',
      title: item.native?.summary ?? item.native?.method ?? 'Native event',
      detail: content || undefined,
      status: item.status,
      source: item.native?.protocol ?? undefined
    };
  }
  if (first?.kind === 'command_output' || first?.kind === 'terminal_input') {
    return {
      id: `event:item:${item.item_id}`,
      kind: 'inlineEvent',
      eventKind: 'command',
      title: item.native?.summary ?? item.native?.method ?? 'Command output',
      detail: undefined,
      output: commandOutputText(item.content),
      status: item.status,
      source: item.native?.protocol ?? undefined
    };
  }
  if (isNativeCommand) {
    return {
      id: `event:item:${item.item_id}`,
      kind: 'inlineEvent',
      eventKind: 'command',
      title: first?.text?.trim() || 'Command',
      detail: undefined,
      output: commandOutputText(item.content),
      status: item.status,
      source: item.native?.protocol ?? undefined
    };
  }
  if (isNativeFileChange) {
    return {
      id: `event:item:${item.item_id}`,
      kind: 'inlineEvent',
      eventKind: 'file',
      title:
        methodFileChangeTitle(item.native?.method) ??
        item.native?.summary ??
        (first?.text?.trim() || 'File change'),
      detail: fileChangeDetail(item.content) || content || undefined,
      status: item.status,
      source: item.native?.protocol ?? undefined
    };
  }
  return {
    id: `event:item:${item.item_id}`,
    kind: 'inlineEvent',
    eventKind: 'tool',
    title: item.native?.summary ?? item.native?.method ?? first?.kind ?? 'Tool activity',
    detail: content || undefined,
    status: item.status,
    source: item.native?.protocol ?? undefined
  };
}

function materializeToolItem(item: ItemState, content: string): ChatItem {
  const tool = item.tool!;
  const isToolFileChange =
    isFileChangeMethod(item.native?.method) ||
    item.content.some((block) => isFileDiffBlock(block.kind));
  if (isToolFileChange) {
    return {
      id: `event:item:${item.item_id}`,
      kind: 'inlineEvent',
      eventKind: 'file',
      title:
        methodFileChangeTitle(item.native?.method) ??
        tool.title ??
        item.native?.summary ??
        tool.name ??
        'File change',
      detail: fileChangeDetail(item.content) || content || undefined,
      status: tool.status ?? item.status,
      source: item.native?.protocol ?? undefined
    };
  }
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

function methodFileChangeTitle(method: string | null | undefined): string | undefined {
  if (!method?.startsWith('file_change_')) {
    return undefined;
  }
  return `File change ${method.replace(/^file_change_/, '').replace(/_/g, ' ')}`.trim();
}

function fileChangeDetail(blocks: ContentBlock[]): string | undefined {
  const text = blocks
    .filter((block) => isFileDiffBlock(block.kind))
    .map((block) => block.text ?? '')
    .filter(Boolean)
    .join('');
  return text.length > 0 ? text : undefined;
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
    .filter((block) => (block.kind === 'command_output' || block.kind === 'terminal_input') && !block.block_id.endsWith('-status'))
    .map((block) => (block.kind === 'terminal_input' ? `$ ${block.text ?? ''}` : block.text ?? ''))
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
  return ['approved', 'denied', 'cancelled', 'expired', 'orphaned', 'detached'].includes(status);
}

function terminalQuestionStatus(status: string): boolean {
  return ['answered', 'cancelled', 'expired', 'orphaned', 'detached'].includes(status);
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
): ChatItem {
  if (artifact.kind === 'error') {
    return {
      id: `event:artifact:${artifact.artifact_id}`,
      kind: 'error',
      title: artifact.title,
      detail: artifact.uri ?? undefined
    };
  }
  return {
    id: `event:artifact:${artifact.artifact_id}`,
    kind: 'inlineEvent',
    eventKind: 'event',
    displayLevel: artifact.kind === 'native_raw' ? 'raw' : 'normal',
    title: artifact.title,
    detail: artifact.uri ?? undefined,
    status: artifact.kind === 'native_raw' ? 'native' : artifact.kind
  };
}

function itemText(blocks: ContentBlock[]): string {
  return blocks
    .map((block) => block.text ?? '')
    .filter(Boolean)
    .join('');
}

function hasSemanticEventContent(blocks: ContentBlock[]): boolean {
  return blocks.some((block) =>
    block.kind === 'reasoning' ||
    block.kind === 'image' ||
    block.kind === 'native' ||
    block.kind === 'warning' ||
    block.kind === 'provider_status'
  );
}

function imageDetail(blocks: ContentBlock[]): string | undefined {
  const parts = blocks
    .filter((block) => block.kind === 'image')
    .flatMap((block) => [block.artifact_id ?? undefined, block.text ?? undefined])
    .filter((part): part is string => Boolean(part));
  return parts.length > 0 ? parts.join('\n') : undefined;
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
      active:
        turn.status === 'running' ||
        turn.status === 'waiting_for_approval' ||
        turn.status === 'waiting_for_input' ||
        turn.status === 'interrupting',
      label:
        turn.status === 'waiting_for_approval'
          ? 'Waiting for approval'
          : turn.status === 'waiting_for_input'
            ? 'Waiting for input'
            : turn.status === 'interrupting'
              ? 'Stopping'
            : 'Working'
    };
  }
  return undefined;
}

function ensureSnapshotRowOrder(
  items: ChatItem[],
  rowOrder: Map<string, RowOrder>,
  snapshot: SessionSnapshot
) {
  const snapshotSeq = fallbackRowOrderSeq(snapshot.latest_seq);
  for (const item of items) {
    const existingOrder = rowOrder.get(item.id);
    const timestampOrder = snapshotTimestampOrder(item, rowOrder, snapshot);
    if (existingOrder && !shouldReanchorSnapshotRow(item, existingOrder, timestampOrder)) {
      continue;
    }
    rowOrder.set(
      item.id,
      timestampOrder ?? {
        seq: snapshotSeq,
        ts: '0001-01-01T00:00:00.000Z',
        index: rowOrder.size
      }
    );
  }
}

function snapshotTimestampOrder(
  item: ChatItem,
  rowOrder: Map<string, RowOrder>,
  snapshot: SessionSnapshot
): RowOrder | undefined {
  const ts = snapshotItemTimestamp(item, snapshot);
  if (!ts) {
    return undefined;
  }
  const anchors = [...rowOrder.entries()]
    .filter(([rowId, order]) => rowId !== item.id && order.ts !== '0001-01-01T00:00:00.000Z')
    .map(([, order]) => order)
    .sort((left, right) => {
      const tsOrder = left.ts.localeCompare(right.ts);
      if (tsOrder !== 0) {
        return tsOrder;
      }
      return compareSeq(left.seq, right.seq);
    });
  const nextAnchor = anchors.find((order) => order.ts >= ts);
  return {
    seq: nextAnchor?.seq ?? fallbackRowOrderSeq(snapshot.latest_seq),
    ts,
    index: nextAnchor?.index ?? rowOrder.size
  };
}

function snapshotItemTimestamp(item: ChatItem, snapshot: SessionSnapshot): string | undefined {
  if (item.kind === 'approval') {
    const approval = snapshot.approvals[item.approvalId];
    return approval?.requested_at ?? approval?.resolved_at ?? undefined;
  }
  if (item.kind === 'question') {
    const question = snapshot.questions[item.questionId];
    return question?.requested_at ?? question?.answered_at ?? undefined;
  }
  if (item.kind === 'plan') {
    return snapshot.plans[item.id.replace(/^plan:/, '')]?.updated_at ?? undefined;
  }
  if (item.kind === 'inlineEvent' && item.id.startsWith('event:diff:')) {
    return snapshot.diffs[item.id.replace(/^event:diff:/, '')]?.updated_at ?? undefined;
  }
  if ((item.kind === 'inlineEvent' || item.kind === 'error') && item.id.startsWith('event:artifact:')) {
    return snapshot.artifacts[item.id.replace(/^event:artifact:/, '')]?.created_at ?? undefined;
  }
  return undefined;
}

function shouldReanchorSnapshotRow(
  item: ChatItem,
  existingOrder: RowOrder,
  timestampOrder: RowOrder | undefined
): boolean {
  if (!timestampOrder || (item.kind !== 'approval' && item.kind !== 'question')) {
    return false;
  }
  return existingOrder.ts === '0001-01-01T00:00:00.000Z' || existingOrder.ts > timestampOrder.ts;
}

function fallbackRowOrderSeq(latestSeq?: string | null): string {
  if (!latestSeq) {
    return '0';
  }
  try {
    const value = BigInt(latestSeq);
    return value > 0n ? String(value - 1n) : '0';
  } catch {
    return '0';
  }
}

function isNativeCommandMethod(method: string | null | undefined): boolean {
  return !!method && (method === 'command_started' || method.startsWith('command_'));
}

function isFileChangeMethod(method: string | null | undefined): boolean {
  return !!method && method.startsWith('file_change');
}

function isFileDiffBlock(kind: string): boolean {
  return kind === 'file_diff' || kind.startsWith('file_diff');
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
      return [rowIdForContentEvent(event.item_id, event.event.data.block_id, event.event.data.kind)];
    case 'approval.requested':
      return [`approval:${event.event.data.approval.approval_id}`];
    case 'approval.resolved':
      return [`approval:${event.event.data.approval_id}`];
    case 'question.requested':
    case 'question.answered':
      return [`question:${event.event.data.question.question_id}`];
    case 'plan.updated':
      return [`plan:${event.event.data.plan.plan_id}`];
    case 'diff.updated':
      return [`event:diff:${event.event.data.diff.diff_id}`];
    case 'artifact.created':
      return [`event:artifact:${event.event.data.artifact.artifact_id}`];
    case 'error.reported':
      return [`event:artifact:error:${event.event_id}`];
    case 'provider.notification':
      return [`event:artifact:provider:${event.event_id}`];
    case 'native.unknown':
      return [`event:artifact:native:${event.event_id}`];
    default:
      return [];
  }
}

function rowIdForItem(item: ItemState): string {
  if (item.role === 'user') {
    return `user:${item.item_id}`;
  }
  if (item.role === 'assistant' && !hasSemanticEventContent(item.content)) {
    return `agent:${item.item_id}`;
  }
  if (item.tool?.kind === 'subagent') {
    return `subagent:${item.item_id}`;
  }
  return `event:item:${item.item_id}`;
}

function rowIdForContentEvent(
  envelopeItemId: string | null | undefined,
  blockId: string,
  blockKind: string | null | undefined
): string {
  const itemId = envelopeItemId ?? blockId;
  const kind = blockKind ?? 'text';
  if (kind === 'text') {
    return `agent:${itemId}`;
  }
  if (kind === 'tool_call' || kind === 'tool_result' || kind === 'command_output' || kind === 'terminal_input') {
    return `event:item:${itemId}`;
  }
  if (kind === 'file_diff' || kind.startsWith('file_diff')) {
    return `event:item:${itemId}`;
  }
  if (kind === 'reasoning' || kind === 'image' || kind === 'native' || kind === 'warning' || kind === 'provider_status') {
    return `event:item:${itemId}`;
  }
  return `event:item:${itemId}`;
}
