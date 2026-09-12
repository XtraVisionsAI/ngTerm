import type { TagType } from '@/utils/audit'

/** Shapes returned by the enterprise task center (`/api/tasks`). */
export type TaskStatus = 'running' | 'stopped' | 'interrupted'

export interface TaskRecord {
  taskId: string
  userId: string
  sessionId: string
  agentId: string
  serverId: string | null
  serverAlias: string | null
  toolId: string | null
  title: string
  initialPrompt: string
  workingDir: string | null
  target: string | null
  status: TaskStatus
  stopReason: string | null
  createdAt: string
  updatedAt: string
  finishedAt: string | null
  turns: number
  inputTokens: number
  outputTokens: number
  costUsd: number | null
  continuedFrom: string | null
  eventCount: number
  eventBytes: number
  truncated: boolean
}

export interface TaskEvent {
  seq: number
  ts: string
  event: Record<string, unknown> & { type?: string }
}

export interface TaskDetail {
  task: TaskRecord
  events: TaskEvent[]
  /** Whether the run is live in the current server process. */
  live: boolean
}

export const taskStatusInfo: Record<TaskStatus, { label: string; type: TagType }> = {
  running: { label: '运行中', type: 'success' },
  stopped: { label: '已结束', type: 'default' },
  interrupted: { label: '已中断', type: 'warning' }
}

/** Plain text of an `assistant` event: text blocks joined, tool uses named. */
export function assistantText(ev: Record<string, unknown>): string {
  const msg = ev.message as { content?: unknown } | undefined
  const content = msg?.content
  if (typeof content === 'string') return content
  if (!Array.isArray(content)) return ''
  return content
    .map((b: Record<string, unknown>) => {
      if (b.type === 'text') return String(b.text ?? '')
      if (b.type === 'tool_use') return `（调用工具 ${String(b.name ?? '?')}）`
      return ''
    })
    .filter(Boolean)
    .join('\n')
}
