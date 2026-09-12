<script setup lang="ts">
  /**
   * Task center (UX-02): every native agent run, searchable, with its
   * transcript, the audit operations it issued, export, and "continue" on a
   * live session of the same server after a target check.
   */
  import type { DataTableColumns } from 'naive-ui'
  import type { OperationRecord } from '@/utils/audit'
  import type { TaskDetail, TaskRecord, TaskStatus } from '@/utils/tasks'
  import {
    NButton,
    NDataTable,
    NDescriptions,
    NDescriptionsItem,
    NDrawer,
    NDrawerContent,
    NInput,
    NPagination,
    NSelect,
    NSpace,
    NTag,
    useMessage
  } from 'naive-ui'
  import { computed, h, onMounted, ref, watch } from 'vue'
  import { useRouter } from 'vue-router'
  import { useApi } from '@/composables/useApi'
  import { renderMarkdown } from '@/composables/useMarkdown'
  import { useUiState } from '@/composables/useUiState'
  import { useAuthStore } from '@/stores/auth'
  import { useSessionStore } from '@/stores/session'
  import { statusInfo as opStatusInfo } from '@/utils/audit'
  import { formatTime } from '@/utils/format'
  import { assistantText, taskStatusInfo } from '@/utils/tasks'

  const api = useApi()
  const message = useMessage()
  const router = useRouter()
  const auth = useAuthStore()
  const sessionStore = useSessionStore()
  const uiState = useUiState()

  const items = ref<TaskRecord[]>([])
  const total = ref(0)
  const loading = ref(false)
  const page = ref(1)
  const pageSize = 30
  const query = ref('')
  const filterStatus = ref<TaskStatus | null>(null)

  const statusOptions = (Object.keys(taskStatusInfo) as TaskStatus[]).map((k) => ({
    label: taskStatusInfo[k].label,
    value: k
  }))

  async function load() {
    loading.value = true
    try {
      const params = new URLSearchParams()
      if (query.value.trim()) params.set('q', query.value.trim())
      if (filterStatus.value) params.set('status', filterStatus.value)
      params.set('limit', String(pageSize))
      params.set('offset', String((page.value - 1) * pageSize))
      const r = await api.get<{ items: TaskRecord[]; total: number }>(`/tasks?${params}`)
      items.value = r.items
      total.value = r.total
    } catch (e) {
      message.error(`加载失败: ${(e as Error).message}`)
    } finally {
      loading.value = false
    }
  }

  let searchTimer: ReturnType<typeof setTimeout> | null = null
  watch(query, () => {
    if (searchTimer) clearTimeout(searchTimer)
    searchTimer = setTimeout(() => {
      page.value = 1
      load()
    }, 300)
  })
  watch([filterStatus, page], load)
  onMounted(load)

  // --- detail ---
  const showDetail = ref(false)
  const detail = ref<TaskDetail | null>(null)
  const operations = ref<OperationRecord[]>([])
  const detailLoading = ref(false)

  async function openDetail(row: TaskRecord) {
    showDetail.value = true
    detailLoading.value = true
    detail.value = null
    operations.value = []
    try {
      const [d, ops] = await Promise.all([
        api.get<TaskDetail>(`/tasks/${row.taskId}`),
        api.get<{ items: OperationRecord[] }>(`/tasks/${row.taskId}/operations`)
      ])
      detail.value = d
      operations.value = ops.items
    } catch (e) {
      message.error(`加载失败: ${(e as Error).message}`)
    } finally {
      detailLoading.value = false
    }
  }

  const transcript = computed(() => {
    if (!detail.value) return []
    return detail.value.events
      .map((ev) => {
        const t = ev.event.type
        if (t === 'user_message') return { seq: ev.seq, ts: ev.ts, role: 'user', text: String(ev.event.content ?? '') }
        if (t === 'assistant') {
          const text = assistantText(ev.event)
          return text ? { seq: ev.seq, ts: ev.ts, role: 'assistant', html: renderMarkdown(text) } : null
        }
        if (t === 'permission_request') {
          const tool = ev.event.tool as { name?: string; input?: unknown } | undefined
          return {
            seq: ev.seq,
            ts: ev.ts,
            role: 'tool',
            text: `${tool?.name ?? '?'} ${JSON.stringify(tool?.input ?? {})}`
          }
        }
        if (t === 'approval_decided') {
          return { seq: ev.seq, ts: ev.ts, role: 'system', text: `第二人审批：${ev.event.status ?? ''}` }
        }
        if (t === 'error') return { seq: ev.seq, ts: ev.ts, role: 'error', text: String(ev.event.message ?? '') }
        if (t === 'truncated') return { seq: ev.seq, ts: ev.ts, role: 'system', text: '记录已截断（超出存储上限）' }
        return null
      })
      .filter((x): x is NonNullable<typeof x> => x !== null)
  })

  // --- export ---
  async function exportTask(format: 'md' | 'json') {
    const t = detail.value?.task
    if (!t) return
    try {
      const res = await fetch(`/api/tasks/${t.taskId}/export?format=${format}`, {
        headers: auth.token ? { Authorization: `Bearer ${auth.token}` } : {}
      })
      if (!res.ok) throw new Error((await res.json().catch(() => ({ error: res.statusText }))).error)
      const blob = await res.blob()
      const url = URL.createObjectURL(blob)
      const a = document.createElement('a')
      a.href = url
      a.download = `task-${t.taskId.slice(5, 13)}.${format}`
      a.click()
      URL.revokeObjectURL(url)
    } catch (e) {
      message.error(`导出失败: ${(e as Error).message}`)
    }
  }

  // --- continue ---
  const continueSession = ref<string | null>(null)
  const continuePrompt = ref('')
  const continuing = ref(false)
  const candidateSessions = computed(() => {
    const t = detail.value?.task
    if (!t) return []
    return sessionStore.tabs
      .filter((tab) => tab.serverId === t.serverId && !tab.id.startsWith('pending-'))
      .map((tab) => ({ label: `${tab.serverAlias} · ${tab.id.slice(0, 8)}`, value: tab.id }))
  })
  const canContinue = computed(() => {
    const t = detail.value?.task
    return !!t && t.userId === auth.userId && !!t.toolId && !(detail.value?.live && t.status === 'running')
  })

  async function continueTask() {
    const t = detail.value?.task
    if (!t || !continueSession.value) return
    continuing.value = true
    try {
      const r = await api.post<{
        sessionId: string
        taskId: string | null
        verification: { hostname?: string | null; workingDir?: string; workingDirExists?: boolean | null }
      }>(`/tasks/${t.taskId}/continue`, {
        sessionId: continueSession.value,
        prompt: continuePrompt.value.trim() || undefined
      })
      const v = r.verification
      const cwdNote =
        v.workingDir === undefined
          ? ''
          : v.workingDirExists === true
            ? `，工作目录存在`
            : v.workingDirExists === false
              ? `，工作目录不存在`
              : `，工作目录未能核验`
      message.success(`已在新会话上继续（主机 ${v.hostname ?? '未知'}${cwdNote}）；旧 shell 未恢复`)
      showDetail.value = false
      sessionStore.activeTabId = r.sessionId
      uiState.updateLayout({ showAgent: true })
      router.push('/')
    } catch (e) {
      message.error(`继续失败: ${(e as Error).message}`)
    } finally {
      continuing.value = false
    }
  }

  const columns = computed<DataTableColumns<TaskRecord>>(() => [
    {
      title: '任务',
      key: 'title',
      ellipsis: { tooltip: true },
      render: (row) => h('span', { class: 'cursor-pointer hover:underline', onClick: () => openDetail(row) }, row.title)
    },
    {
      title: '服务器',
      key: 'serverAlias',
      width: 140,
      ellipsis: { tooltip: true },
      render: (row) => row.serverAlias || row.serverId || '本地'
    },
    {
      title: '状态',
      key: 'status',
      width: 90,
      render: (row) =>
        h(NTag, { size: 'small', type: taskStatusInfo[row.status].type }, () => taskStatusInfo[row.status].label)
    },
    { title: '轮次', key: 'turns', width: 60 },
    {
      title: 'tokens',
      key: 'tokens',
      width: 110,
      render: (row) => `${row.inputTokens} / ${row.outputTokens}`
    },
    { title: '开始', key: 'createdAt', width: 150, render: (row) => formatTime(row.createdAt) },
    { title: '结束', key: 'finishedAt', width: 150, render: (row) => formatTime(row.finishedAt) }
  ])
</script>

<template>
  <div class="h-full flex flex-col p-4">
    <div class="mb-2 flex items-center justify-between">
      <h2 class="text-lg font-bold">AI 任务</h2>
      <n-button size="small" :loading="loading" @click="load">刷新</n-button>
    </div>
    <n-space class="mb-2" size="small">
      <n-input v-model:value="query" placeholder="搜索标题 / 提示词" clearable size="small" style="width: 260px" />
      <n-select
        v-model:value="filterStatus"
        :options="statusOptions"
        placeholder="状态"
        clearable
        size="small"
        style="width: 120px"
      />
    </n-space>
    <div class="min-h-0 flex-1 overflow-auto">
      <n-data-table
        :columns="columns"
        :data="items"
        :loading="loading"
        :row-key="(r: TaskRecord) => r.taskId"
        size="small"
        :bordered="false"
      />
    </div>
    <div class="mt-2 flex justify-end">
      <n-pagination v-model:page="page" :page-size="pageSize" :item-count="total" size="small" />
    </div>

    <n-drawer v-model:show="showDetail" :width="680" placement="right">
      <n-drawer-content :title="detail?.task.title || '任务详情'" closable :native-scrollbar="false">
        <template v-if="detail">
          <n-descriptions :column="2" size="small" label-placement="left" bordered>
            <n-descriptions-item label="状态">
              <n-tag size="small" :type="taskStatusInfo[detail.task.status].type">
                {{ taskStatusInfo[detail.task.status].label }}
              </n-tag>
              <span v-if="detail.task.stopReason" class="ml-1 text-xs text-om-dimmed">{{
                detail.task.stopReason
              }}</span>
              <span v-if="detail.task.status === 'running' && !detail.live" class="ml-1 text-xs text-om-warning">
                （本进程中不存在，可能属于另一实例）
              </span>
            </n-descriptions-item>
            <n-descriptions-item label="服务器">{{
              detail.task.serverAlias || detail.task.serverId || '本地'
            }}</n-descriptions-item>
            <n-descriptions-item label="会话">
              <span class="text-xs font-mono">{{ detail.task.sessionId }}</span>
            </n-descriptions-item>
            <n-descriptions-item label="工作目录">
              <span class="text-xs font-mono">{{ detail.task.workingDir || '-' }}</span>
            </n-descriptions-item>
            <n-descriptions-item label="开始">{{ formatTime(detail.task.createdAt) }}</n-descriptions-item>
            <n-descriptions-item label="结束">{{ formatTime(detail.task.finishedAt) }}</n-descriptions-item>
            <n-descriptions-item label="用量">
              {{ detail.task.turns }} 轮 · {{ detail.task.inputTokens }} in / {{ detail.task.outputTokens }} out
              <span v-if="detail.task.costUsd != null"> · ${{ detail.task.costUsd.toFixed(4) }}</span>
            </n-descriptions-item>
            <n-descriptions-item label="继续自">
              <span class="text-xs font-mono">{{ detail.task.continuedFrom || '-' }}</span>
            </n-descriptions-item>
          </n-descriptions>

          <div class="mt-3 flex items-center gap-2">
            <n-button size="tiny" @click="exportTask('md')">导出 Markdown</n-button>
            <n-button size="tiny" @click="exportTask('json')">导出 JSON</n-button>
            <span v-if="detail.task.truncated" class="text-xs text-om-warning">记录已截断</span>
          </div>

          <!-- Continue -->
          <div v-if="canContinue" class="mt-3 border border-om-border rounded p-2">
            <div class="mb-1 text-xs text-om-dimmed">
              在同一服务器的打开会话上继续。这会启动一个新的 Agent 并附带之前的对话摘录；旧 shell
              的目录、环境与进程不会恢复，继续前会核验主机名与工作目录。
            </div>
            <div class="flex items-center gap-2">
              <n-select
                v-model:value="continueSession"
                :options="candidateSessions"
                size="small"
                placeholder="选择该服务器上已打开的会话"
                style="width: 260px"
              />
              <n-input v-model:value="continuePrompt" size="small" placeholder="现在要做什么（可选）" class="flex-1" />
              <n-button
                size="small"
                type="primary"
                :disabled="!continueSession"
                :loading="continuing"
                @click="continueTask"
              >
                继续
              </n-button>
            </div>
            <div v-if="candidateSessions.length === 0" class="mt-1 text-xs text-om-dimmed">
              没有打开的会话：请先在终端页连接 {{ detail.task.serverAlias || '该服务器' }}。
            </div>
          </div>

          <!-- Transcript -->
          <div class="mb-1 mt-4 text-xs text-om-dimmed">对话记录（{{ transcript.length }} 条）</div>
          <div class="max-h-[50vh] overflow-auto border border-om-border rounded p-2 text-xs">
            <div v-for="m in transcript" :key="m.seq" class="mb-2">
              <div class="mb-0.5 text-om-dimmed">
                <span
                  :class="
                    m.role === 'user'
                      ? 'text-om-primary'
                      : m.role === 'assistant'
                        ? 'text-om-success'
                        : m.role === 'error'
                          ? 'text-om-danger'
                          : ''
                  "
                >
                  {{
                    m.role === 'user'
                      ? '用户'
                      : m.role === 'assistant'
                        ? '助手'
                        : m.role === 'tool'
                          ? '工具请求'
                          : m.role === 'error'
                            ? '错误'
                            : '系统'
                  }}
                </span>
                · {{ formatTime(m.ts) }}
              </div>
              <div v-if="m.html" class="agent-markdown text-sm leading-6" v-html="m.html" />
              <pre v-else class="whitespace-pre-wrap break-all font-mono">{{ m.text }}</pre>
            </div>
            <div v-if="transcript.length === 0" class="text-om-dimmed">（无记录）</div>
          </div>

          <!-- Linked operations -->
          <div class="mb-1 mt-4 text-xs text-om-dimmed">关联审计操作（{{ operations.length }}）</div>
          <div v-if="operations.length === 0" class="text-xs text-om-dimmed">（无）</div>
          <div v-for="op in operations" :key="op.operationId" class="mb-1 flex items-center gap-2 text-xs">
            <n-tag size="tiny" :type="opStatusInfo[op.status]?.type || 'default'">
              {{ opStatusInfo[op.status]?.label || op.status }}
            </n-tag>
            <span class="text-om-dimmed">{{ formatTime(op.startedAt) }}</span>
            <span class="flex-1 truncate font-mono" :title="op.summary">{{ op.summary }}</span>
            <router-link
              :to="{ path: '/audit', query: { operationId: op.operationId } }"
              class="text-om-primary hover:underline"
            >
              审计
            </router-link>
          </div>
        </template>
        <div v-else-if="detailLoading" class="text-om-dimmed">加载中…</div>
      </n-drawer-content>
    </n-drawer>
  </div>
</template>
