import type { TagType } from '@/utils/audit'

/** Shapes of the enterprise operations flows API (`/api/flows`, `/api/flow-runs`). */
export type ParamType = 'string' | 'int' | 'bool' | 'enum'

export interface ParamDef {
  key: string
  label: string
  type: ParamType
  required: boolean
  default?: unknown
  pattern?: string | null
  options: string[]
  description: string
}

export interface Expect {
  exitCode?: number | null
  contains?: string | null
}

export interface Check {
  name: string
  command: string
  expect: Expect
  failMessage: string
}

export interface Step {
  name: string
  command: string
  approval: 'none' | 'required'
  timeoutSecs?: number | null
  failOnError: boolean
  verify?: Check | null
}

export interface Definition {
  params: ParamDef[]
  preChecks: Check[]
  steps: Step[]
  cwd?: string | null
  readOnly?: boolean
}

export interface Flow {
  flowId: string
  name: string
  description: string
  version: number
  definition: Definition
  builtin: boolean
  createdBy: string | null
  createdAt: string
  updatedAt: string
}

export interface CommandResult {
  command: string
  operationId: string | null
  exitCode: number | null
  output: string
  truncated: boolean
  durationMs: number
  passed: boolean
  message: string | null
}

export type CheckResult = CommandResult & { name: string }

export interface StepResult {
  index: number
  name: string
  approval: 'none' | 'required'
  approvalRequestId: string | null
  result: CommandResult
  verify: CheckResult | null
  finishedAt: string
}

export type RunStatus = 'ready' | 'precheck_failed' | 'finished' | 'failed' | 'aborted' | 'skipped'

export interface Run {
  runId: string
  batchId: string | null
  flowId: string
  flowName: string
  flowVersion: number
  definition: Definition
  userId: string
  sessionId: string
  serverId: string | null
  serverAlias: string | null
  params: Record<string, string>
  status: RunStatus
  currentStep: number
  preChecks: CheckResult[]
  steps: StepResult[]
  error: string | null
  startedAt: string
  updatedAt: string
  finishedAt: string | null
}

export const runStatusInfo: Record<RunStatus, { label: string; type: TagType }> = {
  ready: { label: '进行中', type: 'info' },
  precheck_failed: { label: '前置检查未通过', type: 'warning' },
  finished: { label: '已完成', type: 'success' },
  failed: { label: '失败', type: 'error' },
  aborted: { label: '已中止', type: 'default' },
  skipped: { label: '已跳过', type: 'warning' }
}

export type BatchStatus = 'running' | 'finished' | 'cancelled'

export interface Batch {
  batchId: string
  flowId: string
  flowName: string
  flowVersion: number
  userId: string
  params: Record<string, string>
  serverIds: string[]
  concurrency: number
  status: BatchStatus
  total: number
  succeeded: number
  failed: number
  skipped: number
  startedAt: string
  updatedAt: string
  finishedAt: string | null
}

export const batchStatusInfo: Record<BatchStatus, { label: string; type: TagType }> = {
  running: { label: '执行中', type: 'info' },
  finished: { label: '已完成', type: 'success' },
  cancelled: { label: '已取消', type: 'default' }
}

/** Whether a flow may run across many servers at once. */
export function batchEligible(def: Definition): boolean {
  return !!def.readOnly && !def.steps.some((s) => s.approval === 'required')
}

/** Initial form values from a definition's defaults. */
export function defaultParams(def: Definition): Record<string, unknown> {
  const out: Record<string, unknown> = {}
  for (const p of def.params) {
    if (p.default !== undefined && p.default !== null) out[p.key] = p.default
    else if (p.type === 'bool') out[p.key] = false
    else out[p.key] = null
  }
  return out
}

export type NotifyOn = 'always' | 'failure' | 'never'

export interface Schedule {
  scheduleId: string
  flowId: string
  flowVersion: number
  name: string
  userId: string
  params: Record<string, string>
  serverIds: string[]
  concurrency: number
  intervalSecs: number
  enabled: boolean
  notifyOn: NotifyOn
  createdAt: string
  updatedAt: string
  nextRunAt: string
  lastRunAt: string | null
  lastBatchId: string | null
  lastStatus: string | null
}

/** Human interval like "每 30 分钟" / "每 2 小时" / "每 1 天". */
export function intervalLabel(secs: number): string {
  if (secs % 86400 === 0) return `每 ${secs / 86400} 天`
  if (secs % 3600 === 0) return `每 ${secs / 3600} 小时`
  if (secs % 60 === 0) return `每 ${secs / 60} 分钟`
  return `每 ${secs} 秒`
}

/** Preset intervals offered in the schedule form. */
export const intervalOptions: { label: string; value: number }[] = [
  { label: '每 5 分钟', value: 300 },
  { label: '每 15 分钟', value: 900 },
  { label: '每 30 分钟', value: 1800 },
  { label: '每小时', value: 3600 },
  { label: '每 6 小时', value: 21600 },
  { label: '每 12 小时', value: 43200 },
  { label: '每天', value: 86400 },
  { label: '每周', value: 604800 }
]

export const notifyOnLabel: Record<NotifyOn, string> = {
  always: '每次都通知',
  failure: '仅失败时通知',
  never: '不通知'
}
