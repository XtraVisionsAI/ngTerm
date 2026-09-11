export type ApprovalKind = 'session_admission' | 'operation'
export type ApprovalStatus = 'pending' | 'approved' | 'rejected' | 'expired' | 'cancelled'

export interface ApprovalRequest {
  requestId: string
  kind: ApprovalKind
  status: ApprovalStatus
  requesterUserId: string
  requesterUsername: string | null
  sessionId: string | null
  agentId: string | null
  serverId: string | null
  serverAlias: string | null
  serverGroup: string | null
  remoteUser: string | null
  operationKind: string | null
  summary: string
  snapshot: Record<string, unknown>
  snapshotHash: string
  policyVersion: string
  riskLevel: string | null
  riskReason: string | null
  createdAt: string
  expiresAt: string
  decidedAt: string | null
  decidedBy: string | null
  decidedByUsername: string | null
  decisionComment: string | null
  consumedAt: string | null
  consumedOperationId: string | null
  cancelledAt: string | null
  cancelReason: string | null
  canDecide: boolean
  canCancel: boolean
}

export interface ApprovalRole {
  userId: string
  username: string | null
  role: string
  scopeKind: string
  scopeId: string
  grantedBy: string
  grantedAt: string
}

export interface ApprovalPolicy {
  scopeKind: string
  scopeId: string
  sessionAdmission: boolean
  operationMinRisk: string | null
  operationKinds: string[]
  ttlSecs: number
  updatedBy: string
  updatedAt: string
  version?: string
}

export const approvalKindLabel: Record<ApprovalKind, string> = {
  session_admission: '会话准入',
  operation: '操作'
}

export const approvalStatusInfo: Record<
  ApprovalStatus,
  { label: string; type: 'warning' | 'success' | 'error' | 'default' | 'info' }
> = {
  pending: { label: '待审批', type: 'warning' },
  approved: { label: '已批准', type: 'success' },
  rejected: { label: '已拒绝', type: 'error' },
  expired: { label: '已过期', type: 'default' },
  cancelled: { label: '已撤回', type: 'default' }
}

export const riskLevelLabel: Record<string, string> = {
  low: '低',
  medium: '中',
  high: '高',
  critical: '严重'
}

export const scopeKindLabel: Record<string, string> = {
  all: '全部服务器',
  group: '服务器分组',
  server: '单台服务器'
}

export const policyOperationKinds: { label: string; value: string }[] = [
  { label: '命令执行', value: 'command' },
  { label: '文件读取', value: 'file_read' },
  { label: '文件写入', value: 'file_write' },
  { label: '文件删除', value: 'file_delete' },
  { label: '文件重命名', value: 'file_rename' },
  { label: '创建目录', value: 'mkdir' },
  { label: '上传', value: 'upload' },
  { label: '下载', value: 'download' },
  { label: 'Git 操作', value: 'git' },
  { label: '工具调用', value: 'tool_call' },
  { label: 'MCP 调用', value: 'mcp_call' },
  { label: '启动 Agent', value: 'agent_launch' }
]

export function scopeLabel(kind: string, id: string): string {
  if (kind === 'all') return scopeKindLabel.all
  return `${scopeKindLabel[kind] || kind}: ${id}`
}

/** Seconds until `iso`, never negative. */
export function secondsUntil(iso: string): number {
  return Math.max(0, Math.floor((new Date(iso).getTime() - Date.now()) / 1000))
}
