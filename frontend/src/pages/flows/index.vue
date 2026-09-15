<script setup lang="ts">
  /**
   * Operations flows (UX-04): pick a flow, fill its parameters, run it on an
   * open session step by step. Pre-checks gate the run; steps marked as
   * approval points wait for a second person; verification is shown as the
   * evidence of each step. Admins edit definitions as JSON (version + 1).
   */
  import type { DataTableColumns } from 'naive-ui'
  import type { Batch, Flow, Run } from '@/utils/flows'
  import {
    NButton,
    NCheckbox,
    NDataTable,
    NDrawer,
    NDrawerContent,
    NInput,
    NInputNumber,
    NModal,
    NPopconfirm,
    NSelect,
    NSpace,
    NTag,
    NTooltip,
    useMessage
  } from 'naive-ui'
  import { computed, h, onMounted, ref, watch } from 'vue'
  import BatchPanel from '@/components/flow-batch-panel.vue'
  import RunView from '@/components/flow-run-view.vue'
  import JsonEditor from '@/components/json-editor.vue'
  import LoadState from '@/components/load-state.vue'
  import { useApi } from '@/composables/useApi'
  import { postWithAdmission } from '@/composables/useSessionAdmission'
  import { useAuthStore } from '@/stores/auth'
  import { useSessionStore } from '@/stores/session'
  import { batchEligible, batchStatusInfo, defaultParams, runStatusInfo } from '@/utils/flows'
  import { formatTime } from '@/utils/format'

  const api = useApi()
  const message = useMessage()
  const auth = useAuthStore()
  const sessionStore = useSessionStore()

  // --- flows ---
  const flows = ref<Flow[]>([])
  const loadingFlows = ref(false)
  const flowsError = ref<string | null>(null)
  const selected = ref<Flow | null>(null)

  async function loadFlows() {
    loadingFlows.value = true
    flowsError.value = null
    try {
      const r = await api.get<{ items: Flow[] }>('/flows')
      flows.value = r.items
      if (selected.value) selected.value = r.items.find((f) => f.flowId === selected.value!.flowId) || null
    } catch (e) {
      flowsError.value = (e as Error).message
    } finally {
      loadingFlows.value = false
    }
  }

  // --- run setup ---
  const run = ref<Run | null>(null)
  const sessionId = ref<string | null>(null)
  const params = ref<Record<string, unknown>>({})
  const sessionOptions = computed(() =>
    sessionStore.tabs
      .filter((t) => !t.id.startsWith('pending-'))
      .map((t) => ({ label: `${t.serverAlias} · ${t.id.slice(0, 8)}`, value: t.id }))
  )

  /** Single session or batch across servers (read-only flows only). */
  const mode = ref<'single' | 'batch'>('single')

  function selectFlow(f: Flow) {
    selected.value = f
    params.value = defaultParams(f.definition)
    run.value = null
    if (!batchEligible(f.definition)) mode.value = 'single'
  }

  // --- run execution ---
  const busy = ref(false)
  const waiting = ref<{ requestId: string; message: string } | null>(null)
  const cancelWait = ref(false)

  async function startRun() {
    if (!selected.value || !sessionId.value) return
    busy.value = true
    waiting.value = null
    cancelWait.value = false
    try {
      run.value = await postWithAdmission<Run>(
        api,
        `/flows/${selected.value.flowId}/runs`,
        { sessionId: sessionId.value, params: params.value },
        (requestId, msg) => (waiting.value = { requestId, message: msg }),
        () => cancelWait.value
      )
      if (run.value.status === 'precheck_failed') message.warning(run.value.error || '前置检查未通过')
    } catch (e) {
      message.error(`启动失败: ${(e as Error).message}`)
    } finally {
      busy.value = false
      waiting.value = null
    }
  }

  async function nextStep() {
    if (!run.value) return
    busy.value = true
    waiting.value = null
    cancelWait.value = false
    try {
      run.value = await postWithAdmission<Run>(
        api,
        `/flow-runs/${run.value.runId}/next`,
        {},
        (requestId, msg) => (waiting.value = { requestId, message: msg }),
        () => cancelWait.value
      )
      if (run.value.status === 'failed') message.error(run.value.error || '步骤失败')
      if (run.value.status === 'finished') message.success('流程已完成')
    } catch (e) {
      message.error(`执行失败: ${(e as Error).message}`)
    } finally {
      busy.value = false
      waiting.value = null
    }
  }

  async function abortRun() {
    if (!run.value) return
    try {
      run.value = await api.post<Run>(`/flow-runs/${run.value.runId}/abort`, {})
    } catch (e) {
      message.error((e as Error).message)
    }
  }

  const nextStepDef = computed(() => {
    const r = run.value
    if (!r || r.status !== 'ready') return null
    return r.definition.steps[r.currentStep] ?? null
  })

  // --- history ---
  const runs = ref<Run[]>([])
  const batches = ref<Batch[]>([])
  const loadingRuns = ref(false)
  const showHistory = ref(false)
  const historyRun = ref<Run | null>(null)
  const historyBatch = ref<{ batch: Batch; runs: Run[] } | null>(null)

  async function openBatch(b: Batch) {
    try {
      historyBatch.value = await api.get<{ batch: Batch; runs: Run[] }>(`/flow-batches/${b.batchId}`)
    } catch (e) {
      message.error((e as Error).message)
    }
  }

  async function loadRuns() {
    loadingRuns.value = true
    try {
      const q = selected.value ? `?flowId=${selected.value.flowId}&limit=50` : '?limit=50'
      const [r, b] = await Promise.all([
        api.get<{ items: Run[] }>(`/flow-runs${q}`),
        api.get<{ items: Batch[] }>('/flow-batches?limit=50')
      ])
      runs.value = r.items
      batches.value = selected.value ? b.items.filter((x) => x.flowId === selected.value!.flowId) : b.items
    } catch (e) {
      message.error((e as Error).message)
    } finally {
      loadingRuns.value = false
    }
  }
  watch(selected, () => {
    if (showHistory.value) loadRuns()
  })
  watch(showHistory, (v) => {
    if (v) loadRuns()
  })

  const runColumns = computed<DataTableColumns<Run>>(() => [
    { title: '流程', key: 'flowName', width: 140, render: (r) => `${r.flowName} v${r.flowVersion}` },
    { title: '服务器', key: 'serverAlias', width: 120, render: (r) => r.serverAlias || r.serverId || '本地' },
    {
      title: '状态',
      key: 'status',
      width: 120,
      render: (r) => h(NTag, { size: 'small', type: runStatusInfo[r.status].type }, () => runStatusInfo[r.status].label)
    },
    { title: '步骤', key: 'steps', width: 70, render: (r) => `${r.steps.length}/${r.definition.steps.length}` },
    { title: '开始', key: 'startedAt', width: 150, render: (r) => formatTime(r.startedAt) },
    {
      title: '',
      key: 'open',
      width: 70,
      render: (r) => h(NButton, { size: 'tiny', quaternary: true, onClick: () => (historyRun.value = r) }, () => '详情')
    }
  ])

  const batchColumns = computed<DataTableColumns<Batch>>(() => [
    { title: '流程', key: 'flowName', width: 140, render: (b) => `${b.flowName} v${b.flowVersion}` },
    {
      title: '状态',
      key: 'status',
      width: 100,
      render: (b) =>
        h(NTag, { size: 'small', type: batchStatusInfo[b.status].type }, () => batchStatusInfo[b.status].label)
    },
    {
      title: '结果',
      key: 'counts',
      render: (b) => `${b.total} 台 · 成功 ${b.succeeded} · 失败 ${b.failed} · 跳过 ${b.skipped}`
    },
    { title: '开始', key: 'startedAt', width: 150, render: (b) => formatTime(b.startedAt) },
    {
      title: '',
      key: 'open',
      width: 70,
      render: (b) => h(NButton, { size: 'tiny', quaternary: true, onClick: () => openBatch(b) }, () => '详情')
    }
  ])

  // --- admin editor ---
  const showEditor = ref(false)
  const editing = ref<Flow | null>(null)
  const editorName = ref('')
  const editorDesc = ref('')
  const editorJson = ref('{}')
  const saving = ref(false)

  function openEditor(f: Flow | null) {
    editing.value = f
    editorName.value = f?.name || ''
    editorDesc.value = f?.description || ''
    editorJson.value = JSON.stringify(
      f?.definition || {
        params: [
          {
            key: 'target',
            label: '目标',
            type: 'string',
            required: true,
            pattern: '[A-Za-z0-9_.-]+',
            options: [],
            description: ''
          }
        ],
        preChecks: [{ name: '检查', command: 'true', expect: {}, failMessage: '' }],
        steps: [
          {
            name: '步骤 1',
            command: 'echo {{target}}',
            approval: 'none',
            timeoutSecs: 60,
            failOnError: true,
            verify: null
          }
        ]
      },
      null,
      2
    )
    showEditor.value = true
  }

  async function saveFlow() {
    let def: unknown
    try {
      def = JSON.parse(editorJson.value)
    } catch {
      return message.error('定义不是合法 JSON')
    }
    saving.value = true
    try {
      if (editing.value) {
        await api.put(`/flows/${editing.value.flowId}`, { description: editorDesc.value, definition: def })
        message.success('已保存，版本 +1')
      } else {
        await api.post('/flows', { name: editorName.value.trim(), description: editorDesc.value, definition: def })
        message.success('已创建')
      }
      showEditor.value = false
      await loadFlows()
    } catch (e) {
      const err = e as Error & { errors?: string[] }
      message.error(err.message)
    } finally {
      saving.value = false
    }
  }

  async function deleteFlow(f: Flow) {
    try {
      await api.del(`/flows/${f.flowId}`)
      if (selected.value?.flowId === f.flowId) selected.value = null
      await loadFlows()
    } catch (e) {
      message.error((e as Error).message)
    }
  }

  onMounted(loadFlows)
</script>

<template>
  <div class="h-full flex flex-col p-4">
    <div class="mb-2 flex items-center justify-between">
      <h2 class="text-lg font-bold">运维流程</h2>
      <n-space size="small">
        <n-button size="small" @click="showHistory = !showHistory">{{
          showHistory ? '隐藏历史' : '运行历史'
        }}</n-button>
        <n-button v-if="auth.isAdmin" size="small" @click="openEditor(null)">新建流程</n-button>
        <n-button size="small" :loading="loadingFlows" @click="loadFlows">刷新</n-button>
      </n-space>
    </div>

    <div class="min-h-0 flex flex-1 gap-3 overflow-hidden">
      <!-- Flow list -->
      <div class="w-72 flex shrink-0 flex-col overflow-auto border border-om-border rounded">
        <load-state
          :loading="loadingFlows"
          :error="flowsError"
          :empty="flows.length === 0"
          empty-text="没有流程"
          size="small"
          @retry="loadFlows"
        >
          <div
            v-for="f in flows"
            :key="f.flowId"
            class="cursor-pointer border-b border-om-border px-3 py-2 hover:bg-om-hover"
            :class="selected?.flowId === f.flowId ? 'bg-om-hover' : ''"
            @click="selectFlow(f)"
          >
            <div class="flex items-center gap-2">
              <span class="text-sm font-mono">{{ f.name }}</span>
              <n-tag size="tiny" :bordered="false">v{{ f.version }}</n-tag>
              <n-tag v-if="f.builtin" size="tiny" type="info" :bordered="false">内置</n-tag>
            </div>
            <div class="line-clamp-2 mt-0.5 text-xs text-om-dimmed">{{ f.description }}</div>
            <div class="mt-1 text-[10px] text-om-dimmed">
              {{ f.definition.params.length }} 参数 · {{ f.definition.preChecks.length }} 前置检查 ·
              {{ f.definition.steps.length }} 步骤
              <span v-if="f.definition.steps.some((s) => s.approval === 'required')">· 含审批点</span>
            </div>
          </div>
        </load-state>
      </div>

      <!-- Run panel -->
      <div class="min-w-0 flex-1 overflow-auto">
        <div v-if="!selected" class="p-6 text-sm text-om-dimmed">选择左侧流程开始</div>
        <template v-else>
          <div class="mb-2 flex items-start justify-between gap-2">
            <div>
              <div class="flex items-center gap-2">
                <span class="text-base font-semibold">{{ selected.name }}</span>
                <n-tag size="tiny" :bordered="false">v{{ selected.version }}</n-tag>
              </div>
              <div class="text-xs text-om-dimmed">{{ selected.description }}</div>
            </div>
            <n-space v-if="auth.isAdmin" size="small">
              <n-button size="tiny" @click="openEditor(selected)">编辑定义</n-button>
              <n-popconfirm v-if="!selected.builtin" @positive-click="deleteFlow(selected)">
                <template #trigger><n-button size="tiny" type="error" quaternary>删除</n-button></template>
                删除流程定义？已有运行记录会保留。
              </n-popconfirm>
            </n-space>
          </div>

          <!-- Steps overview -->
          <div class="mb-3 border border-om-border rounded p-2 text-xs">
            <div class="mb-1 text-om-dimmed">步骤</div>
            <ol class="list-decimal pl-5">
              <li v-for="(s, i) in selected.definition.steps" :key="i" class="mb-0.5">
                <span>{{ s.name }}</span>
                <n-tag v-if="s.approval === 'required'" size="tiny" type="warning" :bordered="false" class="ml-1"
                  >审批点</n-tag
                >
                <n-tag v-if="s.verify" size="tiny" type="info" :bordered="false" class="ml-1">含验证</n-tag>
                <code class="ml-2 text-om-dimmed">{{ s.command }}</code>
              </li>
            </ol>
            <div v-if="selected.definition.preChecks.length" class="mt-1 text-om-dimmed">
              前置检查：{{ selected.definition.preChecks.map((c) => c.name).join('、') }}
            </div>
          </div>

          <div v-if="!run && batchEligible(selected.definition)" class="mb-2 flex items-center gap-2 text-xs">
            <n-tag size="tiny" type="success" :bordered="false">只读流程</n-tag>
            <n-button size="tiny" :type="mode === 'single' ? 'primary' : 'default'" quaternary @click="mode = 'single'"
              >单台会话</n-button
            >
            <n-button size="tiny" :type="mode === 'batch' ? 'primary' : 'default'" quaternary @click="mode = 'batch'"
              >批量执行</n-button
            >
          </div>

          <!-- Batch -->
          <template v-if="!run && mode === 'batch'">
            <div class="mb-2 border border-om-border rounded p-3">
              <div v-for="p in selected.definition.params" :key="p.key" class="mb-2 flex items-center gap-2">
                <span class="w-24 truncate text-xs text-om-dimmed"
                  >{{ p.label || p.key }}<span v-if="p.required" class="text-om-danger">*</span></span
                >
                <n-input-number
                  v-if="p.type === 'int'"
                  :value="(params[p.key] as number | null) ?? null"
                  size="small"
                  style="width: 200px"
                  @update:value="(v: number | null) => (params[p.key] = v)"
                />
                <n-checkbox
                  v-else-if="p.type === 'bool'"
                  :checked="!!params[p.key]"
                  size="small"
                  @update:checked="(v: boolean) => (params[p.key] = v)"
                />
                <n-select
                  v-else-if="p.type === 'enum'"
                  :value="(params[p.key] as string | null) ?? null"
                  :options="p.options.map((o) => ({ label: o, value: o }))"
                  size="small"
                  style="width: 240px"
                  @update:value="(v: string | null) => (params[p.key] = v)"
                />
                <n-input
                  v-else
                  :value="(params[p.key] as string | null) ?? ''"
                  size="small"
                  style="width: 320px"
                  @update:value="(v: string) => (params[p.key] = v)"
                />
              </div>
              <div v-if="!selected.definition.params.length" class="text-xs text-om-dimmed">此流程没有参数</div>
            </div>
            <batch-panel :flow="selected" :params="params" />
          </template>

          <!-- Setup -->
          <div v-else-if="!run" class="border border-om-border rounded p-3">
            <div class="mb-2 flex items-center gap-2">
              <span class="w-24 text-xs text-om-dimmed">目标会话</span>
              <n-select
                v-model:value="sessionId"
                :options="sessionOptions"
                size="small"
                placeholder="选择已打开的会话（命令在其独立 exec 通道执行，不进入终端）"
                style="max-width: 420px"
              />
            </div>
            <div v-for="p in selected.definition.params" :key="p.key" class="mb-2 flex items-center gap-2">
              <n-tooltip :disabled="!p.description">
                <template #trigger>
                  <span class="w-24 truncate text-xs text-om-dimmed"
                    >{{ p.label || p.key }}<span v-if="p.required" class="text-om-danger">*</span></span
                  >
                </template>
                {{ p.description }}
              </n-tooltip>
              <n-input-number
                v-if="p.type === 'int'"
                :value="(params[p.key] as number | null) ?? null"
                size="small"
                style="width: 200px"
                @update:value="(v: number | null) => (params[p.key] = v)"
              />
              <n-checkbox
                v-else-if="p.type === 'bool'"
                :checked="!!params[p.key]"
                size="small"
                @update:checked="(v: boolean) => (params[p.key] = v)"
              />
              <n-select
                v-else-if="p.type === 'enum'"
                :value="(params[p.key] as string | null) ?? null"
                :options="p.options.map((o) => ({ label: o, value: o }))"
                size="small"
                style="width: 240px"
                @update:value="(v: string | null) => (params[p.key] = v)"
              />
              <n-input
                v-else
                :value="(params[p.key] as string | null) ?? ''"
                size="small"
                style="width: 320px"
                :placeholder="p.pattern ? `匹配 ${p.pattern}` : ''"
                @update:value="(v: string) => (params[p.key] = v)"
              />
            </div>
            <div class="mt-3 flex items-center gap-2">
              <n-button type="primary" size="small" :disabled="!sessionId" :loading="busy" @click="startRun">
                运行前置检查并开始
              </n-button>
              <span v-if="waiting" class="text-xs text-om-warning">等待审批：{{ waiting.message }}</span>
              <n-button v-if="waiting" size="tiny" quaternary @click="cancelWait = true">取消等待</n-button>
            </div>
          </div>

          <!-- Run progress -->
          <run-view v-else-if="run" :run="run">
            <template #actions>
              <n-space size="small" align="center">
                <template v-if="run.status === 'ready' && nextStepDef">
                  <n-button type="primary" size="small" :loading="busy" @click="nextStep">
                    执行第 {{ run.currentStep + 1 }} 步：{{ nextStepDef.name }}
                    <span v-if="nextStepDef.approval === 'required'" class="ml-1 opacity-80">（需要审批）</span>
                  </n-button>
                  <n-button size="small" :disabled="busy" @click="abortRun">中止</n-button>
                </template>
                <n-button v-else size="small" @click="run = null">返回</n-button>
                <span v-if="waiting" class="text-xs text-om-warning">等待第二人审批：{{ waiting.message }}</span>
                <n-button v-if="waiting" size="tiny" quaternary @click="cancelWait = true">取消等待</n-button>
              </n-space>
            </template>
          </run-view>
        </template>
      </div>

      <!-- History -->
      <div v-if="showHistory" class="w-[520px] shrink-0 overflow-auto border border-om-border rounded p-2">
        <div class="mb-1 text-xs text-om-dimmed">运行历史{{ selected ? `（${selected.name}）` : '' }}</div>
        <n-data-table
          :columns="runColumns"
          :data="runs"
          :loading="loadingRuns"
          size="small"
          :bordered="false"
          :row-key="(r: Run) => r.runId"
        />
        <div class="mb-1 mt-3 text-xs text-om-dimmed">批量执行历史</div>
        <n-data-table
          :columns="batchColumns"
          :data="batches"
          :loading="loadingRuns"
          size="small"
          :bordered="false"
          :row-key="(b: Batch) => b.batchId"
        />
      </div>
    </div>

    <n-drawer
      :show="!!historyBatch"
      :width="760"
      placement="right"
      @update:show="(v: boolean) => !v && (historyBatch = null)"
    >
      <n-drawer-content
        v-if="historyBatch"
        :title="`批量 · ${historyBatch.batch.flowName} v${historyBatch.batch.flowVersion} · ${historyBatch.batch.total} 台`"
        closable
        :native-scrollbar="false"
      >
        <div class="mb-2 text-xs">
          <n-tag size="small" :type="batchStatusInfo[historyBatch.batch.status].type">{{
            batchStatusInfo[historyBatch.batch.status].label
          }}</n-tag>
          <span class="ml-2"
            >成功 {{ historyBatch.batch.succeeded }} · 失败 {{ historyBatch.batch.failed }} · 跳过
            {{ historyBatch.batch.skipped }}</span
          >
          <span v-if="Object.keys(historyBatch.batch.params).length" class="ml-2 text-om-dimmed font-mono">
            {{
              Object.entries(historyBatch.batch.params)
                .map(([k, v]) => `${k}=${v}`)
                .join(' ')
            }}
          </span>
        </div>
        <div v-for="r in historyBatch.runs" :key="r.runId" class="mb-3">
          <div class="mb-1 text-sm font-semibold">{{ r.serverAlias || r.serverId }}</div>
          <run-view :run="r" />
        </div>
      </n-drawer-content>
    </n-drawer>

    <n-drawer
      :show="!!historyRun"
      :width="720"
      placement="right"
      @update:show="(v: boolean) => !v && (historyRun = null)"
    >
      <n-drawer-content
        v-if="historyRun"
        :title="`${historyRun.flowName} v${historyRun.flowVersion}`"
        closable
        :native-scrollbar="false"
      >
        <run-view :run="historyRun" />
      </n-drawer-content>
    </n-drawer>

    <n-modal
      :show="showEditor"
      preset="card"
      :title="editing ? `编辑 ${editing.name}（保存后版本 +1）` : '新建流程'"
      style="width: min(900px, 94vw)"
      @update:show="(v: boolean) => (showEditor = v)"
    >
      <div class="mb-2 flex gap-2">
        <n-input
          v-if="!editing"
          v-model:value="editorName"
          size="small"
          placeholder="名称（字母、数字、- _）"
          style="width: 240px"
        />
        <n-input v-model:value="editorDesc" size="small" placeholder="描述" class="flex-1" />
      </div>
      <div class="mb-1 text-xs text-om-dimmed">
        定义 JSON：params[{key,label,type:
        string|int|bool|enum,required,default,pattern,options,description}]，preChecks/steps[{name,command,expect:{exitCode,contains},failMessage}]，steps
        另有 approval: none|required、timeoutSecs、failOnError、verify。命令中以
        <code v-pre>{{ key }}</code> 引用参数，值在渲染时整体做 shell 转义。
      </div>
      <json-editor v-model="editorJson" :rows="18" />
      <template #footer>
        <n-space justify="end">
          <n-button size="small" @click="showEditor = false">取消</n-button>
          <n-button size="small" type="primary" :loading="saving" @click="saveFlow">保存</n-button>
        </n-space>
      </template>
    </n-modal>
  </div>
</template>
