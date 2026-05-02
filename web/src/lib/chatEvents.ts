import type { AgentQuestionField, AppEvent, BrowserEventEnvelope } from '../api/types';

export type ChatItem =
  | {
      id: string;
      kind: 'user' | 'assistant';
      messageId: string;
      content: string;
      markdown: true;
      completed?: boolean;
    }
  | {
      id: string;
      kind: 'inlineEvent';
      eventKind: 'command';
      title: string;
      detail?: string;
      output: string;
      status: string;
      success?: boolean;
      exitCode?: number;
      durationMs?: number;
      processId?: string;
      source?: string;
      actions?: CommandActionView[];
    }
  | {
      id: string;
      kind: 'inlineEvent';
      eventKind: 'tool' | 'file' | 'event';
      title: string;
      detail?: string;
      output?: never;
      success?: never;
      status: string;
      exitCode?: number;
      durationMs?: number;
      processId?: string;
      source?: string;
      actions?: CommandActionView[];
    }
  | {
      id: string;
      kind: 'plan';
      title: string;
      content: string;
    }
  | {
      id: string;
      kind: 'subagent';
      operation: 'spawn' | 'wait' | 'close';
      title: string;
      status: string;
      agentIds: string[];
      model?: string;
      reasoningEffort?: string;
      prompt?: string;
      states: SubagentStateView[];
      providerPayload?: Record<string, unknown>;
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
      kind: 'question';
      questionId: string;
      title: string;
      description?: string;
      fields: AgentQuestionField[];
      answered: boolean;
    }
  | {
      id: string;
      kind: 'error';
      title: string;
      detail?: string;
    };

export interface CommandActionView {
  kind: string;
  label: string;
  detail?: string;
  path?: string;
}

export interface SubagentStateView {
  agentId: string;
  status: string;
  message?: string;
}

export interface ChatActivity {
  status: string;
  active: boolean;
  label: string;
}

export interface ChatState {
  seenEventIds: Set<string>;
  items: ChatItem[];
  activity?: ChatActivity;
}

export function createChatState(): ChatState {
  return {
    seenEventIds: new Set(),
    items: [],
    activity: undefined
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
    items: applyAppEvent(state.items, envelope.event, envelope.event_id),
    activity: applyActivity(state.activity, envelope.event)
  };
}

function applyAppEvent(items: ChatItem[], event: AppEvent, eventId?: string): ChatItem[] {
  const payload = event.payload;
  switch (event.type) {
    case 'user_message':
      return upsert(items, {
        id: `user:${stringField(payload, 'message_id') ?? fallbackId(event)}`,
        kind: 'user',
        messageId: stringField(payload, 'message_id') ?? fallbackId(event),
        content: stringField(payload, 'content') ?? '',
        markdown: true
      });
    case 'session_status_changed':
      return upsert(items, {
        id: `status:${eventId ?? stringField(payload, 'status') ?? fallbackId(event)}`,
        kind: 'inlineEvent',
        eventKind: 'event',
        title: statusLabel(stringField(payload, 'status')),
        detail: stringField(payload, 'reason'),
        status: stringField(payload, 'status') ?? 'updated'
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
        markdown: true as const,
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
        markdown: true,
        completed: true
      });
    }
    case 'plan_updated':
      return upsertPlan(items, {
        id: `plan:${stringField(payload, 'plan_id') ?? stringField(payload, 'message_id') ?? eventId ?? eventIdHint(event)}`,
        kind: 'plan',
        title: stringField(payload, 'title') ?? 'Implementation plan',
        content: stringField(payload, 'content') ?? stringField(payload, 'markdown') ?? previewJson(payload) ?? ''
      }, Boolean(payload.append));
    case 'command_started':
      return upsert(items, {
        id: `event:command:${stringField(payload, 'command_id') ?? fallbackId(event)}`,
        kind: 'inlineEvent',
        eventKind: 'command',
        title: stringField(payload, 'command') ?? 'Command',
        detail: commandDetail(payload),
        output: '',
        status: 'running',
        success: undefined,
        processId: stringField(payload, 'process_id'),
        source: stringField(payload, 'source'),
        actions: commandActions(payload)
      });
    case 'command_output_delta':
      return updateCommandOutput(items, payload);
    case 'command_completed':
      return updateCommandCompleted(items, payload);
    case 'tool_started':
    case 'tool_updated':
    case 'tool_completed': {
      const subagent = subagentItem(payload, event.type === 'tool_completed' ? 'completed' : 'running');
      if (subagent) {
        return upsert(items, subagent);
      }
      return upsert(items, {
        id: `event:tool:${stringField(payload, 'tool_call_id') ?? fallbackId(event)}`,
        kind: 'inlineEvent',
        eventKind: 'tool',
        title: toolTitle(payload),
        detail: toolDetail(payload),
        status: event.type === 'tool_completed' ? 'completed' : 'running'
      });
    }
    case 'file_change_proposed':
    case 'file_change_applied':
    case 'file_change_rejected':
      return upsert(items, {
        id: `event:file:${stringField(payload, 'path') ?? fallbackId(event)}`,
        kind: 'inlineEvent',
        eventKind: 'file',
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
    case 'question_requested':
      return upsert(items, {
        id: `question:${stringField(payload, 'question_id') ?? fallbackId(event)}`,
        kind: 'question',
        questionId: stringField(payload, 'question_id') ?? fallbackId(event),
        title: stringField(payload, 'title') ?? 'Input requested',
        description: stringField(payload, 'description'),
        fields: questionFields(payload),
        answered: false
      });
    case 'question_answered':
      return updateQuestionAnswered(items, payload);
    case 'provider_event':
      return upsert(items, {
        id: `event:provider:${stringField(payload, 'event_id') ?? eventId ?? fallbackId(event)}`,
        kind: 'inlineEvent',
        eventKind: 'event',
        title: stringField(payload, 'title') ?? providerEventTitle(payload),
        detail: stringField(payload, 'detail') ?? providerEventDetail(payload),
        status: stringField(payload, 'status') ?? 'received'
      });
    case 'error':
      return [
        ...items,
        {
          id: `error:${items.length}`,
          kind: 'error',
          title: stringField(payload, 'message') ?? 'Agent error',
          detail: errorDetail(payload)
        }
      ];
    default:
      return [
        ...items,
        {
          id: `event:generic:${event.type}:${items.length}`,
          kind: 'inlineEvent',
          eventKind: 'event',
          title: event.type,
          status: 'received',
          detail: JSON.stringify(payload, null, 2)
        }
      ];
  }
}

function errorDetail(payload: Record<string, unknown>): string | undefined {
  const parts = [
    stringField(payload, 'code') ? `code: ${stringField(payload, 'code')}` : undefined,
    providerPayloadDetail(payload)
  ].filter(Boolean);
  return parts.length > 0 ? parts.join('\n\n') : undefined;
}

function providerPayloadDetail(payload: Record<string, unknown>): string | undefined {
  const provider = payload.provider_payload;
  if (typeof provider === 'object' && provider !== null) {
    return previewJson(provider);
  }
  return undefined;
}

function questionFields(payload: Record<string, unknown>): AgentQuestionField[] {
  const fields = payload.fields;
  return Array.isArray(fields)
    ? fields.filter((field): field is AgentQuestionField => typeof field === 'object' && field !== null)
    : [];
}

function updateCommandOutput(items: ChatItem[], payload: Record<string, unknown>): ChatItem[] {
  const commandId = stringField(payload, 'command_id') ?? fallbackId({ type: 'command_output_delta', payload });
  const id = `event:command:${commandId}`;
  const existing = items.find((item) => item.id === id);
  const output =
    existing?.kind === 'inlineEvent' && existing.eventKind === 'command'
      ? `${existing.output}${stringField(payload, 'delta') ?? ''}`
      : stringField(payload, 'delta') ?? '';
  return upsert(items, {
    id,
    kind: 'inlineEvent',
    eventKind: 'command',
    title:
      existing?.kind === 'inlineEvent' && existing.eventKind === 'command'
        ? existing.title
        : 'Command output',
    detail:
      existing?.kind === 'inlineEvent' && existing.eventKind === 'command'
        ? existing.detail
        : commandDetail(payload),
    output,
    status: existing?.kind === 'inlineEvent' ? existing.status : 'running',
    success:
      existing?.kind === 'inlineEvent' && existing.eventKind === 'command'
        ? existing.success
        : undefined,
    exitCode:
      existing?.kind === 'inlineEvent' && existing.eventKind === 'command'
        ? existing.exitCode
        : numberField(payload, 'exit_code'),
    durationMs:
      existing?.kind === 'inlineEvent' && existing.eventKind === 'command'
        ? existing.durationMs
        : numberField(payload, 'duration_ms'),
    processId:
      existing?.kind === 'inlineEvent' && existing.eventKind === 'command'
        ? existing.processId
        : stringField(payload, 'process_id'),
    source:
      existing?.kind === 'inlineEvent' && existing.eventKind === 'command'
        ? existing.source
        : stringField(payload, 'source'),
    actions:
      existing?.kind === 'inlineEvent' && existing.eventKind === 'command'
        ? existing.actions
        : commandActions(payload)
  });
}

function updateCommandCompleted(items: ChatItem[], payload: Record<string, unknown>): ChatItem[] {
  const commandId = stringField(payload, 'command_id') ?? fallbackId({ type: 'command_completed', payload });
  const id = `event:command:${commandId}`;
  const existing = items.find((item) => item.id === id);
  return upsert(items, {
    id,
    kind: 'inlineEvent',
    eventKind: 'command',
    title: existing?.kind === 'inlineEvent' ? existing.title : 'Command',
    detail: existing?.kind === 'inlineEvent' ? existing.detail : undefined,
    output: existing?.kind === 'inlineEvent' && existing.eventKind === 'command' ? existing.output : '',
    status: 'completed',
    success: Boolean(payload.success),
    exitCode: numberField(payload, 'exit_code'),
    durationMs: numberField(payload, 'duration_ms'),
    processId:
      existing?.kind === 'inlineEvent' && existing.eventKind === 'command'
        ? existing.processId
        : stringField(payload, 'process_id'),
    source:
      existing?.kind === 'inlineEvent' && existing.eventKind === 'command'
        ? existing.source
        : stringField(payload, 'source'),
    actions: existing?.kind === 'inlineEvent' && existing.eventKind === 'command' ? existing.actions : []
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

function updateQuestionAnswered(items: ChatItem[], payload: Record<string, unknown>): ChatItem[] {
  const questionId = stringField(payload, 'question_id') ?? fallbackId({ type: 'question_answered', payload });
  const id = `question:${questionId}`;
  const existing = items.find((item) => item.id === id);
  return upsert(items, {
    id,
    kind: 'question',
    questionId,
    title: existing?.kind === 'question' ? existing.title : 'Input answered',
    description: existing?.kind === 'question' ? existing.description : undefined,
    fields: existing?.kind === 'question' ? existing.fields : [],
    answered: true
  });
}

function upsert(items: ChatItem[], next: ChatItem): ChatItem[] {
  const index = items.findIndex((item) => item.id === next.id);
  if (index === -1) {
    return [...items, next];
  }

  return [...items.slice(0, index), next, ...items.slice(index + 1)];
}

function upsertPlan(items: ChatItem[], next: Extract<ChatItem, { kind: 'plan' }>, append: boolean): ChatItem[] {
  if (!append) {
    return upsert(items, next);
  }
  const existing = items.find((item) => item.id === next.id);
  if (existing?.kind !== 'plan') {
    return upsert(items, next);
  }
  return upsert(items, {
    ...next,
    title: next.title || existing.title,
    content: `${existing.content}${next.content}`
  });
}

function applyActivity(current: ChatActivity | undefined, event: AppEvent): ChatActivity | undefined {
  if (event.type !== 'session_status_changed') {
    return current;
  }
  const status = stringField(event.payload, 'status');
  if (!status) {
    return current;
  }
  return {
    status,
    active: status === 'running',
    label: statusLabel(status)
  };
}

function statusLabel(status: string | undefined): string {
  switch (status) {
    case 'running':
      return 'Working';
    case 'waiting_for_approval':
      return 'Waiting for approval';
    case 'waiting_for_input':
      return 'Waiting for input';
    case 'completed':
      return 'Turn complete';
    case 'failed':
      return 'Failed';
    case 'interrupted':
      return 'Interrupted';
    default:
      return status ? humanizeStatus(status) : 'Status changed';
  }
}

function humanizeStatus(status: string): string {
  return status
    .split('_')
    .filter(Boolean)
    .map((part) => `${part.charAt(0).toUpperCase()}${part.slice(1)}`)
    .join(' ');
}

function stringField(payload: Record<string, unknown>, field: string): string | undefined {
  const value = payload[field];
  return typeof value === 'string' ? value : undefined;
}

function numberField(payload: Record<string, unknown>, field: string): number | undefined {
  const value = payload[field];
  return typeof value === 'number' ? value : undefined;
}

function commandDetail(payload: Record<string, unknown>): string | undefined {
  const parts = [
    stringField(payload, 'cwd'),
    stringField(payload, 'source'),
    stringField(payload, 'process_id') ? `pid ${stringField(payload, 'process_id')}` : undefined
  ].filter(Boolean);
  return parts.length > 0 ? parts.join(' · ') : undefined;
}

function commandActions(payload: Record<string, unknown>): CommandActionView[] {
  const actions = payload.actions;
  if (!Array.isArray(actions)) {
    return [];
  }
  return actions
    .filter((action): action is Record<string, unknown> => typeof action === 'object' && action !== null)
    .map((action) => {
      const kind = stringField(action, 'kind') ?? 'unknown';
      const path = stringField(action, 'path');
      const query = stringField(action, 'query');
      const command = stringField(action, 'command');
      const name = stringField(action, 'name');
      const skillName = skillNameFromPath(path);
      if (skillName) {
        return {
          kind: 'skill',
          label: `Skill: ${skillName}`,
          detail: path,
          path
        };
      }
      if (kind === 'read') {
        return {
          kind,
          label: `Read ${name ?? basename(path) ?? 'file'}`,
          detail: path ?? command,
          path
        };
      }
      if (kind === 'search') {
        return {
          kind,
          label: `Search ${query ?? basename(path) ?? 'workspace'}`,
          detail: path ?? command
        };
      }
      if (kind === 'listFiles') {
        return {
          kind,
          label: `List ${path ?? 'files'}`,
          detail: command,
          path
        };
      }
      return {
        kind,
        label: command ?? kind,
        detail: path ?? query
      };
    });
}

function toolTitle(payload: Record<string, unknown>): string {
  const provider = providerPayload(payload);
  const tool = stringField(provider, 'tool') ?? stringField(payload, 'name') ?? stringField(payload, 'title');
  if (tool === 'spawnAgent') {
    return 'Spawn subagent';
  }
  if (tool === 'wait') {
    return 'Wait for subagents';
  }
  if (tool === 'closeAgent') {
    return 'Close subagent';
  }
  return stringField(payload, 'title') ?? stringField(payload, 'name') ?? tool ?? 'Tool';
}

function toolDetail(payload: Record<string, unknown>): string | undefined {
  const provider = providerPayload(payload);
  const tool = stringField(provider, 'tool') ?? stringField(payload, 'name');
  if (tool === 'spawnAgent') {
    return [
      receiverThreadIds(provider),
      stringField(provider, 'model'),
      stringField(provider, 'reasoningEffort'),
      stringField(provider, 'prompt')
    ]
      .filter(Boolean)
      .join('\n\n');
  }
  if (tool === 'wait' || tool === 'closeAgent') {
    return [receiverThreadIds(provider), previewJson(provider.agentsStates)].filter(Boolean).join('\n\n');
  }
  return previewJson(payload.input ?? payload.output);
}

function providerPayload(payload: Record<string, unknown>): Record<string, unknown> {
  const provider = payload.provider_payload;
  if (typeof provider === 'object' && provider !== null) {
    return provider as Record<string, unknown>;
  }
  const input = payload.input;
  if (typeof input === 'object' && input !== null) {
    return input as Record<string, unknown>;
  }
  return payload;
}

function providerEventTitle(payload: Record<string, unknown>): string {
  const category = stringField(payload, 'category');
  if (category === 'compaction') {
    return 'Context compacted';
  }
  if (category === 'warning') {
    return 'Provider warning';
  }
  if (category === 'reasoning') {
    return 'Reasoning update';
  }
  if (category === 'model') {
    return 'Model update';
  }
  return 'Provider event';
}

function providerEventDetail(payload: Record<string, unknown>): string | undefined {
  const provider = payload.provider_payload;
  if (typeof provider === 'object' && provider !== null) {
    return previewJson(provider);
  }
  return stringField(payload, 'category');
}

function receiverThreadIds(payload: Record<string, unknown>): string | undefined {
  const value = payload.receiverThreadIds;
  return Array.isArray(value) && value.length > 0 ? `Agents: ${value.join(', ')}` : undefined;
}

function subagentItem(payload: Record<string, unknown>, status: string): ChatItem | undefined {
  const provider = providerPayload(payload);
  if (provider.type !== 'collabAgentToolCall') {
    return undefined;
  }
  const tool = stringField(provider, 'tool') ?? stringField(payload, 'name');
  const operation =
    tool === 'spawnAgent'
      ? 'spawn'
      : tool === 'wait'
        ? 'wait'
        : tool === 'closeAgent'
          ? 'close'
          : undefined;
  if (!operation) {
    return undefined;
  }
  const agentIds = arrayOfStrings(provider.receiverThreadIds);
  const states = subagentStates(provider);
  return {
    id: `subagent:${stringField(payload, 'tool_call_id') ?? stringField(provider, 'id') ?? fallbackId({ type: 'tool_completed', payload })}`,
    kind: 'subagent',
    operation,
    title:
      operation === 'spawn'
        ? 'Spawn subagent'
        : operation === 'wait'
          ? 'Wait for subagent'
          : 'Close subagent',
    status,
    agentIds,
    model: stringField(provider, 'model'),
    reasoningEffort: stringField(provider, 'reasoningEffort'),
    prompt: stringField(provider, 'prompt'),
    states,
    providerPayload: provider
  };
}

function subagentStates(provider: Record<string, unknown>): SubagentStateView[] {
  const states = provider.agentsStates;
  if (typeof states !== 'object' || states === null || Array.isArray(states)) {
    return [];
  }
  return Object.entries(states as Record<string, unknown>)
    .filter((entry): entry is [string, Record<string, unknown>] => typeof entry[1] === 'object' && entry[1] !== null)
    .map(([agentId, state]) => ({
      agentId,
      status: stringField(state, 'status') ?? 'unknown',
      message: stringField(state, 'message')
    }));
}

function arrayOfStrings(value: unknown): string[] {
  return Array.isArray(value) ? value.filter((item): item is string => typeof item === 'string') : [];
}

function skillNameFromPath(path: string | undefined): string | undefined {
  if (!path) {
    return undefined;
  }
  const match = path.match(/\/skills\/([^/]+)\/SKILL\.md$/);
  return match?.[1];
}

function basename(path: string | undefined): string | undefined {
  return path?.split('/').filter(Boolean).at(-1);
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

function eventIdHint(event: AppEvent): string {
  return fallbackId(event);
}
