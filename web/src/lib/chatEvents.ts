import type { AppEvent, BrowserEventEnvelope } from '../api/types';

export type ChatItem =
  | {
      id: string;
      kind: 'user' | 'assistant';
      messageId: string;
      content: string;
      completed?: boolean;
    }
  | {
      id: string;
      kind: 'command';
      commandId: string;
      title: string;
      detail?: string;
      output: string;
      status: 'running' | 'completed';
      success?: boolean;
    }
  | {
      id: string;
      kind: 'tool';
      toolCallId: string;
      title: string;
      detail?: string;
      status: 'running' | 'completed';
    }
  | {
      id: string;
      kind: 'file';
      path: string;
      title: string;
      detail?: string;
      status: 'proposed' | 'applied' | 'rejected';
    }
  | {
      id: string;
      kind: 'approval';
      approvalId: string;
      title: string;
      detail?: string;
      resolvedDecision?: string;
    }
  | {
      id: string;
      kind: 'error';
      title: string;
      detail?: string;
    }
  | {
      id: string;
      kind: 'event';
      title: string;
      detail?: string;
    };

export interface ChatState {
  seenEventIds: Set<string>;
  items: ChatItem[];
}

export function createChatState(): ChatState {
  return {
    seenEventIds: new Set(),
    items: []
  };
}

export function applyChatEnvelope(state: ChatState, envelope: BrowserEventEnvelope): ChatState {
  if (envelope.event_id && state.seenEventIds.has(envelope.event_id)) {
    return state;
  }

  const seenEventIds = new Set(state.seenEventIds);
  if (envelope.event_id) {
    seenEventIds.add(envelope.event_id);
  }

  return {
    seenEventIds,
    items: applyAppEvent(state.items, envelope.event)
  };
}

function applyAppEvent(items: ChatItem[], event: AppEvent): ChatItem[] {
  const payload = event.payload;
  switch (event.type) {
    case 'user_message':
      return upsert(items, {
        id: `user:${stringField(payload, 'message_id') ?? fallbackId(event)}`,
        kind: 'user',
        messageId: stringField(payload, 'message_id') ?? fallbackId(event),
        content: stringField(payload, 'content') ?? ''
      });
    case 'agent_message_delta': {
      const messageId = stringField(payload, 'message_id') ?? fallbackId(event);
      const id = `agent:${messageId}`;
      const existing = items.find((item) => item.id === id);
      const next = {
        id,
        kind: 'assistant' as const,
        messageId,
        content:
          existing?.kind === 'assistant'
            ? `${existing.content}${stringField(payload, 'delta') ?? ''}`
            : stringField(payload, 'delta') ?? '',
        completed: existing?.kind === 'assistant' ? existing.completed : false
      };
      return upsert(items, next);
    }
    case 'agent_message_completed': {
      const messageId = stringField(payload, 'message_id') ?? fallbackId(event);
      const id = `agent:${messageId}`;
      const existing = items.find((item) => item.id === id);
      return upsert(items, {
        id,
        kind: 'assistant',
        messageId,
        content:
          stringField(payload, 'content') ??
          (existing?.kind === 'assistant' ? existing.content : ''),
        completed: true
      });
    }
    case 'command_started':
      return upsert(items, {
        id: `command:${stringField(payload, 'command_id') ?? fallbackId(event)}`,
        kind: 'command',
        commandId: stringField(payload, 'command_id') ?? fallbackId(event),
        title: stringField(payload, 'command') ?? 'Command',
        detail: stringField(payload, 'cwd'),
        output: '',
        status: 'running'
      });
    case 'command_output_delta':
      return updateCommandOutput(items, payload);
    case 'command_completed':
      return updateCommandCompleted(items, payload);
    case 'tool_started':
    case 'tool_updated':
    case 'tool_completed':
      return upsert(items, {
        id: `tool:${stringField(payload, 'tool_call_id') ?? fallbackId(event)}`,
        kind: 'tool',
        toolCallId: stringField(payload, 'tool_call_id') ?? fallbackId(event),
        title: stringField(payload, 'title') ?? stringField(payload, 'name') ?? 'Tool',
        detail: previewJson(payload.input ?? payload.output),
        status: event.type === 'tool_completed' ? 'completed' : 'running'
      });
    case 'file_change_proposed':
    case 'file_change_applied':
    case 'file_change_rejected':
      return upsert(items, {
        id: `file:${event.type}:${stringField(payload, 'path') ?? fallbackId(event)}`,
        kind: 'file',
        path: stringField(payload, 'path') ?? '',
        title: stringField(payload, 'path') ?? 'File change',
        detail: stringField(payload, 'diff'),
        status:
          event.type === 'file_change_applied'
            ? 'applied'
            : event.type === 'file_change_rejected'
              ? 'rejected'
              : 'proposed'
      });
    case 'approval_requested':
      return upsert(items, {
        id: `approval:${stringField(payload, 'approval_id') ?? fallbackId(event)}`,
        kind: 'approval',
        approvalId: stringField(payload, 'approval_id') ?? fallbackId(event),
        title: stringField(payload, 'title') ?? 'Approval requested',
        detail: stringField(payload, 'details')
      });
    case 'approval_resolved':
      return updateApprovalResolved(items, payload);
    case 'error':
      return [
        ...items,
        {
          id: `error:${items.length}`,
          kind: 'error',
          title: stringField(payload, 'message') ?? 'Agent error',
          detail: stringField(payload, 'code')
        }
      ];
    default:
      return [
        ...items,
        {
          id: `event:${event.type}:${items.length}`,
          kind: 'event',
          title: event.type,
          detail: JSON.stringify(payload, null, 2)
        }
      ];
  }
}

function updateCommandOutput(items: ChatItem[], payload: Record<string, unknown>): ChatItem[] {
  const commandId = stringField(payload, 'command_id') ?? fallbackId({ type: 'command_output_delta', payload });
  const id = `command:${commandId}`;
  const existing = items.find((item) => item.id === id);
  const output =
    existing?.kind === 'command'
      ? `${existing.output}${stringField(payload, 'delta') ?? ''}`
      : stringField(payload, 'delta') ?? '';
  return upsert(items, {
    id,
    kind: 'command',
    commandId,
    title: 'Command output',
    output,
    status: existing?.kind === 'command' ? existing.status : 'running'
  });
}

function updateCommandCompleted(items: ChatItem[], payload: Record<string, unknown>): ChatItem[] {
  const commandId = stringField(payload, 'command_id') ?? fallbackId({ type: 'command_completed', payload });
  const id = `command:${commandId}`;
  const existing = items.find((item) => item.id === id);
  return upsert(items, {
    id,
    kind: 'command',
    commandId,
    title: existing?.kind === 'command' ? existing.title : 'Command',
    detail: existing?.kind === 'command' ? existing.detail : undefined,
    output: existing?.kind === 'command' ? existing.output : '',
    status: 'completed',
    success: Boolean(payload.success)
  });
}

function updateApprovalResolved(items: ChatItem[], payload: Record<string, unknown>): ChatItem[] {
  const approvalId = stringField(payload, 'approval_id') ?? fallbackId({ type: 'approval_resolved', payload });
  const id = `approval:${approvalId}`;
  const existing = items.find((item) => item.id === id);
  const decision = payload.decision;
  return upsert(items, {
    id,
    kind: 'approval',
    approvalId,
    title: existing?.kind === 'approval' ? existing.title : 'Approval resolved',
    detail: existing?.kind === 'approval' ? existing.detail : undefined,
    resolvedDecision:
      typeof decision === 'object' && decision !== null && 'decision' in decision
        ? String(decision.decision)
        : undefined
  });
}

function upsert(items: ChatItem[], next: ChatItem): ChatItem[] {
  const index = items.findIndex((item) => item.id === next.id);
  if (index === -1) {
    return [...items, next];
  }

  return [...items.slice(0, index), next, ...items.slice(index + 1)];
}

function stringField(payload: Record<string, unknown>, field: string): string | undefined {
  const value = payload[field];
  return typeof value === 'string' ? value : undefined;
}

function previewJson(value: unknown): string | undefined {
  if (value === undefined || value === null) {
    return undefined;
  }
  return JSON.stringify(value, null, 2);
}

function fallbackId(event: Pick<AppEvent, 'type' | 'payload'>): string {
  return `${event.type}:${JSON.stringify(event.payload)}`;
}
