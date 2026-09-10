<script setup lang="ts">
  import type { DataTableColumns } from 'naive-ui'
  import type { AuditEvent, ConfigChangePayload, OperationRecord } from '@/utils/audit'
  import {
    NButton,
    NDataTable,
    NDatePicker,
    NDescriptions,
    NDescriptionsItem,
    NDrawer,
    NDrawerContent,
    NEmpty,
    NInput,
    NPagination,
    NSelect,
    NSpace,
    NTag,
    NTooltip,
    useMessage
  } from 'naive-ui'
  import { computed, h, onMounted, ref, watch } from 'vue'
  import { useRouter } from 'vue-router'
  import { useApi } from '@/composables/useApi'
  import { useAuthStore } from '@/stores/auth'
  import { actorKindLabel, evidenceLabel, exitLabel, kindLabel, sourceLabel, statusInfo } from '@/utils/audit'
  import { downloadWithAuth } from '@/utils/download'
  import { formatTime } from '@/utils/format'
  import ConfigChangeDetail from './config-change-detail.vue'

  const api = useApi()
  const auth = useAuthStore()
  const router = useRouter()
  const message = useMessage()

  const items = ref<OperationRecord[]>([])
  const total = ref(0)
  const page = ref(1)
  const pageSize = ref(20)
  const loading = ref(false)

  const search = ref('')
  const filterStatus = ref<string | null>(null)
  const filterKind = ref<string | null>(null)
  const filterActor = ref<string | null>(null)
  const filterTimeRange = ref<[number, number] | null>(null)

  const statusOptions = Object.entries(statusInfo).map(([value, info]) => ({ label: info.label, value }))
  const actorOptions = Object.entries(actorKindLabel).map(([value, label]) => ({ label, value }))
  const kindOptions = Object.entries(kindLabel).map(([value, label]) => ({ label, value }))

  function buildParams(paged: boolean): URLSearchParams {
    const params = new URLSearchParams()
    if (paged) {
      params.set('limit', String(pageSize.value))
      params.set('offset', String((page.value - 1) * pageSize.value))
    }
    if (search.value.trim()) params.set('q', search.value.trim())
    if (filterStatus.value) params.set('status', filterStatus.value)
    if (filterKind.value) params.set('kind', filterKind.value)
    if (filterActor.value) params.set('actorKind', filterActor.value)
    if (filterTimeRange.value) {
      params.set('timeFrom', new Date(filterTimeRange.value[0]).toISOString())
      params.set('timeTo', new Date(filterTimeRange.value[1]).toISOString())
    }
    return params
  }

  async function load() {
    loading.value = true
    try {
      const data = await api.get<{ items: OperationRecord[]; total: number }>(`/audit/operations?${buildParams(true)}`)
      items.value = data.items
      total.value = data.total
    } catch (e) {
      message.error((e as Error).message)
    } finally {
      loading.value = false
    }
  }

  let searchTimer: ReturnType<typeof setTimeout> | null = null
  watch(search, () => {
    if (searchTimer) clearTimeout(searchTimer)
    searchTimer = setTimeout(() => {
      page.value = 1
      load()
    }, 300)
  })

  onMounted(load)
  watch([page, pageSize], load)
  watch([filterStatus, filterKind, filterActor, filterTimeRange], () => {
    page.value = 1
    load()
  })

  async function exportAs(format: 'json' | 'csv') {
    const params = buildParams(false)
    params.set('type', 'operations')
    params.set('format', format)
    try {
      await downloadWithAuth(`/audit/export?${params}`, `audit-operations.${format}`)
    } catch (e) {
      message.warning((e as Error).message)
    }
  }

  async function locate(row: OperationRecord) {
    try {
      const detail = await api.get<{ recording: { recordingId: string; offsetMs: number } | null }>(
        `/audit/operations/${row.operationId}`
      )
      if (!detail.recording) {
        message.info('该操作没有关联的终端录像')
        return
      }
      router.push(`/audit/recordings/${detail.recording.recordingId}?t=${detail.recording.offsetMs}`)
    } catch (e) {
      message.error((e as Error).message)
    }
  }

  // --- Detail drawer ---
  interface OperationDetail {
    operation: OperationRecord
    events: AuditEvent[]
    recording: { recordingId: string; offsetMs: number } | null
    /** Lower-level operations caused by this one (the commands behind a tool call). */
    children: OperationRecord[]
  }
  const detailOpen = ref(false)
  const detail = ref<OperationDetail | null>(null)

  async function openDetail(row: OperationRecord) {
    try {
      const d = await api.get<OperationDetail>(`/audit/operations/${row.operationId}`)
      detail.value = { ...d, children: d.children ?? [] }
      detailOpen.value = true
    } catch (e) {
      message.error((e as Error).message)
    }
  }

  async function openDetailById(operationId: string) {
    await openDetail({ operationId } as OperationRecord)
  }

  function configChange(ev: AuditEvent): ConfigChangePayload | null {
    return ev.eventType === 'config.change' ? (ev.payload as unknown as ConfigChangePayload) : null
  }

  const columns = computed<DataTableColumns<OperationRecord>>(() => {
    const cols: DataTableColumns<OperationRecord> = [
      { title: '时间', key: 'startedAt', width: 165, render: (r) => formatTime(r.startedAt) }
    ]
    if (auth.isAdmin) {
      cols.push({ title: '用户', key: 'user', width: 90, render: (r) => r.actor.username || r.actor.user_id || '-' })
    }
    cols.push(
      {
        title: '操作者',
        key: 'actorKind',
        width: 100,
        render: (r) => {
          const label = actorKindLabel[r.actor.kind || ''] || r.actor.kind || '-'
          return r.actor.tool_id
            ? h(NTooltip, null, { trigger: () => h('span', label), default: () => `工具：${r.actor.tool_id}` })
            : label
        }
      },
      { title: '来源', key: 'source', width: 70, render: (r) => sourceLabel[r.source] || r.source },
      { title: '类型', key: 'kind', width: 90, render: (r) => kindLabel[r.kind] || r.kind },
      { title: '命令 / 操作', key: 'summary', ellipsis: { tooltip: true }, className: 'font-mono text-xs' },
      {
        title: '目标',
        key: 'target',
        width: 150,
        ellipsis: { tooltip: true },
        render: (r) => {
          const host = r.target.server_alias || r.target.server_id || '-'
          return r.target.remote_user ? `${r.target.remote_user}@${host}` : host
        }
      },
      {
        title: '状态',
        key: 'status',
        width: 90,
        render: (r) => {
          const info = statusInfo[r.status] || { label: r.status, type: 'default' as const }
          return h(NTag, { size: 'small', type: info.type }, { default: () => info.label })
        }
      },
      { title: '退出码', key: 'exit', width: 90, ellipsis: { tooltip: true }, render: (r) => exitLabel(r.exit) },
      {
        title: '结果依据',
        key: 'evidence',
        width: 100,
        render: (r) => evidenceLabel[r.evidence] || r.evidence
      },
      {
        title: '',
        key: 'actions',
        width: 140,
        render: (r) =>
          h(NSpace, { size: 4, wrap: false }, () => [
            h(NButton, { size: 'tiny', quaternary: true, onClick: () => openDetail(r) }, { default: () => '详情' }),
            h(
              NButton,
              { size: 'tiny', quaternary: true, disabled: !r.sessionId, onClick: () => locate(r) },
              { default: () => '定位录像' }
            )
          ])
      }
    )
    return cols
  })
</script>

<template>
  <div class="h-full flex flex-col">
    <div class="mb-3 flex flex-wrap items-center justify-between gap-2">
      <div class="flex flex-wrap items-center gap-2">
        <n-input v-model:value="search" placeholder="搜索命令（已脱敏）" clearable size="small" class="w-64" />
        <n-select
          v-model:value="filterStatus"
          :options="statusOptions"
          placeholder="状态"
          clearable
          size="small"
          class="w-28"
        />
        <n-select
          v-model:value="filterKind"
          :options="kindOptions"
          placeholder="类型"
          clearable
          size="small"
          class="w-32"
        />
        <n-select
          v-model:value="filterActor"
          :options="actorOptions"
          placeholder="操作者"
          clearable
          size="small"
          class="w-32"
        />
        <n-date-picker v-model:value="filterTimeRange" type="datetimerange" clearable size="small" class="w-72" />
      </div>
      <n-space size="small">
        <n-button size="small" @click="exportAs('csv')">导出 CSV</n-button>
        <n-button size="small" @click="exportAs('json')">导出 JSON</n-button>
      </n-space>
    </div>
    <p class="mb-2 text-xs opacity-60">
      包含经平台执行通道登记的操作（内置 Agent、受管命令、文件与 Git
      操作）以及用户、服务器、密钥、工具与配置的变更；人工终端键入不在此列，请查看会话录像。
    </p>

    <div v-if="!loading && items.length === 0" class="flex flex-1 items-center justify-center">
      <n-empty description="没有匹配的操作记录" />
    </div>
    <template v-else>
      <n-data-table
        :columns="columns"
        :data="items"
        :loading="loading"
        :bordered="false"
        :row-key="(r: OperationRecord) => r.operationId"
        flex-height
        class="min-h-0 flex-1"
      />
      <n-space justify="end" class="mt-3">
        <n-pagination
          v-model:page="page"
          v-model:page-size="pageSize"
          :item-count="total"
          :page-sizes="[20, 50, 100]"
          show-size-picker
          size="small"
        />
      </n-space>
    </template>

    <n-drawer v-model:show="detailOpen" :width="680" placement="right">
      <n-drawer-content v-if="detail" title="操作详情" closable>
        <n-descriptions :column="2" size="small" label-placement="left" bordered>
          <n-descriptions-item label="时间">{{ formatTime(detail.operation.startedAt) }}</n-descriptions-item>
          <n-descriptions-item label="结束">{{ formatTime(detail.operation.finishedAt) }}</n-descriptions-item>
          <n-descriptions-item label="操作者">
            {{ detail.operation.actor.username || detail.operation.actor.user_id || '-' }}
            <span class="opacity-60">({{ actorKindLabel[detail.operation.actor.kind || ''] || '-' }})</span>
          </n-descriptions-item>
          <n-descriptions-item label="类型">
            {{ kindLabel[detail.operation.kind] || detail.operation.kind }}
          </n-descriptions-item>
          <n-descriptions-item label="状态">
            <n-tag size="small" :type="statusInfo[detail.operation.status]?.type || 'default'">
              {{ statusInfo[detail.operation.status]?.label || detail.operation.status }}
            </n-tag>
          </n-descriptions-item>
          <n-descriptions-item label="结果依据">
            {{ evidenceLabel[detail.operation.evidence] || detail.operation.evidence }}
          </n-descriptions-item>
          <n-descriptions-item label="摘要" :span="2">
            <span class="break-all text-xs font-mono">{{ detail.operation.summary }}</span>
          </n-descriptions-item>
          <n-descriptions-item v-if="detail.operation.exit" label="退出码" :span="2">
            {{ exitLabel(detail.operation.exit) }}
          </n-descriptions-item>
          <n-descriptions-item v-if="detail.operation.parentOperationId" label="所属操作" :span="2">
            <n-button text size="tiny" type="primary" @click="openDetailById(detail.operation.parentOperationId!)">
              查看上层操作
            </n-button>
            <span class="ml-2 text-xs opacity-60">此记录是上层工具调用的底层调用，统计时不重复计数</span>
          </n-descriptions-item>
        </n-descriptions>

        <template v-if="detail.children.length > 0">
          <h3 class="mb-2 mt-4 text-sm font-bold">底层调用（{{ detail.children.length }}）</h3>
          <div v-for="c in detail.children" :key="c.operationId" class="mb-1 flex items-center gap-2 text-xs">
            <span class="opacity-60">{{ formatTime(c.startedAt) }}</span>
            <n-tag size="tiny" :type="statusInfo[c.status]?.type || 'default'">
              {{ statusInfo[c.status]?.label || c.status }}
            </n-tag>
            <span>{{ kindLabel[c.kind] || c.kind }}</span>
            <n-button text size="tiny" type="primary" @click="openDetail(c)">
              <span class="break-all font-mono">{{ c.summary }}</span>
            </n-button>
          </div>
        </template>

        <template v-for="ev in detail.events" :key="ev.eventId">
          <template v-if="configChange(ev)">
            <h3 class="mb-2 mt-4 text-sm font-bold">配置变更内容</h3>
            <config-change-detail :payload="configChange(ev)!" />
          </template>
        </template>

        <template v-if="detail.events.some((e) => !configChange(e))">
          <h3 class="mb-2 mt-4 text-sm font-bold"
            >事件（{{ detail.events.filter((e) => !configChange(e)).length }}）</h3
          >
          <div v-for="ev in detail.events.filter((e) => !configChange(e))" :key="ev.eventId" class="mb-1 text-xs">
            <span class="opacity-60">{{ formatTime(ev.occurredAt) }}</span>
            <span class="ml-2 font-mono">{{ ev.eventType }}</span>
          </div>
        </template>
        <n-empty v-if="detail.events.length === 0" description="该操作没有附加事件" size="small" class="mt-4" />
      </n-drawer-content>
    </n-drawer>
  </div>
</template>
