import { requestJson } from './http';
import type { AgentProviderId, ApprovalPolicyRule } from './types';

export async function listApprovalRules(
  workspaceId: string,
  providerId: AgentProviderId
): Promise<ApprovalPolicyRule[]> {
  const params = new URLSearchParams({
    workspace_id: workspaceId,
    provider_id: providerId
  });
  return requestJson<ApprovalPolicyRule[]>(`/api/approval-rules?${params.toString()}`);
}

export async function disableApprovalRule(ruleId: string): Promise<ApprovalPolicyRule> {
  return requestJson<ApprovalPolicyRule>(
    `/api/approval-rules/${encodeURIComponent(ruleId)}/disable`,
    { method: 'POST' }
  );
}
