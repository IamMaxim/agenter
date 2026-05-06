import type {
  ApprovalRequest,
  ArtifactState,
  ContentBlock,
  ContentBlockKind,
  DiffState,
  ItemState,
  PlanState,
  QuestionState,
  SessionSnapshot,
  SessionInfo,
  TurnState,
  UniversalEventEnvelope
} from '../api/types';

export function cloneSnapshot(snapshot: SessionSnapshot): SessionSnapshot {
  return {
    session_id: snapshot.session_id,
    latest_seq: snapshot.latest_seq ?? null,
    info: snapshot.info ?? null,
    capabilities: snapshot.capabilities,
    turns: { ...snapshot.turns },
    items: Object.fromEntries(
      Object.entries(snapshot.items).map(([id, item]) => [id, cloneItem(item)])
    ),
    questions: Object.fromEntries(
      Object.entries(snapshot.questions ?? {}).map(([id, question]) => [
        id,
        {
          ...question,
          fields: question.fields.map((field) => ({
            ...field,
            choices: [...field.choices],
            default_answers: [...field.default_answers],
            schema: cloneJson(field.schema)
          })),
          answer: question.answer
            ? {
                ...question.answer,
                answers: Object.fromEntries(
                  Object.entries(question.answer.answers).map(([key, value]) => [key, [...value]])
                )
              }
            : null
        }
      ])
    ),
    approvals: Object.fromEntries(
      Object.entries(snapshot.approvals).map(([id, approval]) => [
        id,
        { ...approval, options: [...approval.options] }
      ])
    ),
    plans: Object.fromEntries(
      Object.entries(snapshot.plans).map(([id, plan]) => [
        id,
        {
          ...plan,
          entries: [...plan.entries],
          artifact_refs: [...plan.artifact_refs]
        }
      ])
    ),
    diffs: Object.fromEntries(
      Object.entries(snapshot.diffs).map(([id, diff]) => [
        id,
        { ...diff, files: [...diff.files] }
      ])
    ),
    artifacts: { ...snapshot.artifacts },
    active_turns: [...snapshot.active_turns]
  };
}

export function applyUniversalEvent(snapshot: SessionSnapshot, envelope: UniversalEventEnvelope): SessionSnapshot {
  const next = cloneSnapshot(snapshot);
  if (compareSeq(envelope.seq, next.latest_seq) > 0) {
    next.latest_seq = envelope.seq;
  }
  switch (envelope.event.type) {
    case 'session.created':
      next.info = envelope.event.data.session;
      break;
    case 'session.status_changed':
      if (next.info) {
        next.info = { ...next.info, status: envelope.event.data.status };
      }
      break;
    case 'session.metadata_changed':
      if (next.info) {
        next.info = {
          ...next.info,
          title: envelope.event.data.title ?? undefined
        };
      }
      break;
    case 'turn.started':
    case 'turn.status_changed':
    case 'turn.completed':
    case 'turn.failed':
    case 'turn.cancelled':
    case 'turn.interrupted':
    case 'turn.detached':
      upsertTurn(next, envelope.event.data.turn);
      break;
    case 'item.created':
      next.items[envelope.event.data.item.item_id] = cloneItem(envelope.event.data.item);
      break;
    case 'content.delta':
      mergeContentDelta(
        next,
        envelope.item_id,
        envelope.event.data.block_id,
        envelope.event.data.kind ?? 'text',
        envelope.event.data.delta
      );
      break;
    case 'content.completed':
      mergeContentCompleted(
        next,
        envelope.item_id,
        envelope.event.data.block_id,
        envelope.event.data.kind ?? 'text',
        envelope.event.data.text
      );
      break;
    case 'approval.requested':
      next.approvals[envelope.event.data.approval.approval_id] = mergeApproval(
        next.approvals[envelope.event.data.approval.approval_id],
        envelope.event.data.approval
      );
      break;
    case 'approval.resolved': {
      const existing = next.approvals[envelope.event.data.approval_id];
      if (existing) {
        next.approvals[envelope.event.data.approval_id] = {
          ...cloneApproval(existing),
          status: envelope.event.data.status,
          resolved_at: envelope.event.data.resolved_at,
          native: envelope.event.data.native ?? existing.native
        };
      }
      break;
    }
    case 'question.requested':
    case 'question.answered':
      next.questions[envelope.event.data.question.question_id] = mergeQuestion(
        next.questions[envelope.event.data.question.question_id],
        envelope.event.data.question
      );
      break;
    case 'plan.updated':
      next.plans[envelope.event.data.plan.plan_id] = mergePlan(
        next.plans[envelope.event.data.plan.plan_id],
        envelope.event.data.plan
      );
      break;
    case 'diff.updated':
      next.diffs[envelope.event.data.diff.diff_id] = cloneDiff(envelope.event.data.diff);
      break;
    case 'artifact.created':
      next.artifacts[envelope.event.data.artifact.artifact_id] = cloneArtifact(envelope.event.data.artifact);
      break;
    case 'usage.updated':
      next.info = { ...(next.info ?? minimalSessionInfo(envelope.session_id)), usage: envelope.event.data.usage };
      break;
    case 'error.reported':
      next.artifacts[`error:${envelope.event_id}`] = {
        artifact_id: `error:${envelope.event_id}`,
        session_id: envelope.session_id,
        turn_id: envelope.turn_id ?? null,
        kind: 'error',
        title: errorEventTitle(envelope.event.data.code, envelope.native?.method),
        uri: envelope.event.data.message,
        mime_type: null,
        native: envelope.native ?? null,
        created_at: envelope.ts
      };
      break;
    case 'provider.notification':
      next.artifacts[`provider:${envelope.event_id}`] = {
        artifact_id: `provider:${envelope.event_id}`,
        session_id: envelope.session_id,
        turn_id: envelope.turn_id ?? null,
        kind: 'native',
        title: envelope.event.data.notification.title,
        uri: providerNotificationDetail(envelope.event.data.notification),
        mime_type: null,
        native: envelope.native ?? null,
        created_at: envelope.ts
      };
      break;
    case 'native.unknown':
      next.artifacts[`native:${envelope.event_id}`] = {
        artifact_id: `native:${envelope.event_id}`,
        session_id: envelope.session_id,
        turn_id: envelope.turn_id ?? null,
        kind: isPromotedNativeMethod(envelope.native?.method) ? 'native' : 'native_raw',
        title: nativeEventTitle(envelope.native?.method, envelope.event.data.summary),
        uri: nativeEventDetail(envelope.native?.method, envelope.event.data.summary),
        mime_type: null,
        native: envelope.native ?? null,
        created_at: envelope.ts
      };
      break;
    default:
      break;
  }
  return next;
}

function providerNotificationDetail(notification: {
  category: string;
  detail?: string | null;
  status?: string | null;
  severity?: string | null;
  subject?: string | null;
}): string | undefined {
  const parts = [
    notification.detail,
    notification.status ? `status: ${notification.status}` : undefined,
    notification.subject ? `subject: ${notification.subject}` : undefined
  ].filter((part): part is string => Boolean(part));
  return parts.length > 0 ? parts.join('\n') : notification.category;
}

function errorEventTitle(code: string | null | undefined, method: string | null | undefined): string {
  if (code?.endsWith('_auth_refresh_required')) return 'Provider auth refresh required';
  if (code?.endsWith('_capability_gap')) return 'Provider capability gap';
  if (code?.endsWith('_unknown_server_request')) return 'Unknown provider server request';
  return method ? nativeEventTitle(method, code ?? undefined) : (code ?? 'Provider error');
}

function nativeEventTitle(method: string | null | undefined, fallback: string | null | undefined): string {
  switch (method) {
    case 'thread/started': return 'Thread started';
    case 'thread/archived': return 'Thread archived';
    case 'thread/unarchived': return 'Thread unarchived';
    case 'thread/closed': return 'Thread closed';
    case 'thread/name/updated': return 'Thread name updated';
    case 'thread/contextWindow/updated': return 'Thread context window updated';
    case 'hook/started': return 'Hook started';
    case 'hook/completed': return 'Hook completed';
    case 'item/autoApprovalReview/started': return 'Auto approval review started';
    case 'item/autoApprovalReview/completed': return 'Auto approval review completed';
    case 'guardianWarning': return 'Guardian warning';
    case 'item/commandExecution/terminalInteraction': return 'Terminal interaction';
    case 'item/mcpToolCall/progress': return 'MCP tool call progress';
    case 'mcpServer/oauthLogin/completed': return 'MCP OAuth login completed';
    case 'mcpServer/startupStatus/updated': return 'MCP server startup status updated';
    case 'account/updated': return 'Account updated';
    case 'account/rateLimits/updated': return 'Rate limits updated';
    case 'model/rerouted': return 'Model rerouted';
    case 'model/verification': return 'Model verification';
    case 'warning': return 'Warning';
    case 'deprecationNotice': return 'Deprecation notice';
    case 'configWarning': return 'Configuration warning';
    case 'fuzzyFileSearch/sessionUpdated': return 'Fuzzy file search updated';
    case 'fuzzyFileSearch/sessionCompleted': return 'Fuzzy file search completed';
    case 'fs/changed': return 'Filesystem changed';
    case 'windows/worldWritableWarning': return 'World-writable path warning';
    case 'windowsSandbox/setupCompleted': return 'Windows sandbox setup completed';
    default: return fallback ?? method ?? 'Provider event';
  }
}

function nativeEventDetail(method: string | null | undefined, summary: string | null | undefined): string | undefined {
  if (!method || summary === method || summary === 'native notification') {
    return summary ?? undefined;
  }
  return summary ? `${method}\n${summary}` : method;
}

function isPromotedNativeMethod(method: string | null | undefined): boolean {
  return Boolean(
    method &&
      (method.startsWith('thread/') ||
        method.startsWith('hook/') ||
        method.startsWith('item/autoApprovalReview/') ||
        method === 'item/commandExecution/terminalInteraction' ||
        method.startsWith('item/mcpToolCall/') ||
        method.startsWith('mcpServer/') ||
        method.startsWith('account/') ||
        method.startsWith('model/') ||
        method.startsWith('fuzzyFileSearch/') ||
        method.startsWith('fs/') ||
        method.startsWith('windows') ||
        method === 'guardianWarning' ||
        method === 'warning' ||
        method === 'deprecationNotice' ||
        method === 'configWarning')
  );
}

export function universalEventKey(event: Pick<UniversalEventEnvelope, 'seq' | 'event_id'>): string {
  return `${event.seq}:${event.event_id}`;
}

export function compareSeq(left: string | null | undefined, right: string | null | undefined): number {
  const l = BigInt(left ?? '0');
  const r = BigInt(right ?? '0');
  return l === r ? 0 : l > r ? 1 : -1;
}

function upsertTurn(snapshot: SessionSnapshot, turn: TurnState) {
  snapshot.turns[turn.turn_id] = turn;
  const active =
    turn.status === 'running' ||
    turn.status === 'waiting_for_approval' ||
    turn.status === 'waiting_for_input' ||
    turn.status === 'interrupting';
  const without = snapshot.active_turns.filter((turnId) => turnId !== turn.turn_id);
  snapshot.active_turns = active ? [...without, turn.turn_id] : without;
}

function mergeApproval(
  existing: ApprovalRequest | undefined,
  incoming: ApprovalRequest
): ApprovalRequest {
  if (!existing) {
    return cloneApproval(incoming);
  }
  const incomingIsTerminal = terminalApprovalStatus(incoming.status);
  const shouldReplaceStatus = !terminalApprovalStatus(existing.status) || incomingIsTerminal;
  const hasRequestDetails = incoming.title !== 'Approval resolved';
  return {
    ...existing,
    turn_id: incoming.turn_id ?? existing.turn_id,
    item_id: incoming.item_id ?? existing.item_id,
    kind: hasRequestDetails ? incoming.kind : existing.kind,
    title: hasRequestDetails ? incoming.title : existing.title,
    details: incoming.details ?? existing.details,
    options: incoming.options.length > 0 ? incoming.options.map((option) => ({ ...option })) : [...existing.options],
    status: shouldReplaceStatus ? incoming.status : existing.status,
    risk: incoming.risk ?? existing.risk,
    subject: incoming.subject ?? existing.subject,
    native_request_id: incoming.native_request_id ?? existing.native_request_id,
    native_blocking: incoming.native_blocking ?? existing.native_blocking,
    policy: incoming.policy ?? existing.policy,
    native: incoming.native ?? existing.native,
    requested_at: incoming.requested_at ?? existing.requested_at,
    resolved_at: incoming.resolved_at ?? existing.resolved_at
  };
}

function cloneApproval(approval: ApprovalRequest): ApprovalRequest {
  return {
    ...approval,
    options: approval.options.map((option) => ({ ...option }))
  };
}

function terminalApprovalStatus(status: string): boolean {
  return ['approved', 'denied', 'cancelled', 'expired', 'orphaned'].includes(status);
}

function mergeQuestion(existing: QuestionState | undefined, incoming: QuestionState): QuestionState {
  if (!existing) {
    return cloneQuestion(incoming);
  }
  const hasRequestDetails = incoming.title !== 'Input requested';
  return {
    ...existing,
    turn_id: incoming.turn_id ?? existing.turn_id,
    title: hasRequestDetails ? incoming.title : existing.title,
    description: incoming.description ?? existing.description,
    fields: incoming.fields.length > 0 ? cloneQuestionFields(incoming) : cloneQuestionFields(existing),
    status: incoming.status,
    answer: incoming.answer ?? existing.answer,
    native_request_id: incoming.native_request_id ?? existing.native_request_id,
    native_blocking: incoming.native_blocking ?? existing.native_blocking,
    native: incoming.native ?? existing.native,
    requested_at: incoming.requested_at ?? existing.requested_at,
    answered_at: incoming.answered_at ?? existing.answered_at
  };
}

function cloneQuestion(question: QuestionState): QuestionState {
  return {
    ...question,
    fields: cloneQuestionFields(question),
    answer: question.answer
      ? {
          ...question.answer,
          answers: Object.fromEntries(
            Object.entries(question.answer.answers).map(([key, value]) => [key, [...value]])
          )
        }
      : null
  };
}

function cloneQuestionFields(question: QuestionState): QuestionState['fields'] {
  return question.fields.map((field) => ({
    ...field,
    choices: [...field.choices],
    default_answers: [...field.default_answers],
    schema: cloneJson(field.schema)
  }));
}

function cloneJson<T>(value: T): T {
  if (value === undefined || value === null) {
    return value;
  }
  return structuredClone(value);
}

function mergeContentDelta(
  snapshot: SessionSnapshot,
  itemId: string | null | undefined,
  blockId: string,
  kind: ContentBlockKind,
  delta: string
) {
  const item = findOrCreateItemForBlock(snapshot, itemId, blockId, kind);
  const block = findOrCreateBlock(item, blockId, kind);
  block.text = `${block.text ?? ''}${delta}`;
  item.status = 'streaming';
  if (item.tool) {
    item.tool = { ...item.tool, status: 'streaming' };
  }
}

function mergeContentCompleted(
  snapshot: SessionSnapshot,
  itemId: string | null | undefined,
  blockId: string,
  kind: ContentBlockKind,
  text: string | null | undefined
) {
  const item = findOrCreateItemForBlock(snapshot, itemId, blockId, kind);
  const block = findOrCreateBlock(item, blockId, kind);
  if (typeof text === 'string') {
    block.text = text;
  }
  item.status = 'completed';
  if (item.tool) {
    item.tool = { ...item.tool, status: 'completed' };
  }
}

function findOrCreateItemForBlock(
  snapshot: SessionSnapshot,
  itemId: string | null | undefined,
  blockId: string,
  kind: ContentBlockKind
): ItemState {
  if (!itemId) {
    const existingWithBlock = Object.values(snapshot.items).find((item) =>
      item.content.some((block) => block.block_id === blockId)
    );
    if (existingWithBlock) {
      return existingWithBlock;
    }
  }
  const resolvedItemId = itemId ?? blockId;
  const existing = snapshot.items[resolvedItemId];
  if (existing) {
    return existing;
  }
  const role =
    kind === 'command_output' ||
    kind === 'terminal_input' ||
    kind === 'tool_call' ||
    kind === 'tool_result'
      ? 'tool'
      : 'assistant';
  const item: ItemState = {
    item_id: resolvedItemId,
    session_id: snapshot.session_id,
    role,
    status: 'streaming',
    content: []
  };
  snapshot.items[resolvedItemId] = item;
  return item;
}

function findOrCreateBlock(item: ItemState, blockId: string, kind: ContentBlockKind): ContentBlock {
  const existing = item.content.find((block) => block.block_id === blockId);
  if (existing) {
    return existing;
  }
  const block: ContentBlock = { block_id: blockId, kind, text: '' };
  item.content = [...item.content, block];
  return block;
}

function mergePlan(existing: PlanState | undefined, incoming: PlanState): PlanState {
  if (!existing || !incoming.partial) {
    return clonePlan(incoming);
  }
  const entries = [...existing.entries];
  for (const entry of incoming.entries) {
    const index = entries.findIndex((existingEntry) => existingEntry.entry_id === entry.entry_id);
    if (index === -1) {
      entries.push(entry);
    } else {
      entries[index] = entry;
    }
  }
  return {
    ...existing,
    ...incoming,
    content:
      typeof incoming.content === 'string'
        ? `${existing.content ?? ''}${incoming.content}`
        : existing.content,
    entries,
    artifact_refs: incoming.artifact_refs.length > 0 ? incoming.artifact_refs : existing.artifact_refs
  };
}

function cloneItem(item: ItemState): ItemState {
  return {
    ...item,
    content: item.content.map((block) => ({ ...block }))
  };
}

function clonePlan(plan: PlanState): PlanState {
  return {
    ...plan,
    entries: [...plan.entries],
    artifact_refs: [...plan.artifact_refs]
  };
}

function cloneDiff(diff: DiffState): DiffState {
  return {
    ...diff,
    files: [...diff.files]
  };
}

function cloneArtifact(artifact: ArtifactState): ArtifactState {
  return { ...artifact, native: artifact.native ? cloneNativeRef(artifact.native) : artifact.native };
}

function cloneNativeRef<T extends { raw_payload?: unknown }>(native: T): T {
  return {
    ...native,
    raw_payload: cloneJson(native.raw_payload)
  };
}

function minimalSessionInfo(sessionId: string): SessionInfo {
  return {
    session_id: sessionId,
    owner_user_id: '',
    runner_id: '',
    workspace_id: '',
    provider_id: 'unknown',
    status: 'degraded',
    external_session_id: null,
    title: null,
    created_at: null,
    updated_at: null,
    usage: null
  };
}
