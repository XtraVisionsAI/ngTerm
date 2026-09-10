<script setup lang="ts">
  import type { DataTableColumns } from 'naive-ui'
  import type { AuditSession, OperationRecord, RecordingMeta } from '@/utils/audit'
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
  import {
    actorKindLabel,
    formatBytes,
    formatMs,
    integrityInfo,
    reasonInfo,
    sourceLabel,
    statusInfo
  } from '@/utils/audit'
  import { downloadWithAuth } from '@/utils/download'
  import { formatTime } from '@/utils/format'

  const api = useApi()
  const auth = useAuthStore()
  const router = useRouter()
  const message = useMessage()

  const items = ref<AuditSession[]>([])
  const total = ref(0)
  const page = ref(1)
  const pageSize = ref(20)
  const loading = ref(false)

  const filterUsername = ref<string | null>(null)
  const filterRemoteUser = ref<string | null>(null)
  const filterSource = ref<string | null>(null)
  const filterActive = ref<'true' | 'false' | null>(null)
  const filterIntegrity = ref<string | null>(null)
  const filterTimeRange = ref<[number, number] | null>(null)

  const sourceOptions = Object.entries(sourceLabel).map(([value, label]) => ({ label, value }))
  const activeOptions = [
    { label: '连接中', value: 'true' },
    { label: '已结束', value: 'false' }
  ]
  const integrityOptions = [
    { label: '完整', value: 'complete' },
    { label: '有缺口', value: 'gap' },
    { label: '截断', value: 'truncated' }
  ]

  function buildParams(paged: boolean): URLSearchParams {
    const params = new URLSearchParams()
    if (paged) {
      params.set('limit', String(pageSize.value))
      params.set('offset', String((page.value - 1) * pageSize.value))
    }
    if (filterUsername.value) params.set('username', filterUsername.value)
    if (filterRemoteUser.value) params.set('remoteUser', filterRemoteUser.value)
    if (filterSource.value) params.set('source', filterSource.value)
    if (filterActive.value) params.set('active', filterActive.value)
    if (filterIntegrity.value) params.set('integrity', filterIntegrity.value)
    if (filterTimeRange.value) {
      params.set('timeFrom', new Date(filterTimeRange.value[0]).toISOString())
      params.set('timeTo', new Date(filterTimeRange.value[1]).toISOString())
    }
    return params
  }

  async function load() {
    loading.value = true
    try {
      const data = await api.get<{ items: AuditSession[]; total: number }>(`/audit/sessions?${buildParams(true)}`)
      items.value = data.items
      total.value = data.total
    } catch (e) {
      message.error((e as Error).message)
    } finally {
      loading.value = false
    }
  }

  onMounted(load)
  watch([page, pageSize], load)
  watch([filterUsername, filterRemoteUser, filterSource, filterActive, filterIntegrity, filterTimeRange], () => {
    page.value = 1
    load()
  })

  async function exportAs(format: 'json' | 'csv') {
    const params = buildParams(false)
    params.set('type', 'sessions')
    params.set('format', format)
    try {
      await downloadWithAuth(`/audit/export?${params}`, `audit-sessions.${format}`)
    } catch (e) {
      message.warning((e as Error).message)
    }
  }

  // --- Detail drawer ---
  const detailOpen = ref(false)
  const detail = ref<{ session: AuditSession; operations: OperationRecord[]; recordings: RecordingMeta[] } | null>(null)

  async function openDetail(row: AuditSession) {
    try {
      detail.value = await api.get(`/audit/sessions/${row.sessionId}`)
      detailOpen.value = true
    } catch (e) {
      message.error((e as Error).message)
    }
  }

  function play(rec: RecordingMeta) {
    router.push(`/audit/recordings/${rec.recordingId}`)
  }

  function integrityTag(row: { integrity: AuditSession['integrity'] }) {
    const info = integrityInfo(row.integrity)
    const tag = h(NTag, { size: 'small', type: info.type }, { default: () => info.label })
    return info.detail ? h(NTooltip, null, { trigger: () => tag, default: () => info.detail }) : tag
  }

  const columns = computed<DataTableColumns<AuditSession>>(() => {
    const cols: DataTableColumns<AuditSession> = []
    if (auth.isAdmin) {
      cols.push({
        title: '用户',
        key: 'username',
        width: 90,
        render: (r) => r.actor.username || r.actor.user_id || '-'
      })
    }
    cols.push(
      {
        title: '服务器',
        key: 'server',
        render: (r) => r.target.server_alias || r.target.server_id || '-'
      },
      { title: '远端账号', key: 'remoteUser', width: 110, render: (r) => r.target.remote_user || '-' },
      { title: '来源地址', key: 'remoteAddr', width: 140, render: (r) => r.actor.remote_addr || '-' },
      {
        title: '来源',
        key: 'source',
        width: 70,
        render: (r) => sourceLabel[r.source] || r.source
      },
      { title: '连接时间', key: 'connectedAt', width: 170, render: (r) => formatTime(r.connectedAt) },
      { title: '断开时间', key: 'disconnectedAt', width: 170, render: (r) => formatTime(r.disconnectedAt) },
      {
        title: '结束原因',
        key: 'reason',
        width: 110,
        render: (r) => {
          if (!r.disconnectedAt) return h(NTag, { size: 'small', type: 'success' }, { default: () => '连接中' })
          const info = reasonInfo[r.disconnectReason || ''] || { label: r.disconnectReason || '-', type: 'default' }
          return h(NTag, { size: 'small', type: info.type }, { default: () => info.label })
        }
      },
      { title: '录制', key: 'integrity', width: 100, render: (r) => integrityTag(r) },
      {
        title: '',
        key: 'actions',
        width: 80,
        render: (r) =>
          h(NButton, { size: 'tiny', quaternary: true, onClick: () => openDetail(r) }, { default: () => '详情' })
      }
    )
    return cols
  })

  const opColumns: DataTableColumns<OperationRecord> = [
    { title: '时间', key: 'startedAt', width: 160, render: (r) => formatTime(r.startedAt) },
    {
      title: '操作者',
      key: 'actor',
      width: 100,
      render: (r) => actorKindLabel[r.actor.kind || ''] || r.actor.kind || '-'
    },
    { title: '操作', key: 'summary', ellipsis: { tooltip: true } },
    {
      title: '状态',
      key: 'status',
      width: 90,
      render: (r) => {
        const info = statusInfo[r.status] || { label: r.status, type: 'default' as const }
        return h(NTag, { size: 'small', type: info.type }, { default: () => info.label })
      }
    }
  ]
</script>

<template>
  <div class="h-full flex flex-col">
    <div class="mb-3 flex flex-wrap items-center justify-between gap-2">
      <div class="flex flex-wrap items-center gap-2">
        <n-input
          v-if="auth.isAdmin"
          v-model:value="filterUsername"
          placeholder="用户名"
          clearable
          size="small"
          class="w-28"
        />
        <n-input v-model:value="filterRemoteUser" placeholder="远端账号" clearable size="small" class="w-28" />
        <n-select
          v-model:value="filterSource"
          :options="sourceOptions"
          placeholder="来源"
          clearable
          size="small"
          class="w-24"
        />
        <n-select
          v-model:value="filterActive"
          :options="activeOptions"
          placeholder="状态"
          clearable
          size="small"
          class="w-28"
        />
        <n-select
          v-model:value="filterIntegrity"
          :options="integrityOptions"
          placeholder="录制完整性"
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

    <div v-if="!loading && items.length === 0" class="flex flex-1 items-center justify-center">
      <n-empty description="暂无会话记录" />
    </div>
    <template v-else>
      <n-data-table
        :columns="columns"
        :data="items"
        :loading="loading"
        :bordered="false"
        :row-key="(r: AuditSession) => r.sessionId"
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

    <n-drawer v-model:show="detailOpen" :width="720" placement="right">
      <n-drawer-content v-if="detail" title="会话详情" closable>
        <n-descriptions :column="2" size="small" label-placement="left" bordered>
          <n-descriptions-item label="会话 ID">{{ detail.session.sessionId }}</n-descriptions-item>
          <n-descriptions-item label="用户">
            {{ detail.session.actor.username || detail.session.actor.user_id || '-' }}
          </n-descriptions-item>
          <n-descriptions-item label="服务器">
            {{ detail.session.target.server_alias || detail.session.target.server_id || '-' }}
            <span v-if="detail.session.target.server_host" class="opacity-60">
              ({{ detail.session.target.server_host }})
            </span>
          </n-descriptions-item>
          <n-descriptions-item label="远端账号">{{ detail.session.target.remote_user || '-' }}</n-descriptions-item>
          <n-descriptions-item label="来源地址">{{ detail.session.actor.remote_addr || '-' }}</n-descriptions-item>
          <n-descriptions-item label="来源">{{ sourceLabel[detail.session.source] }}</n-descriptions-item>
          <n-descriptions-item label="连接">{{ formatTime(detail.session.connectedAt) }}</n-descriptions-item>
          <n-descriptions-item label="断开">
            {{ formatTime(detail.session.disconnectedAt) }}
            <span v-if="detail.session.disconnectReason" class="opacity-60">
              ({{ reasonInfo[detail.session.disconnectReason]?.label || detail.session.disconnectReason }})
            </span>
          </n-descriptions-item>
        </n-descriptions>

        <h3 class="mb-2 mt-4 text-sm font-bold">录像</h3>
        <n-empty v-if="detail.recordings.length === 0" description="该会话没有录像" size="small" />
        <div v-for="rec in detail.recordings" :key="rec.recordingId" class="mb-2 flex items-center gap-3 text-sm">
          <n-tag size="small" :type="integrityInfo(rec.integrity).type">{{ integrityInfo(rec.integrity).label }}</n-tag>
          <span>{{ rec.status === 'recording' ? '录制中' : rec.status === 'interrupted' ? '已中断' : '已完成' }}</span>
          <span class="opacity-70">{{ rec.durationMs != null ? formatMs(rec.durationMs) : '-' }}</span>
          <span class="opacity-70">{{ rec.chunkCount }} 块 / {{ formatBytes(rec.totalBytes) }}</span>
          <span class="opacity-70"
            >输入：{{
              rec.inputPolicy === 'content' ? '含内容' : rec.inputPolicy === 'none' ? '未记录' : '仅元数据'
            }}</span
          >
          <n-button size="tiny" type="primary" :disabled="rec.chunkCount === 0" @click="play(rec)">播放</n-button>
        </div>
        <p v-if="detail.recordings.some((r) => integrityInfo(r.integrity).detail)" class="text-xs opacity-60">
          {{
            detail.recordings
              .map((r) => integrityInfo(r.integrity).detail)
              .filter(Boolean)
              .join('；')
          }}
        </p>

        <h3 class="mb-2 mt-4 text-sm font-bold">受管操作（{{ detail.operations.length }}）</h3>
        <n-empty
          v-if="detail.operations.length === 0"
          description="没有经平台执行通道登记的操作；人工终端的键入不按命令拆分"
          size="small"
        />
        <n-data-table
          v-else
          :columns="opColumns"
          :data="detail.operations"
          :bordered="false"
          size="small"
          :max-height="360"
          :row-key="(r: OperationRecord) => r.operationId"
        />
      </n-drawer-content>
    </n-drawer>
  </div>
</template>
