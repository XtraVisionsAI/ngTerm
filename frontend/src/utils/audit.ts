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
  exit: { kind: 'known'; code: number } | { kind: 'unknown'; reason: string } | null
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

/** One event of an operation, as returned by /audit/operations/{id}. */
export interface AuditEvent {
  eventId: string
  streamId: string
  seq: number
  occurredAt: string
  recordedAt: string
  sessionId: string | null
  operationId: string | null
  eventType: string
  payload: Record<string, unknown>
  integrity: Integrity
}

/** Payload of a `config.change` event (redacted snapshots, never secrets). */
export interface ConfigChangePayload {
  object: string
  objectId: string | null
  action: string
  outcome: 'succeeded' | 'failed'
  error: string | null
  before: Record<string, unknown> | null
  after: Record<string, unknown> | null
  changedFields: string[]
}

/** Startup report of the running process, as returned in /audit/system. */
export interface StartupReport {
  startedAt: string
  previousShutdownClean: boolean | null
  sessionsClosed: number
  operationsInterrupted: number
  recordingsInterrupted: number
  recordingEnabled: boolean
}

export type TagType = 'default' | 'success' | 'warning' | 'error' | 'info'

export const kindLabel: Record<string, string> = {
  command: '命令',
  file_read: '读文件',
  file_write: '写文件',
  file_delete: '删除文件',
  file_rename: '重命名',
  mkdir: '建目录',
  upload: '上传',
  download: '下载',
  git: 'Git',
  tool_call: '工具调用',
  mcp_call: 'MCP 调用',
  approval: '审批',
  config_change: '配置变更',
  agent_launch: 'Agent 启动'
}

export const objectKindLabel: Record<string, string> = {
  user: '用户',
  server: '服务器',
  ssh_key: 'SSH 密钥',
  ai_tool: 'AI 工具',
  user_tool_config: '用户工具配置',
  server_tool_config: '服务器工具配置'
}

export const actionLabel: Record<string, string> = {
  create: '创建',
  update: '修改',
  delete: '删除',
  password_change: '修改密码',
  password_reset: '重置密码'
}

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
  if (exit.kind === 'known') return String(exit.code)
  return `未知（${exit.reason}）`
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

export interface RuntimeMetrics {
  takenAt: string
  auditWriteFailures: number
  lastAuditWriteError: { at: string; error: string } | null
  recording: { enabled: boolean; eventsDropped: number; queueCapacity: number }
  disk: { path: string; freeBytes: number; totalBytes: number; lowThresholdBytes: number; low: boolean } | null
  operations: { running: number; stale: number; staleAfterSecs: number }
  activeSessions: number
  activeAgents: number
  warnings: string[]
}

export const warningLabel: Record<string, string> = {
  audit_write_failures: '审计记录写入失败',
  recording_drops: '录像事件被丢弃',
  low_disk: '数据目录磁盘不足',
  disk_unknown: '无法获取磁盘余量',
  stale_operations: '存在长时间未结束的操作',
  unclean_previous_shutdown: '上一进程异常退出',
  recording_disabled: '终端录制已关闭'
}
