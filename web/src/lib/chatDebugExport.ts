import type { BrowserServerMessage, SessionInfo, SessionSnapshot } from '../api/types';
import type { ChatItem, ChatState } from './chatEvents';
import type { UniversalClientState } from './sessionSnapshot';

declare global {
  interface Window {
    __agenterExportChatDebug?: () => unknown;
  }
}

export interface ChatDebugWsEntry {
  receivedAt: string;
  message: BrowserServerMessage;
  stateAfter: ChatDebugStateSummary;
}

export interface ChatDebugExportInput {
  activeSessionId: string;
  connectionState: string;
  session?: SessionInfo;
  verbosity: string;
  chatState: ChatState;
  visibleItems: ChatItem[];
  universalState: UniversalClientState;
  wsMessages: ChatDebugWsEntry[];
}

export interface ChatDebugStateSummary {
  latestSeq?: string;
  snapshotIncomplete: boolean;
  usingUniversal: boolean;
  itemCount: number;
  visibleItemCount: number;
  rowOrderCount: number;
}

export function summarizeChatDebugState(input: {
  chatState: ChatState;
  visibleItems: ChatItem[];
  universalState: UniversalClientState;
}): ChatDebugStateSummary {
  return {
    latestSeq: input.universalState.latestSeq,
    snapshotIncomplete: input.universalState.snapshotIncomplete,
    usingUniversal: input.universalState.usingUniversal,
    itemCount: input.chatState.items.length,
    visibleItemCount: input.visibleItems.length,
    rowOrderCount: input.universalState.rowOrder.size
  };
}

export function buildChatDebugExport(input: ChatDebugExportInput) {
  const snapshot = input.universalState.snapshot;
  return {
    version: 1,
    exportedAt: new Date().toISOString(),
    activeSessionId: input.activeSessionId,
    connectionState: input.connectionState,
    verbosity: input.verbosity,
    session: input.session ?? null,
    universal: {
      latestSeq: input.universalState.latestSeq,
      snapshotIncomplete: input.universalState.snapshotIncomplete,
      usingUniversal: input.universalState.usingUniversal,
      seenUniversalEventCount: input.universalState.seenUniversalEvents.size,
      rowOrder: [...input.universalState.rowOrder.entries()].map(([rowId, order]) => ({
        rowId,
        ...order
      }))
    },
    snapshot: snapshot ?? null,
    wsMessages: input.wsMessages,
    chatItems: input.chatState.items.map((item) => decorateChatItem(item, snapshot, input.universalState)),
    visibleItems: input.visibleItems.map((item) => decorateChatItem(item, snapshot, input.universalState))
  };
}

function decorateChatItem(
  item: ChatItem,
  snapshot: SessionSnapshot | undefined,
  universalState: UniversalClientState
) {
  return {
    item,
    diagnostics: {
      id: item.id,
      kind: item.kind,
      status: itemStatus(item),
      resolvedDecision: item.kind === 'approval' ? item.resolvedDecision : undefined,
      rowOrder: universalState.rowOrder.get(item.id) ?? null,
      source: sourceSnapshotState(item, snapshot)
    }
  };
}

function itemStatus(item: ChatItem): string | undefined {
  if (item.kind === 'inlineEvent' || item.kind === 'subagent' || item.kind === 'plan' || item.kind === 'approval') {
    return item.status;
  }
  if (item.kind === 'assistant') {
    return item.completed ? 'completed' : 'streaming';
  }
  if (item.kind === 'question') {
    return item.answered ? 'answered' : 'pending';
  }
  return undefined;
}

function sourceSnapshotState(item: ChatItem, snapshot: SessionSnapshot | undefined) {
  if (!snapshot) {
    return null;
  }
  if (item.kind === 'approval') {
    const approval = snapshot.approvals[item.approvalId];
    return approval
      ? {
          kind: 'approval',
          approval_id: approval.approval_id,
          status: approval.status,
          title: approval.title,
          requested_at: approval.requested_at ?? null,
          resolved_at: approval.resolved_at ?? null,
          native_request_id: approval.native_request_id ?? null
        }
      : null;
  }
  if (item.kind === 'question') {
    const question = snapshot.questions[item.questionId];
    return question
      ? {
          kind: 'question',
          question_id: question.question_id,
          status: question.status,
          title: question.title,
          requested_at: question.requested_at ?? null,
          answered_at: question.answered_at ?? null
        }
      : null;
  }
  if (item.kind === 'plan') {
    const plan = snapshot.plans[item.id.replace(/^plan:/, '')];
    return plan
      ? {
          kind: 'plan',
          plan_id: plan.plan_id,
          status: plan.status,
          updated_at: plan.updated_at ?? null
        }
      : null;
  }
  if (item.kind === 'inlineEvent' && item.id.startsWith('event:diff:')) {
    const diff = snapshot.diffs[item.id.replace(/^event:diff:/, '')];
    return diff
      ? {
          kind: 'diff',
          diff_id: diff.diff_id,
          updated_at: diff.updated_at ?? null
        }
      : null;
  }
  return null;
}
