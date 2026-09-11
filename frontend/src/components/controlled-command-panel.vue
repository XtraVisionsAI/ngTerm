<script setup lang="ts">
  import type { OperationRecord } from '@/utils/audit'
  import { NButton, NEmpty, NInput, NInputNumber, NTag, NTooltip, useMessage } from 'naive-ui'
  import { onMounted, ref, watch } from 'vue'
  import { useApi } from '@/composables/useApi'
  import { postWithAdmission } from '@/composables/useSessionAdmission'
  import { statusInfo } from '@/utils/audit'
  import { formatTime } from '@/utils/format'

  const props = defineProps<{ sessionId: string }>()

  const api = useApi()
  const message = useMessage()

  interface RunResult {
    operationId: string
    status: 'succeeded' | 'failed' | 'timed_out'
    exitCode?: number
    output?: string
    truncated?: boolean
    outputBytes?: number
    durationMs: number
    error?: string
  }

  const command = ref('')
  const cwd = ref('')
  const timeoutSecs = ref(120)
  const running = ref(false)
  const waiting = ref<{ requestId: string; message: string } | null>(null)
  const result = ref<RunResult | null>(null)
  const history = ref<OperationRecord[]>([])
  const loadingHistory = ref(false)
  const selected = ref<{ op: OperationRecord; output: Record<string, unknown> | null } | null>(null)

  async function loadHistory() {
    loadingHistory.value = true
    try {
      const params = new URLSearchParams({ session: props.sessionId, kind: 'command', limit: '30' })
      const data = await api.get<{ items: OperationRecord[] }>(`/audit/operations?${params}`)
      // Only the controlled channel is submitted through the API; the
      // engine's commands come through chat/terminal sources.
      history.value = data.items.filter((o) => o.source === 'api')
    } catch {
      history.value = []
    } finally {
      loadingHistory.value = false
    }
  }

  onMounted(loadHistory)
  watch(() => props.sessionId, loadHistory)

  async function run() {
    const cmd = command.value.trim()
    if (!cmd || running.value) return
    running.value = true
    result.value = null
    waiting.value = null
    try {
      const res = await postWithAdmission<RunResult>(
        api,
        `/sessions/${props.sessionId}/controlled-commands`,
        { command: cmd, cwd: cwd.value.trim() || undefined, timeoutSecs: timeoutSecs.value },
        (requestId, msg) => {
          waiting.value = { requestId, message: msg }
        }
      )
      result.value = res
      if (res.status === 'succeeded') message.success(`执行完成（exit ${res.exitCode}）`)
    } catch (e) {
      const msg = (e as Error).message
      // Non-2xx answers still carry the operation record when execution
      // started; the error text is what the server said.
      result.value = { operationId: '', status: 'failed', durationMs: 0, error: msg }
      message.error(msg)
    } finally {
      waiting.value = null
      running.value = false
      loadHistory()
    }
  }

  async function openHistory(op: OperationRecord) {
    try {
      const d = await api.get<{
        operation: OperationRecord
        events: { eventType: string; payload: Record<string, unknown> }[]
      }>(`/audit/operations/${op.operationId}`)
      const out = d.events.find((e) => e.eventType === 'command.output')
      selected.value = { op: d.operation, output: out ? out.payload : null }
    } catch (e) {
      message.error((e as Error).message)
    }
  }

  function exitText(op: OperationRecord): string {
    if (!op.exit) return ''
    return op.exit.kind === 'known' ? `exit ${op.exit.code}` : op.exit.reason
  }

  function handleKeydown(e: KeyboardEvent) {
    if ((e.metaKey || e.ctrlKey) && e.key === 'Enter') {
      e.preventDefault()
      run()
    }
  }
</script>

<template>
  <div class="h-full flex flex-col overflow-hidden bg-om-panel text-xs">
    <div class="flex items-center justify-between border-b border-om-border px-3 py-2">
      <span class="font-semibold">受管命令</span>
      <n-tooltip>
        <template #trigger>
          <i class="i-ri:information-line text-om-dimmed" style="display: inline-block; width: 14px; height: 14px" />
        </template>
        命令由服务端在独立通道执行并完整登记（含输出），不经过当前终端；策略要求时会先等待第二人审批。
      </n-tooltip>
    </div>

    <div class="border-b border-om-border p-3">
      <n-input
        v-model:value="command"
        type="textarea"
        size="small"
        placeholder="完整命令，例如 systemctl status nginx（⌘/Ctrl+Enter 提交）"
        :autosize="{ minRows: 2, maxRows: 6 }"
        :disabled="running"
        class="font-mono"
        @keydown="handleKeydown"
      />
      <div class="mt-2 flex items-center gap-2">
        <n-input v-model:value="cwd" size="small" placeholder="工作目录（可选，绝对路径或 ~）" :disabled="running" />
        <n-input-number
          v-model:value="timeoutSecs"
          size="small"
          :min="1"
          :max="600"
          :show-button="false"
          style="width: 80px"
          :disabled="running"
        >
          <template #suffix>s</template>
        </n-input-number>
        <n-button size="small" type="primary" :loading="running" :disabled="!command.trim()" @click="run">
          提交执行
        </n-button>
      </div>
      <div v-if="waiting" class="mt-2 flex items-center gap-2 border border-om-warning rounded bg-om-bg p-2">
        <i class="i-ri:loader-4-line animate-spin" style="display: inline-block; width: 14px; height: 14px" />
        <span>{{ waiting.message }}</span>
        <router-link
          class="ml-auto text-om-primary"
          :to="{ path: '/approvals', query: { request: waiting.requestId } }"
        >
          查看申请
        </router-link>
      </div>
    </div>

    <div v-if="result" class="border-b border-om-border p-3">
      <div class="mb-1 flex items-center gap-2">
        <n-tag
          size="small"
          :bordered="false"
          :type="result.status === 'succeeded' ? 'success' : result.status === 'timed_out' ? 'warning' : 'error'"
        >
          {{ result.status === 'succeeded' ? '成功' : result.status === 'timed_out' ? '超时（结果未知）' : '失败' }}
        </n-tag>
        <span v-if="result.exitCode !== undefined" class="font-mono">exit {{ result.exitCode }}</span>
        <span class="text-om-dimmed">{{ result.durationMs }} ms</span>
        <span v-if="result.truncated" class="text-om-warning">输出已截断（共 {{ result.outputBytes }} 字节）</span>
      </div>
      <div v-if="result.error" class="text-om-error">{{ result.error }}</div>
      <pre
        v-if="result.output !== undefined"
        class="mt-1 max-h-64 overflow-auto whitespace-pre-wrap break-all rounded bg-om-bg p-2 font-mono"
        >{{ result.output || '(无输出)' }}</pre>
    </div>

    <div class="min-h-0 flex-1 overflow-auto p-3">
      <div class="mb-1 flex items-center justify-between text-om-dimmed">
        <span>本会话的受管命令记录</span>
        <n-button text size="tiny" :loading="loadingHistory" @click="loadHistory">刷新</n-button>
      </div>
      <n-empty v-if="history.length === 0" description="尚无记录" size="small" />
      <div
        v-for="op in history"
        :key="op.operationId"
        class="mb-1 cursor-pointer rounded px-2 py-1 hover:bg-om-bg"
        @click="openHistory(op)"
      >
        <div class="flex items-center gap-2">
          <n-tag size="tiny" :bordered="false" :type="statusInfo[op.status]?.type || 'default'">
            {{ statusInfo[op.status]?.label || op.status }}
          </n-tag>
          <span class="truncate font-mono">{{ op.summary }}</span>
        </div>
        <div class="text-om-dimmed">{{ formatTime(op.startedAt) }} · {{ exitText(op) }}</div>
      </div>

      <div v-if="selected" class="mt-3 border-t border-om-border pt-2">
        <div class="mb-1 flex items-center justify-between">
          <span class="font-mono">{{ selected.op.summary }}</span>
          <n-button text size="tiny" @click="selected = null">关闭</n-button>
        </div>
        <div v-if="selected.op.cwd" class="text-om-dimmed">cwd: {{ selected.op.cwd }}</div>
        <pre
          v-if="selected.output"
          class="mt-1 max-h-64 overflow-auto whitespace-pre-wrap break-all rounded bg-om-bg p-2 font-mono"
          >{{ (selected.output.output as string) || '(无输出)' }}</pre>
        <div v-else class="text-om-dimmed">该记录没有输出事件（未执行或未能记录）。</div>
        <div v-if="selected.output?.truncated" class="text-om-warning">审计中的输出已截断。</div>
      </div>
    </div>
  </div>
</template>
