import { UAP_PROTOCOL_VERSION, type UapProtocolVersion } from '../api/types';
import type {
  AgentCollaborationMode,
  AgentModelOption,
  AgentOptions,
  ApprovalDecision,
  ApprovalKind,
  ApprovalOption,
  ApprovalOptionKind,
  ApprovalRequest,
  ArtifactKind,
  ArtifactState,
  AgentReasoningEffort,
  AgentQuestionAnswer,
  AgentQuestionField,
  AgentTurnSettings,
  BrowserSessionSnapshot,
  CapabilitySet,
  ContentBlock,
  ContentBlockKind,
  DiffFile,
  DiffState,
  FileChangeKind,
  ItemRole,
  ItemState,
  ItemStatus,
  NativeRef,
  PlanEntryStatus,
  PlanSource,
  PlanState,
  PlanStatus,
  ProviderNotification,
  QuestionState,
  QuestionStatus,
  BrowserServerMessage,
  RunnerInfo,
  RunnerStatus,
  SessionInfo,
  SessionSnapshot,
  SessionStatus,
  SessionUsageContext,
  SessionUsageSnapshot,
  SessionUsageWindow,
  TurnState,
  TurnStatus,
  UniversalEventEnvelope,
  UniversalEventKind,
  UniversalEventSource,
  UniversalPlanEntry,
  WorkspaceRef
} from '../api/types';

export const defaultReasoningEfforts: AgentReasoningEffort[] = [
  'minimal',
  'low',
  'medium',
  'high',
  'xhigh'
];

const reasoningEfforts = new Set<AgentReasoningEffort>([
  'none',
  'minimal',
  'low',
  'medium',
  'high',
  'xhigh'
]);

const sessionStatuses = new Set<SessionStatus>([
  'starting',
  'running',
  'waiting_for_input',
  'waiting_for_approval',
  'idle',
  'stopped',
  'completed',
  'interrupted',
  'degraded',
  'failed',
  'archived'
]);

export function normalizeAgentOptions(value: unknown): AgentOptions {
  const record = objectRecord(value);
  return {
    models: arrayValue(record.models).map(normalizeAgentModelOption).filter(isPresent),
    collaboration_modes: arrayValue(record.collaboration_modes)
      .map(normalizeCollaborationMode)
      .filter(isPresent)
  };
}

export function normalizeTurnSettings(value: unknown): AgentTurnSettings {
  const record = objectRecord(value);
  const settings: AgentTurnSettings = {};
  if (typeof record.model === 'string') {
    settings.model = record.model;
  }
  if (isReasoningEffort(record.reasoning_effort)) {
    settings.reasoning_effort = record.reasoning_effort;
  }
  if (typeof record.collaboration_mode === 'string') {
    settings.collaboration_mode = record.collaboration_mode;
  }
  return settings;
}

export function effortsForSelectedModel(
  options: AgentOptions,
  settings: AgentTurnSettings
): AgentReasoningEffort[] {
  const selected = options.models.find((model) => model.id === settings.model);
  return selected?.supported_reasoning_efforts.length
    ? selected.supported_reasoning_efforts
    : defaultReasoningEfforts;
}

export function normalizeRunners(value: unknown): RunnerInfo[] {
  return arrayValue(value).map(normalizeRunner).filter(isPresent);
}

export function normalizeWorkspaces(value: unknown): WorkspaceRef[] {
  return arrayValue(value).map(normalizeWorkspace).filter(isPresent);
}

export function normalizeSessions(value: unknown): SessionInfo[] {
  return arrayValue(value).map(normalizeSession).filter(isPresent);
}

export function normalizeSessionInfo(value: unknown): SessionInfo | undefined {
  return normalizeSession(value);
}

export function normalizeBrowserServerMessage(value: unknown): BrowserServerMessage {
  const record = objectRecord(value);
  if (record.type === 'session_snapshot') {
    return normalizeBrowserSessionSnapshot(record);
  }
  if (record.type === 'universal_event') {
    return {
      type: 'universal_event',
      ...normalizeUniversalEventEnvelope(record)
    };
  }
  if (record.type === 'ack') {
    return {
      type: 'ack',
      ...(typeof record.request_id === 'string' ? { request_id: record.request_id } : {})
    };
  }
  if (record.type === 'error') {
    return {
      type: 'error',
      ...(typeof record.request_id === 'string' ? { request_id: record.request_id } : {}),
      code: typeof record.code === 'string' ? record.code : 'unknown',
      message: typeof record.message === 'string' ? record.message : 'Unknown browser event error.'
    };
  }
  throw new Error(`Unsupported browser server message type: ${String(record.type ?? 'unknown')}`);
}

export function normalizeBrowserSessionSnapshot(value: unknown): BrowserSessionSnapshot {
  const record = objectRecord(value);
  return {
    type: 'session_snapshot',
    protocol_version: normalizeUapProtocolVersion(record.protocol_version),
    ...(typeof record.request_id === 'string' ? { request_id: record.request_id } : {}),
    snapshot: normalizeSessionSnapshot(record.snapshot),
    events: arrayValue(record.events).map(normalizeUniversalEventEnvelope),
    latest_seq: normalizeSeq(record.latest_seq),
    has_more: record.has_more === true
  };
}

export function normalizeSessionSnapshot(value: unknown): SessionSnapshot {
  const record = objectRecord(value);
  const sessionId = typeof record.session_id === 'string' ? record.session_id : '';
  return {
    session_id: sessionId,
    latest_seq: normalizeSeq(record.latest_seq),
    info: normalizeSessionInfo(record.info) ?? null,
    capabilities: normalizeCapabilitySet(record.capabilities),
    turns: normalizeRecord(record.turns, normalizeTurnState),
    items: normalizeRecord(record.items, normalizeItemState),
    approvals: normalizeRecord(record.approvals, normalizeApprovalRequest),
    questions: normalizeRecord(record.questions, normalizeQuestionState),
    plans: normalizeRecord(record.plans, normalizePlanState),
    diffs: normalizeRecord(record.diffs, normalizeDiffState),
    artifacts: normalizeRecord(record.artifacts, normalizeArtifactState),
    active_turns: arrayValue(record.active_turns).filter(isString)
  };
}

export function normalizeUniversalEventEnvelope(value: unknown): UniversalEventEnvelope {
  const record = objectRecord(value);
  const seq = normalizeSeq(record.seq);
  if (!seq) {
    throw new Error('Universal event is missing a valid universal seq.');
  }
  if (typeof record.event_id !== 'string' || record.event_id.length === 0) {
    throw new Error('Universal event is missing event_id.');
  }
  if (!isUuid(record.event_id)) {
    throw new Error('Universal event event_id must be UUID-shaped.');
  }
  return {
    protocol_version: normalizeUapProtocolVersion(record.protocol_version),
    event_id: record.event_id,
    seq,
    session_id: typeof record.session_id === 'string' ? record.session_id : '',
    turn_id: typeof record.turn_id === 'string' ? record.turn_id : null,
    item_id: typeof record.item_id === 'string' ? record.item_id : null,
    ts: typeof record.ts === 'string' ? record.ts : '',
    source: typeof record.source === 'string' ? (record.source as UniversalEventSource) : 'control_plane',
    native: normalizeNativeRef(record.native),
    event: normalizeUniversalEventKind(record.event)
  };
}

function normalizeUapProtocolVersion(value: unknown): UapProtocolVersion {
  if (value === undefined || value === null) {
    return UAP_PROTOCOL_VERSION;
  }
  if (value === UAP_PROTOCOL_VERSION) {
    return UAP_PROTOCOL_VERSION;
  }
  throw new Error(`Unsupported universal protocol version: ${String(value)}`);
}

function normalizeUniversalEventKind(value: unknown): UniversalEventKind {
  const record = objectRecord(value);
  const type = typeof record.type === 'string' ? record.type : 'native.unknown';
  const data = objectRecord(record.data);
  switch (type) {
    case 'session.created':
      return { type, data: { session: normalizeSessionInfo(data.session) ?? minimalSessionInfo() } };
    case 'session.status_changed':
      return {
        type,
        data: {
          status: stringOr(data.status, 'degraded') as SessionStatus,
          reason: typeof data.reason === 'string' ? data.reason : null
        }
      };
    case 'session.metadata_changed':
      return {
        type,
        data: {
          title: typeof data.title === 'string' ? data.title : null
        }
      };
    case 'turn.started':
    case 'turn.status_changed':
    case 'turn.completed':
    case 'turn.failed':
    case 'turn.cancelled':
    case 'turn.interrupted':
    case 'turn.detached':
      return { type, data: { turn: normalizeTurnState(data.turn) } };
    case 'item.created':
      return { type, data: { item: normalizeItemState(data.item) } };
    case 'content.delta':
      return {
        type,
        data: {
          block_id: stringOr(data.block_id, ''),
          kind: normalizeContentBlockKind(data.kind),
          delta: stringOr(data.delta, '')
        }
      };
    case 'content.completed':
      return {
        type,
        data: {
          block_id: stringOr(data.block_id, ''),
          kind: normalizeContentBlockKind(data.kind),
          text: typeof data.text === 'string' ? data.text : null
        }
      };
    case 'approval.requested':
      return { type, data: { approval: normalizeApprovalRequest(data.approval) } };
    case 'approval.resolved':
      return {
        type,
        data: {
          approval_id: stringOr(data.approval_id, ''),
          status: stringOr(data.status, 'approved'),
          resolved_at: stringOr(data.resolved_at, ''),
          resolved_by_user_id: typeof data.resolved_by_user_id === 'string' ? data.resolved_by_user_id : null,
          native: normalizeNativeRef(data.native)
        }
      };
    case 'question.requested':
    case 'question.answered':
      return { type, data: { question: normalizeQuestionState(data.question) } };
    case 'plan.updated':
      return { type, data: { plan: normalizePlanState(data.plan) } };
    case 'diff.updated':
      return { type, data: { diff: normalizeDiffState(data.diff) } };
    case 'artifact.created':
      return { type, data: { artifact: normalizeArtifactState(data.artifact) } };
    case 'usage.updated':
      return { type, data: { usage: normalizeSessionUsage(data.usage) ?? {} } };
    case 'error.reported':
      return {
        type,
        data: {
          code: typeof data.code === 'string' ? data.code : null,
          message: stringOr(data.message, 'Provider error')
        }
      };
    case 'provider.notification':
      return { type, data: { notification: normalizeProviderNotification(data.notification) } };
    case 'native.unknown':
      return { type, data: { summary: typeof data.summary === 'string' ? data.summary : null } };
    default:
      return { type: 'native.unknown', data: { summary: type } };
  }
}

function normalizeProviderNotification(value: unknown): ProviderNotification {
  const record = objectRecord(value);
  return {
    category: stringOr(record.category, 'provider'),
    title: stringOr(record.title, 'Provider event'),
    detail: typeof record.detail === 'string' ? record.detail : null,
    status: typeof record.status === 'string' ? record.status : null,
    severity: typeof record.severity === 'string' ? record.severity : null,
    subject: typeof record.subject === 'string' ? record.subject : null
  };
}

function normalizeCapabilitySet(value: unknown): CapabilitySet | undefined {
  const record = objectRecord(value);
  if (Object.keys(record).length === 0) {
    return undefined;
  }
  return {
    protocol: boolGroup(record.protocol),
    content: boolGroup(record.content),
    tools: boolGroup(record.tools),
    approvals: boolGroup(record.approvals),
    plan: boolGroup(record.plan),
    modes: boolGroup(record.modes),
    integration: boolGroup(record.integration),
    provider_details: arrayValue(record.provider_details).map(normalizeProviderCapabilityDetail)
  };
}

function normalizeProviderCapabilityDetail(
  value: unknown
): NonNullable<CapabilitySet['provider_details']>[number] {
  const record = objectRecord(value);
  return {
    key: stringOr(record.key, ''),
    status: stringOr(record.status, 'unsupported'),
    methods: arrayValue(record.methods).filter(isString),
    reason: typeof record.reason === 'string' ? record.reason : null
  };
}

function normalizeTurnState(value: unknown): TurnState {
  const record = objectRecord(value);
  return {
    turn_id: stringOr(record.turn_id, ''),
    session_id: stringOr(record.session_id, ''),
    status: stringOr(record.status, 'running') as TurnStatus,
    started_at: typeof record.started_at === 'string' ? record.started_at : null,
    completed_at: typeof record.completed_at === 'string' ? record.completed_at : null,
    model: typeof record.model === 'string' ? record.model : null,
    mode: typeof record.mode === 'string' ? record.mode : null
  };
}

function normalizeItemState(value: unknown): ItemState {
  const record = objectRecord(value);
  return {
    item_id: stringOr(record.item_id, ''),
    session_id: stringOr(record.session_id, ''),
    turn_id: typeof record.turn_id === 'string' ? record.turn_id : null,
    role: stringOr(record.role, 'system') as ItemRole,
    status: stringOr(record.status, 'created') as ItemStatus,
    content: arrayValue(record.content).map(normalizeContentBlock),
    tool: normalizeToolProjection(record.tool),
    native: normalizeNativeRef(record.native)
  };
}

function normalizeToolProjection(value: unknown): ItemState['tool'] {
  const record = objectRecord(value);
  if (Object.keys(record).length === 0) {
    return null;
  }
  return {
    kind: stringOr(record.kind, 'tool'),
    name: stringOr(record.name, ''),
    title: stringOr(record.title, stringOr(record.name, 'Tool')),
    status: stringOr(record.status, 'created') as ItemState['status'],
    detail: typeof record.detail === 'string' ? record.detail : null,
    input_summary: typeof record.input_summary === 'string' ? record.input_summary : null,
    output_summary: typeof record.output_summary === 'string' ? record.output_summary : null,
    command: objectOrNull(record.command) as NonNullable<ItemState['tool']>['command'],
    subagent: objectOrNull(record.subagent) as NonNullable<ItemState['tool']>['subagent'],
    mcp: objectOrNull(record.mcp) as NonNullable<ItemState['tool']>['mcp']
  };
}

function normalizeContentBlock(value: unknown): ContentBlock {
  const record = objectRecord(value);
  return {
    block_id: stringOr(record.block_id, ''),
    kind: normalizeContentBlockKind(record.kind),
    text: typeof record.text === 'string' ? record.text : null,
    mime_type: typeof record.mime_type === 'string' ? record.mime_type : null,
    artifact_id: typeof record.artifact_id === 'string' ? record.artifact_id : null
  };
}

function normalizeContentBlockKind(value: unknown): ContentBlockKind {
  return stringOr(value, 'text') as ContentBlockKind;
}

function normalizeApprovalRequest(value: unknown): ApprovalRequest {
  const record = objectRecord(value);
  return {
    approval_id: stringOr(record.approval_id, ''),
    session_id: stringOr(record.session_id, ''),
    turn_id: typeof record.turn_id === 'string' ? record.turn_id : null,
    item_id: typeof record.item_id === 'string' ? record.item_id : null,
    kind: stringOr(record.kind, 'provider_specific') as ApprovalKind,
    title: stringOr(record.title, 'Approval requested'),
    details: typeof record.details === 'string' ? record.details : null,
    options: arrayValue(record.options).map(normalizeApprovalOption),
    status: stringOr(record.status, 'pending'),
    risk: typeof record.risk === 'string' ? record.risk : null,
    subject: typeof record.subject === 'string' ? record.subject : null,
    native_request_id: typeof record.native_request_id === 'string' ? record.native_request_id : null,
    native_blocking: record.native_blocking === true,
    policy: objectOrNull(record.policy),
    native: normalizeNativeRef(record.native),
    requested_at: typeof record.requested_at === 'string' ? record.requested_at : null,
    resolved_at: typeof record.resolved_at === 'string' ? record.resolved_at : null
  };
}

function normalizeApprovalOption(value: unknown): ApprovalOption {
  const record = objectRecord(value);
  return {
    option_id: stringOr(record.option_id, ''),
    kind: stringOr(record.kind, 'provider_specific') as ApprovalOptionKind,
    label: stringOr(record.label, stringOr(record.option_id, 'Option')),
    description: typeof record.description === 'string' ? record.description : null,
    scope: typeof record.scope === 'string' ? record.scope : null,
    native_option_id: typeof record.native_option_id === 'string' ? record.native_option_id : null,
    policy_rule: normalizeApprovalPolicyRulePreview(record.policy_rule)
  };
}

function normalizeApprovalPolicyRulePreview(value: unknown): ApprovalOption['policy_rule'] {
  const record = objectOrNull(value);
  if (!record) {
    return null;
  }
  const decision = objectRecord(record.decision);
  return {
    kind: stringOr(record.kind, 'provider_specific') as ApprovalKind,
    matcher: objectOrNull(record.matcher) ?? {},
    decision: {
      decision: stringOr(decision.decision, 'accept_for_session') as ApprovalDecision['decision'],
      option_id: typeof decision.option_id === 'string' ? decision.option_id : undefined,
      feedback: typeof decision.feedback === 'string' ? decision.feedback : undefined
    },
    label: stringOr(record.label, 'Remember approval')
  };
}

function normalizeQuestionState(value: unknown): QuestionState {
  const record = objectRecord(value);
  return {
    question_id: stringOr(record.question_id, ''),
    session_id: stringOr(record.session_id, ''),
    turn_id: typeof record.turn_id === 'string' ? record.turn_id : null,
    title: stringOr(record.title, 'Input requested'),
    description: typeof record.description === 'string' ? record.description : null,
    fields: arrayValue(record.fields).map(normalizeQuestionField),
    status: stringOr(record.status, 'pending') as QuestionStatus,
    answer: normalizeQuestionAnswer(record.answer),
    native: normalizeNativeRef(record.native),
    requested_at: typeof record.requested_at === 'string' ? record.requested_at : null,
    answered_at: typeof record.answered_at === 'string' ? record.answered_at : null
  };
}

function normalizeQuestionField(value: unknown): AgentQuestionField {
  const record = objectRecord(value);
  return {
    id: stringOr(record.id, ''),
    label: stringOr(record.label, stringOr(record.id, 'Field')),
    prompt: typeof record.prompt === 'string' ? record.prompt : null,
    kind: stringOr(record.kind, 'text'),
    required: record.required === true,
    secret: record.secret === true,
    choices: arrayValue(record.choices).map((choice) => {
      const choiceRecord = objectRecord(choice);
      return {
        value: stringOr(choiceRecord.value, ''),
        label: stringOr(choiceRecord.label, stringOr(choiceRecord.value, 'Option')),
        description: typeof choiceRecord.description === 'string' ? choiceRecord.description : null
      };
    }),
    default_answers: arrayValue(record.default_answers).filter(isString)
  };
}

function normalizeQuestionAnswer(value: unknown): AgentQuestionAnswer | null {
  const record = objectRecord(value);
  if (Object.keys(record).length === 0) {
    return null;
  }
  return {
    question_id: stringOr(record.question_id, ''),
    answers: normalizeRecord(record.answers, (answers) => arrayValue(answers).filter(isString))
  };
}

function normalizePlanState(value: unknown): PlanState {
  const record = objectRecord(value);
  return {
    plan_id: stringOr(record.plan_id, ''),
    session_id: stringOr(record.session_id, ''),
    turn_id: typeof record.turn_id === 'string' ? record.turn_id : null,
    status: stringOr(record.status, 'none') as PlanStatus,
    title: typeof record.title === 'string' ? record.title : null,
    content: typeof record.content === 'string' ? record.content : null,
    entries: arrayValue(record.entries).map(normalizePlanEntry),
    artifact_refs: arrayValue(record.artifact_refs).filter(isString),
    source: stringOr(record.source, 'native_structured') as PlanSource,
    partial: record.partial === true,
    updated_at: typeof record.updated_at === 'string' ? record.updated_at : null
  };
}

function normalizePlanEntry(value: unknown): UniversalPlanEntry {
  const record = objectRecord(value);
  return {
    entry_id: stringOr(record.entry_id, ''),
    label: stringOr(record.label, ''),
    status: stringOr(record.status, 'pending') as PlanEntryStatus
  };
}

function normalizeDiffState(value: unknown): DiffState {
  const record = objectRecord(value);
  return {
    diff_id: stringOr(record.diff_id, ''),
    session_id: stringOr(record.session_id, ''),
    turn_id: typeof record.turn_id === 'string' ? record.turn_id : null,
    title: typeof record.title === 'string' ? record.title : null,
    files: arrayValue(record.files).map(normalizeDiffFile),
    updated_at: typeof record.updated_at === 'string' ? record.updated_at : null
  };
}

function normalizeDiffFile(value: unknown): DiffFile {
  const record = objectRecord(value);
  return {
    path: stringOr(record.path, '(unknown)'),
    status: stringOr(record.status, 'modified') as FileChangeKind,
    diff: typeof record.diff === 'string' ? record.diff : null
  };
}

function normalizeArtifactState(value: unknown): ArtifactState {
  const record = objectRecord(value);
  return {
    artifact_id: stringOr(record.artifact_id, ''),
    session_id: stringOr(record.session_id, ''),
    turn_id: typeof record.turn_id === 'string' ? record.turn_id : null,
    kind: stringOr(record.kind, 'native') as ArtifactKind,
    title: stringOr(record.title, 'Artifact'),
    uri: typeof record.uri === 'string' ? record.uri : null,
    mime_type: typeof record.mime_type === 'string' ? record.mime_type : null,
    created_at: typeof record.created_at === 'string' ? record.created_at : null
  };
}

function normalizeNativeRef(value: unknown): NativeRef | null {
  const record = objectRecord(value);
  if (typeof record.protocol !== 'string') {
    return null;
  }
  return {
    protocol: record.protocol,
    method: typeof record.method === 'string' ? record.method : null,
    type: typeof record.type === 'string' ? record.type : null,
    native_id: typeof record.native_id === 'string' ? record.native_id : null,
    summary: typeof record.summary === 'string' ? record.summary : null,
    hash: typeof record.hash === 'string' ? record.hash : null,
    pointer: typeof record.pointer === 'string' ? record.pointer : null
  };
}

function normalizeAgentModelOption(value: unknown): AgentModelOption | undefined {
  const record = objectRecord(value);
  if (typeof record.id !== 'string') {
    return undefined;
  }
  return {
    id: record.id,
    display_name: typeof record.display_name === 'string' ? record.display_name : record.id,
    description: typeof record.description === 'string' ? record.description : null,
    is_default: record.is_default === true,
    default_reasoning_effort: isReasoningEffort(record.default_reasoning_effort)
      ? record.default_reasoning_effort
      : null,
    supported_reasoning_efforts: arrayValue(record.supported_reasoning_efforts).filter(
      isReasoningEffort
    ),
    input_modalities: arrayValue(record.input_modalities).filter(isString)
  };
}

function normalizeCollaborationMode(value: unknown): AgentCollaborationMode | undefined {
  const record = objectRecord(value);
  if (typeof record.id !== 'string') {
    return undefined;
  }
  return {
    id: record.id,
    label: typeof record.label === 'string' ? record.label : record.id,
    model: typeof record.model === 'string' ? record.model : null,
    reasoning_effort: isReasoningEffort(record.reasoning_effort) ? record.reasoning_effort : null
  };
}

function normalizeRunner(value: unknown): RunnerInfo | undefined {
  const record = objectRecord(value);
  if (typeof record.runner_id !== 'string') {
    return undefined;
  }
  return {
    runner_id: record.runner_id,
    name: typeof record.name === 'string' ? record.name : record.runner_id,
    status: normalizeRunnerStatus(record.status),
    last_seen_at: typeof record.last_seen_at === 'string' ? record.last_seen_at : null
  };
}

function normalizeWorkspace(value: unknown): WorkspaceRef | undefined {
  const record = objectRecord(value);
  if (typeof record.workspace_id !== 'string' || typeof record.runner_id !== 'string') {
    return undefined;
  }
  return {
    workspace_id: record.workspace_id,
    runner_id: record.runner_id,
    path: typeof record.path === 'string' ? record.path : 'Unknown workspace',
    display_name: typeof record.display_name === 'string' ? record.display_name : null
  };
}

function normalizeSession(value: unknown): SessionInfo | undefined {
  const record = objectRecord(value);
  if (typeof record.session_id !== 'string' || typeof record.workspace_id !== 'string') {
    return undefined;
  }
  return {
    session_id: record.session_id,
    owner_user_id: typeof record.owner_user_id === 'string' ? record.owner_user_id : '',
    runner_id: typeof record.runner_id === 'string' ? record.runner_id : '',
    workspace_id: record.workspace_id,
    provider_id: typeof record.provider_id === 'string' ? record.provider_id : 'unknown',
    status: isSessionStatus(record.status) ? record.status : 'degraded',
    external_session_id:
      typeof record.external_session_id === 'string' ? record.external_session_id : null,
    title: typeof record.title === 'string' ? record.title : null,
    created_at: typeof record.created_at === 'string' ? record.created_at : null,
    updated_at: typeof record.updated_at === 'string' ? record.updated_at : null,
    usage: normalizeSessionUsage(record.usage)
  };
}

export function normalizeSessionUsage(value: unknown): SessionUsageSnapshot | null {
  if (value === undefined || value === null) {
    return null;
  }
  const record = objectRecord(value);
  return {
    mode_label: typeof record.mode_label === 'string' ? record.mode_label : null,
    model: typeof record.model === 'string' ? record.model : null,
    reasoning_effort: isReasoningEffort(record.reasoning_effort) ? record.reasoning_effort : null,
    context: normalizeUsageContext(record.context),
    window_5h: normalizeUsageWindow(record.window_5h),
    week: normalizeUsageWindow(record.week)
  };
}

function normalizeUsageContext(value: unknown): SessionUsageContext | null {
  if (value === undefined || value === null) {
    return null;
  }
  const record = objectRecord(value);
  return {
    used_percent: numberOrNull(record.used_percent),
    used_tokens: numberOrNull(record.used_tokens),
    total_tokens: numberOrNull(record.total_tokens)
  };
}

function normalizeUsageWindow(value: unknown): SessionUsageWindow | null {
  if (value === undefined || value === null) {
    return null;
  }
  const record = objectRecord(value);
  return {
    used_percent: numberOrNull(record.used_percent),
    remaining_percent: numberOrNull(record.remaining_percent),
    resets_at: typeof record.resets_at === 'string' ? record.resets_at : null,
    window_label: typeof record.window_label === 'string' ? record.window_label : null,
    remaining_text_hint:
      typeof record.remaining_text_hint === 'string' ? record.remaining_text_hint : null
  };
}

function normalizeRunnerStatus(value: unknown): RunnerStatus {
  return typeof value === 'string' ? value : 'offline';
}

function objectRecord(value: unknown): Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : {};
}

function arrayValue(value: unknown): unknown[] {
  return Array.isArray(value) ? value : [];
}

function normalizeRecord<T>(
  value: unknown,
  normalize: (entry: unknown) => T
): Record<string, T> {
  const record = objectRecord(value);
  return Object.fromEntries(Object.entries(record).map(([key, entry]) => [key, normalize(entry)]));
}

function boolGroup(value: unknown): Record<string, boolean> {
  const record = objectRecord(value);
  return Object.fromEntries(Object.entries(record).map(([key, entry]) => [key, entry === true]));
}

function stringOr(value: unknown, fallback: string): string {
  return typeof value === 'string' ? value : fallback;
}

function normalizeSeq(value: unknown): string | undefined {
  if (typeof value === 'string' && /^\d+$/.test(value)) {
    return value;
  }
  if (typeof value === 'number' && Number.isSafeInteger(value) && value >= 0) {
    return String(value);
  }
  return undefined;
}

function isUuid(value: string): boolean {
  return /^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i.test(value);
}

function objectOrNull(value: unknown): Record<string, unknown> | null {
  const record = objectRecord(value);
  return Object.keys(record).length > 0 ? record : null;
}

function minimalSessionInfo(): SessionInfo {
  return {
    session_id: '',
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

function numberOrNull(value: unknown): number | null {
  return typeof value === 'number' && Number.isFinite(value) ? value : null;
}

function isReasoningEffort(value: unknown): value is AgentReasoningEffort {
  return typeof value === 'string' && reasoningEfforts.has(value as AgentReasoningEffort);
}

function isSessionStatus(value: unknown): value is SessionStatus {
  return typeof value === 'string' && sessionStatuses.has(value as SessionStatus);
}

function isString(value: unknown): value is string {
  return typeof value === 'string';
}

function isPresent<T>(value: T | undefined): value is T {
  return value !== undefined;
}
