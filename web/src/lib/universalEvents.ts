import type {
  ArtifactState,
  ContentBlock,
  ContentBlockKind,
  DiffState,
  ItemState,
  PlanState,
  SessionSnapshot,
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
            default_answers: [...field.default_answers]
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
      next.approvals[envelope.event.data.approval.approval_id] = {
        ...envelope.event.data.approval,
        options: [...envelope.event.data.approval.options]
      };
      break;
    case 'question.requested':
    case 'question.answered':
      next.questions[envelope.event.data.question.question_id] = {
        ...envelope.event.data.question,
        fields: envelope.event.data.question.fields.map((field) => ({
          ...field,
          choices: [...field.choices],
          default_answers: [...field.default_answers]
        }))
      };
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
      if (next.info) {
        next.info = { ...next.info, usage: envelope.event.data.usage };
      }
      break;
    default:
      break;
  }
  return next;
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
  const active = turn.status === 'running' || turn.status === 'waiting_for_approval' || turn.status === 'waiting_for_input';
  const without = snapshot.active_turns.filter((turnId) => turnId !== turn.turn_id);
  snapshot.active_turns = active ? [...without, turn.turn_id] : without;
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
  const role = kind === 'command_output' || kind === 'tool_call' || kind === 'tool_result' ? 'tool' : 'assistant';
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
  return { ...artifact };
}
