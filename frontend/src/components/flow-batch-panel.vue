<script setup lang="ts">
  /**
   * Batch run of a read-only flow across servers (UX-07): pick servers and a
   * concurrency cap, watch per-server status arrive, open any server's run
   * for its evidence, cancel what has not started. Polls while running.
   */
  import type { DataTableColumns } from 'naive-ui'
  import type { Batch, Flow, Run } from '@/utils/flows'
  import {
    NButton,
    NDataTable,
    NDrawer,
    NDrawerContent,
    NInputNumber,
    NSelect,
    NSpace,
    NTag,
    useMessage
  } from 'naive-ui'
  import { computed, h, onBeforeUnmount, ref, watch } from 'vue'
  import RunView from '@/components/flow-run-view.vue'
  import { useApi } from '@/composables/useApi'
  import { useServerStore } from '@/stores/server'
  import { batchStatusInfo, runStatusInfo } from '@/utils/flows'
  import { formatTime } from '@/utils/format'

  const props = defineProps<{ flow: Flow; params: Record<string, unknown> }>()
  const api = useApi()
  const message = useMessage()
  const serverStore = useServerStore()

  const selectedServers = ref<string[]>([])
  const concurrency = ref(4)
  const starting = ref(false)
  const batch = ref<Batch | null>(null)
  const runs = ref<Run[]>([])
  const openRun = ref<Run | null>(null)

  const serverOptions = computed(() =>
    serverStore.servers.map((s) => ({
      label: `${s.alias}${s.groupName ? ` · ${s.groupName}` : ''}`,
      value: s.id,
      disabled: !s.keyId
    }))
  )
  const groups = computed(() => Array.from(new Set(serverStore.servers.map((s) => s.groupName).filter(Boolean))))

  function selectGroup(g: string) {
    const ids = serverStore.servers.filter((s) => s.groupName === g && s.keyId).map((s) => s.id)
    selectedServers.value = Array.from(new Set([...selectedServers.value, ...ids]))
  }
  function selectAll() {
    selectedServers.value = serverStore.servers.filter((s) => s.keyId).map((s) => s.id)
  }

  let timer: ReturnType<typeof setTimeout> | null = null
  function stopPolling() {
    if (timer) clearTimeout(timer)
    timer = null
  }
  async function refresh() {
    if (!batch.value) return
    try {
      const r = await api.get<{ batch: Batch; runs: Run[] }>(`/flow-batches/${batch.value.batchId}`)
      batch.value = r.batch
      runs.value = r.runs
    } catch (e) {
      message.error((e as Error).message)
    }
    if (batch.value?.status === 'running') timer = setTimeout(refresh, 2000)
    else stopPolling()
  }
  onBeforeUnmount(stopPolling)
  watch(
    () => props.flow.flowId,
    () => {
      stopPolling()
      batch.value = null
      runs.value = []
    }
  )

  async function start() {
    if (!selectedServers.value.length) return
    starting.value = true
    try {
      batch.value = await api.post<Batch>(`/flows/${props.flow.flowId}/batch-runs`, {
        serverIds: selectedServers.value,
        params: props.params,
        concurrency: concurrency.value
      })
      runs.value = []
      refresh()
    } catch (e) {
      message.error(`启动失败: ${(e as Error).message}`)
    } finally {
      starting.value = false
    }
  }

  async function cancel() {
    if (!batch.value) return
    try {
      batch.value = await api.post<Batch>(`/flow-batches/${batch.value.batchId}/cancel`, {})
      message.info('已请求取消：未开始的服务器将跳过，正在执行的步骤序列会结束后停止')
    } catch (e) {
      message.error((e as Error).message)
    }
  }

  /** Servers selected but not yet reported (queued or running). */
  const pending = computed(() => {
    if (!batch.value) return []
    const done = new Set(runs.value.map((r) => r.serverId))
    return batch.value.serverIds.filter((id) => !done.has(id))
  })
  function aliasOf(id: string) {
    return serverStore.servers.find((s) => s.id === id)?.alias || id
  }

  const columns = computed<DataTableColumns<Run>>(() => [
    { title: '服务器', key: 'serverAlias', width: 160, render: (r) => r.serverAlias || r.serverId || '-' },
    {
      title: '状态',
      key: 'status',
      width: 130,
      render: (r) => h(NTag, { size: 'small', type: runStatusInfo[r.status].type }, () => runStatusInfo[r.status].label)
    },
    { title: '步骤', key: 'steps', width: 70, render: (r) => `${r.steps.length}/${r.definition.steps.length}` },
    {
      title: '说明',
      key: 'error',
      ellipsis: { tooltip: true },
      render: (r) => r.error || (r.status === 'finished' ? '全部步骤完成' : '')
    },
    { title: '结束', key: 'finishedAt', width: 150, render: (r) => formatTime(r.finishedAt) },
    {
      title: '',
      key: 'open',
      width: 70,
      render: (r) => h(NButton, { size: 'tiny', quaternary: true, onClick: () => (openRun.value = r) }, () => '详情')
    }
  ])
</script>

<template>
  <div class="border border-om-border rounded p-3 text-xs">
    <div class="mb-2 text-om-dimmed">
      批量只读执行：以你的密钥为每台服务器建立独立的 exec
      连接（不开终端、不录像），命令逐条进入审计；策略要求会话准入审批的服务器会被跳过并说明，批量不等待审批。
    </div>
    <template v-if="!batch">
      <div class="mb-2 flex items-center gap-2">
        <span class="w-20 text-om-dimmed">服务器</span>
        <n-select
          v-model:value="selectedServers"
          multiple
          filterable
          clearable
          size="small"
          :options="serverOptions"
          placeholder="选择服务器（未分配密钥的不可选）"
          :max-tag-count="6"
          class="flex-1"
        />
      </div>
      <div class="mb-2 flex flex-wrap items-center gap-1 pl-22">
        <n-button size="tiny" quaternary @click="selectAll">全选</n-button>
        <n-button v-for="g in groups" :key="g" size="tiny" quaternary @click="selectGroup(g)">+ {{ g }}</n-button>
      </div>
      <div class="mb-2 flex items-center gap-2">
        <span class="w-20 text-om-dimmed">并发上限</span>
        <n-input-number v-model:value="concurrency" size="small" :min="1" :max="8" style="width: 120px" />
        <span class="text-om-dimmed">同时最多 8 台</span>
      </div>
      <n-button type="primary" size="small" :disabled="!selectedServers.length" :loading="starting" @click="start">
        在 {{ selectedServers.length }} 台服务器上执行
      </n-button>
    </template>

    <template v-else>
      <div class="mb-2 flex flex-wrap items-center gap-2">
        <n-tag size="small" :type="batchStatusInfo[batch.status].type">{{ batchStatusInfo[batch.status].label }}</n-tag>
        <span>
          共 {{ batch.total }} 台 · 成功 {{ batch.succeeded }} · 失败 {{ batch.failed }} · 跳过 {{ batch.skipped }} ·
          待处理 {{ pending.length }} · 并发 {{ batch.concurrency }}
        </span>
        <span class="text-om-dimmed">{{ formatTime(batch.startedAt) }}</span>
        <n-space size="small" class="ml-auto">
          <n-button v-if="batch.status === 'running'" size="tiny" @click="cancel">取消未开始的</n-button>
          <n-button size="tiny" @click="refresh">刷新</n-button>
          <n-button v-if="batch.status !== 'running'" size="tiny" @click="batch = null">新的批量</n-button>
        </n-space>
      </div>
      <div v-if="pending.length" class="mb-2 text-om-dimmed">
        排队／执行中：{{ pending.slice(0, 12).map(aliasOf).join('、')
        }}<span v-if="pending.length > 12"> 等 {{ pending.length }} 台</span>
      </div>
      <n-data-table :columns="columns" :data="runs" size="small" :bordered="false" :row-key="(r: Run) => r.runId" />
    </template>

    <n-drawer :show="!!openRun" :width="720" placement="right" @update:show="(v: boolean) => !v && (openRun = null)">
      <n-drawer-content
        v-if="openRun"
        :title="`${openRun.serverAlias || openRun.serverId} · ${openRun.flowName} v${openRun.flowVersion}`"
        closable
        :native-scrollbar="false"
      >
        <run-view :run="openRun" />
      </n-drawer-content>
    </n-drawer>
  </div>
</template>
