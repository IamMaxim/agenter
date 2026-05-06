import type {
  AgentQuestionField,
  ApprovalOption,
  ApprovalDecisionName,
  PlanHandoffState
} from '../api/types';

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
      displayLevel?: InlineEventDisplayLevel;
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
      subkind?: string;
      rawPayload?: unknown;
    }
  | {
      id: string;
      kind: 'inlineEvent';
      eventKind: 'tool' | 'file' | 'event';
      displayLevel?: InlineEventDisplayLevel;
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
      subkind?: string;
      rawPayload?: unknown;
    }
  | {
      id: string;
      kind: 'plan';
      title: string;
      content: string;
      status?: string;
      entries?: PlanEntryView[];
      source?: string;
      handoff?: PlanHandoffState | null;
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
      providerPayload?: unknown;
    }
  | {
      id: string;
      kind: 'approval';
      approvalId: string;
      title: string;
      detail?: string;
      approvalKind?: string;
      presentation?: Record<string, unknown>;
      options?: ApprovalUiChoice[];
      status?: string;
      risk?: string;
      subject?: string;
      nativeRequestId?: string;
      nativeBlocking?: boolean;
      resolutionState?: 'pending' | 'resolving';
      resolvingDecision?: string;
      resolvedDecision?: string;
      rawPayload?: unknown;
    }
  | {
      id: string;
      kind: 'question';
      questionId: string;
      title: string;
      description?: string;
      fields: AgentQuestionField[];
      answered: boolean;
      status?: string;
      resolvedState?: string;
      nativeRequestId?: string;
      nativeBlocking?: boolean;
      rawPayload?: unknown;
    }
  | {
      id: string;
      kind: 'error';
      title: string;
      detail?: string;
      rawPayload?: unknown;
    };

export interface CommandActionView {
  kind: string;
  label: string;
  detail?: string;
  path?: string;
}

export interface PlanEntryView {
  id: string;
  label: string;
  status: string;
}

export interface ApprovalUiChoice {
  optionId: string;
  decision: ApprovalDecisionName;
  label: string;
  description?: string;
  scope?: string;
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

export type InlineEventDisplayLevel = 'normal' | 'thinking' | 'raw';

export interface ChatState {
  seenEventIds: Set<string>;
  items: ChatItem[];
  activity?: ChatActivity;
  /**
   * Item id of the most recently emitted/updated `kind: 'plan'` row, or
   * `undefined` if no plan has been seen in this session yet. Set every time
   * a `plan_updated` event arrives and reset only when a fresh plan with a
   * different id replaces it.
   */
  latestPlanId?: string;
  /**
   * True when the turn that produced `latestPlanId` has finished (status
   * transitioned to a non-`running` state). The "Implement plan" handoff is
   * only offered once the provider has stopped streaming the plan.
   */
  planTurnComplete: boolean;
}

export function createChatState(): ChatState {
  return {
    seenEventIds: new Set(),
    items: [],
    activity: undefined,
    latestPlanId: undefined,
    planTurnComplete: false
  };
}

function mapNativeDecisionLabel(raw: string): ApprovalDecisionName | undefined {
  const norm = raw.replace(/_/g, '').toLowerCase();
  switch (norm) {
    case 'accept':
      return 'accept';
    case 'acceptforsession':
      return 'accept_for_session';
    case 'decline':
      return 'decline';
    case 'cancel':
      return 'cancel';
    default:
      return undefined;
  }
}

export function approvalUiChoices(item: Extract<ChatItem, { kind: 'approval' }>): ApprovalUiChoice[] {
  return item.options ?? [];
}

export function approvalUiButtonLabel(decision: ApprovalDecisionName): string {
  switch (decision) {
    case 'accept':
      return 'Accept';
    case 'accept_for_session':
      return 'Accept for session';
    case 'decline':
      return 'Decline';
    case 'cancel':
      return 'Cancel';
    default:
      return decision;
  }
}

export function approvalChoiceFromOption(option: ApprovalOption): ApprovalUiChoice | undefined {
  const decision = approvalDecisionFromOption(option);
  if (!decision) {
    return undefined;
  }
  return {
    optionId: option.option_id,
    decision,
    label: approvalOptionLabel(option, decision),
    description: option.description ?? undefined,
    scope: option.scope ?? undefined
  };
}

function approvalOptionLabel(option: ApprovalOption, decision: ApprovalDecisionName): string {
  const native = option.native_option_id?.replaceAll('_', ' ');
  if (native && native.toLowerCase() !== decision.replaceAll('_', ' ')) {
    return native;
  }
  return option.label || approvalUiButtonLabel(decision);
}

function approvalDecisionFromOption(option: ApprovalOption): ApprovalDecisionName | undefined {
  if (option.option_id.startsWith('persist_rule:')) {
    return 'accept_for_session';
  }
  switch (option.option_id) {
    case 'approve_once':
      return 'accept';
    case 'approve_always':
    case 'persist_rule':
    case 'persist_approval_rule':
      return 'accept_for_session';
    case 'deny':
    case 'deny_with_feedback':
      return 'decline';
    case 'cancel_turn':
      return 'cancel';
    default:
      return mapNativeDecisionLabel(option.native_option_id ?? option.kind);
  }
}

function commandPresentationVariant(variant: string): boolean {
  return variant === 'command' || variant === 'shell_command';
}

export function commandApprovalPresentation(
  presentation: Record<string, unknown> | undefined
): string | undefined {
  if (!presentation) {
    return undefined;
  }
  const variant = typeof presentation.variant === 'string' ? presentation.variant : '';
  if (!commandPresentationVariant(variant)) {
    return undefined;
  }
  return typeof presentation.command === 'string' ? presentation.command : undefined;
}

export function fileChangeApprovalFiles(presentation: Record<string, unknown> | undefined): {
  path: string;
  changeKind?: string;
  diff?: string;
}[] {
  if (!presentation || presentation.variant !== 'file_change') {
    return [];
  }
  const raw = presentation.files;
  if (!Array.isArray(raw)) {
    return [];
  }
  return raw
    .filter((entry): entry is Record<string, unknown> => typeof entry === 'object' && entry !== null)
    .map((f) => {
      const unified = f.unified_diff;
      let diffText: string | undefined;
      if (typeof unified === 'string') {
        diffText = unified;
      }
      return {
        path: typeof f.path === 'string' ? f.path : '(unknown)',
        changeKind: typeof f.change_kind === 'string' ? f.change_kind : undefined,
        diff: diffText
      };
    });
}
