import type { RunnerInfo, RunnerStatus, SessionInfo, WorkspaceRef } from '../api/types';

export interface SessionTreeGroup {
  id: string;
  label: string;
  runner: RunnerInfo;
  workspace: WorkspaceRef;
  status: 'online' | 'offline';
  sessions: SessionInfo[];
}

export interface BuildSessionTreeInput {
  runners: RunnerInfo[];
  workspacesByRunner: Record<string, WorkspaceRef[]>;
  sessions: SessionInfo[];
}

export function runnerStatusTone(status: RunnerStatus | undefined): 'online' | 'offline' {
  return status === 'online' || status === 'connected' ? 'online' : 'offline';
}

export function buildSessionTree({
  runners,
  workspacesByRunner,
  sessions
}: BuildSessionTreeInput): SessionTreeGroup[] {
  const sessionsByWorkspace = new Map<string, SessionInfo[]>();
  for (const session of sessions) {
    const current = sessionsByWorkspace.get(session.workspace_id) ?? [];
    current.push(session);
    sessionsByWorkspace.set(session.workspace_id, current);
  }

  return runners
    .flatMap((runner) =>
      (workspacesByRunner[runner.runner_id] ?? []).map((workspace) => {
        const workspaceLabel = workspace.display_name || workspace.path;
        return {
          id: `${runner.runner_id}:${workspace.workspace_id}`,
          label: `${runner.name}:${workspaceLabel}`,
          runner,
          workspace,
          status: runnerStatusTone(runner.status),
          sessions: sortSessionsByDate(sessionsByWorkspace.get(workspace.workspace_id) ?? [])
        };
      })
    )
    .sort((left, right) => left.label.localeCompare(right.label));
}

export function sortSessionsByDate(sessions: SessionInfo[]): SessionInfo[] {
  return sessions.map((session, index) => ({ session, index })).sort((left, right) => {
    const dateOrder = sessionTime(right.session) - sessionTime(left.session);
    if (dateOrder !== 0) {
      return dateOrder;
    }
    if (sessionTime(left.session) !== 0 || sessionTime(right.session) !== 0) {
      return left.index - right.index;
    }
    const leftTitle = left.session.title?.trim();
    const rightTitle = right.session.title?.trim();
    if (leftTitle && rightTitle) {
      return leftTitle.localeCompare(rightTitle);
    }
    if (leftTitle) {
      return -1;
    }
    if (rightTitle) {
      return 1;
    }
    return left.session.session_id.localeCompare(right.session.session_id);
  }).map(({ session }) => session);
}

function sessionTime(session: SessionInfo): number {
  const value = session.updated_at ?? session.created_at;
  if (!value) {
    return 0;
  }
  const time = Date.parse(value);
  return Number.isFinite(time) ? time : 0;
}
