import type {
  AgentQuestionField,
  ApprovalOption,
  ApprovalDecisionName
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
    }
  | {
      id: string;
      kind: 'plan';
      title: string;
      content: string;
      status?: string;
      entries?: PlanEntryView[];
      source?: string;
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
      /** Serialized shapes from runners (Codex-correlated today). See runner `presentation` on `approval_requested`. */
      presentation?: Record<string, unknown>;
      options?: ApprovalUiChoice[];
      status?: string;
      risk?: string;
      subject?: string;
      resolutionState?: 'pending' | 'resolving';
      resolvingDecision?: string;
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
   * transitioned to a non-`running` state). Mirrors Codex TUI's
   * `latest_proposed_plan_markdown` gate: the "Implement plan" handoff is
   * only offered once the model has stopped streaming the plan.
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

const DEFAULT_APPROVAL_ACTIONS: ApprovalDecisionName[] = [
  'accept',
  'accept_for_session',
  'decline',
  'cancel'
];

function mapCodexDecisionLabel(raw: string): ApprovalDecisionName | undefined {
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

function dedupeStable<T extends string>(items: T[]): T[] {
  const seen = new Set<T>();
  const out: T[] = [];
  for (const x of items) {
    if (seen.has(x)) {
      continue;
    }
    seen.add(x);
    out.push(x);
  }
  return out;
}

/** Maps Codex `available_decisions` (when present) to API `ApprovalDecision` names; otherwise all four Codex-supported outcomes. */
export function approvalUiChoices(item: Extract<ChatItem, { kind: 'approval' }>): ApprovalUiChoice[] {
  if (item.options && item.options.length > 0) {
    return item.options;
  }
  const p = item.presentation;
  const variant = p && typeof p.variant === 'string' ? p.variant : '';
  if (variant === 'codex_command' && p && Array.isArray(p.available_decisions)) {
    const mapped = p.available_decisions
      .filter((entry): entry is string => typeof entry === 'string')
      .map(mapCodexDecisionLabel)
      .filter((x): x is ApprovalDecisionName => Boolean(x));
    if (mapped.length > 0) {
      return dedupeStable(mapped).map(choiceFromDefaultDecision);
    }
  }
  return DEFAULT_APPROVAL_ACTIONS.map(choiceFromDefaultDecision);
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
    label: option.label || approvalUiButtonLabel(decision),
    description: option.description ?? undefined,
    scope: option.scope ?? undefined
  };
}

function choiceFromDefaultDecision(decision: ApprovalDecisionName): ApprovalUiChoice {
  return {
    optionId: defaultOptionId(decision),
    decision,
    label: approvalUiButtonLabel(decision)
  };
}

function defaultOptionId(decision: ApprovalDecisionName): string {
  switch (decision) {
    case 'accept':
      return 'approve_once';
    case 'accept_for_session':
      return 'approve_always';
    case 'decline':
      return 'deny';
    case 'cancel':
      return 'cancel_turn';
    default:
      return decision;
  }
}

function approvalDecisionFromOption(option: ApprovalOption): ApprovalDecisionName | undefined {
  switch (option.option_id) {
    case 'approve_once':
      return 'accept';
    case 'approve_always':
      return 'accept_for_session';
    case 'deny':
    case 'deny_with_feedback':
      return 'decline';
    case 'cancel_turn':
      return 'cancel';
    default:
      return mapCodexDecisionLabel(option.native_option_id ?? option.kind);
  }
}

export function fileChangeApprovalFiles(presentation: Record<string, unknown> | undefined): {
  path: string;
  changeKind?: string;
  diff?: string;
}[] {
  if (!presentation || presentation.variant !== 'codex_file_change') {
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
