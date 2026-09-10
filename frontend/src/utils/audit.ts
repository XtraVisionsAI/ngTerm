/** Shared types and labels for the audit pages. */

export interface Actor {
  kind: 'human' | 'external_tool' | 'embedded_agent' | 'platform' | null
  user_id: string | null
  username: string | null
  tool_id: string | null
  agent_id: string | null
  remote_addr: string | null
}

export interface Target {
  server_id: string | null
  server_alias: string | null
  server_host: string | null
  remote_user: string | null
}

export type Integrity = { kind: 'complete' } | { kind: 'gap'; dropped: number } | { kind: 'truncated'; reason: string }

export interface AuditSession {
  sessionId: string
  actor: Actor
  target: Target
  source: 'terminal' | 'chat' | 'api' | 'background'
  parentSessionId: string | null
  connectedAt: string
  disconnectedAt: string | null
  disconnectReason: string | null
  integrity: Integrity
}

export interface OperationRecord {
  operationId: string
  parentOperationId: string | null
  sessionId: string | null
  taskId: string | null
  actor: Actor
  source: string
  kind: string
  summary: string
  target: Target
  cwd: string | null
  status: string
  exit: { Known: { code: number } } | { Unknown: { reason: string } } | null
  evidence: string
  startedAt: string
  finishedAt: string | null
}

export interface RecordingMeta {
  recordingId: string
  sessionId: string
  startedAt: string
  endedAt: string | null
  status: 'recording' | 'complete' | 'interrupted'
  integrity: Integrity
  cols: number
  rows: number
  inputPolicy: string
  chunkCount: number
  totalBytes: number
  durationMs: number | null
}

export interface ChunkProblem {
  kind: 'missing' | 'corrupt' | 'index_gap'
  seq?: number
  expected_seq?: number
  detail?: string
}

/** One playback event as served by /audit/recordings/{id}/events. */
export interface RecordedEvent {
  t: number
  k: 'o' | 'i' | 'r' | 'g' | 'm'
  d?: string
  n?: number
  c?: number
  r?: number
}

export type TagType = 'default' | 'success' | 'warning' | 'error' | 'info'

export const actorKindLabel: Record<string, string> = {
  human: '人工',
  external_tool: '外部工具',
  embedded_agent: '内置 Agent',
  platform: '平台'
}

export const sourceLabel: Record<string, string> = {
  terminal: '终端',
  chat: '对话',
  api: 'API',
  background: '后台'
}

export const statusInfo: Record<string, { label: string; type: TagType }> = {
  intended: { label: '已登记', type: 'default' },
  running: { label: '执行中', type: 'info' },
  succeeded: { label: '成功', type: 'success' },
  failed: { label: '失败', type: 'error' },
  denied: { label: '已拒绝', type: 'warning' },
  timed_out: { label: '超时', type: 'warning' },
  cancelled: { label: '已取消', type: 'default' },
  interrupted: { label: '中断', type: 'warning' },
  unknown: { label: '未知', type: 'warning' }
}

export const evidenceLabel: Record<string, string> = {
  executor_confirmed: '执行器确认',
  parsed_from_output: '输出解析',
  declared: '自述',
  none: '无'
}

export const reasonInfo: Record<string, { label: string; type: TagType }> = {
  ssh_closed: { label: '远端关闭', type: 'info' },
  user_closed: { label: '主动断开', type: 'info' },
  idle_timeout: { label: '空闲超时', type: 'warning' },
  input_closed: { label: '输入通道关闭', type: 'warning' },
  server_shutdown: { label: '服务停止', type: 'warning' },
  server_restart: { label: '服务重启', type: 'warning' }
}

export function integrityInfo(i: Integrity | undefined): { label: string; type: TagType; detail?: string } {
  if (!i) return { label: '-', type: 'default' }
  switch (i.kind) {
    case 'complete':
      return { label: '完整', type: 'success' }
    case 'gap':
      return { label: `缺口 ${i.dropped}`, type: 'warning', detail: `${i.dropped} 个事件在采集时丢失` }
    case 'truncated':
      return { label: '截断', type: 'error', detail: i.reason }
  }
}

export function exitLabel(exit: OperationRecord['exit']): string {
  if (!exit) return '-'
  if ('Known' in exit) return String(exit.Known.code)
  return `未知（${exit.Unknown.reason}）`
}

export function formatMs(ms: number): string {
  const total = Math.floor(ms / 1000)
  const h = Math.floor(total / 3600)
  const m = Math.floor((total % 3600) / 60)
  const s = total % 60
  const mm = String(m).padStart(2, '0')
  const ss = String(s).padStart(2, '0')
  return h > 0 ? `${h}:${mm}:${ss}` : `${mm}:${ss}`
}

export function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KiB`
  return `${(n / 1024 / 1024).toFixed(1)} MiB`
}
