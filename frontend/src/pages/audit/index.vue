<script setup lang="ts">
  import type { DataTableColumns } from 'naive-ui'
  import { NDataTable, NDatePicker, NEmpty, NPagination, NSelect, NSpace, NTag } from 'naive-ui'
  import { computed, h, onActivated, onMounted, ref, watch } from 'vue'
  import { useApi } from '@/composables/useApi'
  import { useAuthStore } from '@/stores/auth'
  import { formatDuration, formatTime } from '@/utils/format'

  interface AuditLog {
    id: string
    username: string
    serverAlias: string
    serverHost: string
    connectedAt: string
    disconnectedAt: string | null
    durationSecs: number | null
    disconnectReason: string
  }

  const api = useApi()
  const auth = useAuthStore()
  const logs = ref<AuditLog[]>([])
  const page = ref(1)
  const pageSize = ref(20)
  const total = ref(0)

  // Filter state
  const filterUser = ref<string | null>(null)
  const filterServer = ref<string | null>(null)
  const filterStatus = ref<string | null>(null)
  const filterTimeRange = ref<[number, number] | null>(null)

  // Filter options from backend
  const userOptions = ref<{ label: string; value: string }[]>([])
  const serverOptions = ref<{ label: string; value: string }[]>([])

  const statusOptions = [
    { label: '连接中', value: 'active' },
    { label: '主动断开', value: 'user_closed' },
    { label: '超时', value: 'timeout' },
    { label: '异常断开', value: 'error' }
  ]

  onMounted(async () => {
    await loadFilters()
    await loadLogs()
  })

  onActivated(loadLogs)

  watch([page, pageSize], loadLogs)
  watch([filterUser, filterServer, filterStatus, filterTimeRange], () => {
    page.value = 1
    loadLogs()
  })

  async function loadFilters() {
    try {
      const data = await api.get<{ usernames: string[]; servers: string[] }>('/audit/filters')
      userOptions.value = data.usernames.map((u) => ({ label: u, value: u }))
      serverOptions.value = data.servers.map((s) => ({ label: s, value: s }))
    } catch {}
  }

  async function loadLogs() {
    const offset = (page.value - 1) * pageSize.value
    const params = new URLSearchParams()
    params.set('limit', String(pageSize.value))
    params.set('offset', String(offset))
    if (filterUser.value) params.set('username', filterUser.value)
    if (filterServer.value) params.set('server', filterServer.value)
    if (filterStatus.value) params.set('status', filterStatus.value)
    if (filterTimeRange.value) {
      params.set('timeFrom', new Date(filterTimeRange.value[0]).toISOString())
      params.set('timeTo', new Date(filterTimeRange.value[1]).toISOString())
    }
    try {
      const data = await api.get<{ items: AuditLog[]; total: number }>(`/audit/logs?${params}`)
      logs.value = data.items
      total.value = data.total
    } catch {}
  }

  const reasonMap: Record<string, { label: string; type: 'success' | 'warning' | 'error' | 'info' }> = {
    active: { label: '连接中', type: 'success' },
    ssh_closed: { label: '正常断开', type: 'info' },
    user_closed: { label: '主动断开', type: 'info' },
    server_restart: { label: '服务重启', type: 'warning' },
    timeout: { label: '超时', type: 'warning' },
    error: { label: '异常断开', type: 'error' }
  }

  const columns = computed<DataTableColumns<AuditLog>>(() => {
    const cols: DataTableColumns<AuditLog> = []
    if (auth.isAdmin) {
      cols.push({ title: '用户', key: 'username', width: 80 })
    }
    cols.push(
      { title: '服务器', key: 'serverAlias' },
      { title: '主机', key: 'serverHost', width: 140 },
      {
        title: '连接时间',
        key: 'connectedAt',
        width: 180,
        render: (row) => formatTime(row.connectedAt)
      },
      {
        title: '断开时间',
        key: 'disconnectedAt',
        width: 180,
        render: (row) => formatTime(row.disconnectedAt)
      },
      {
        title: '时长',
        key: 'durationSecs',
        width: 100,
        render: (row) => formatDuration(row.durationSecs)
      },
      {
        title: '状态',
        key: 'disconnectReason',
        width: 100,
        render: (row) => {
          const info = reasonMap[row.disconnectReason] || { label: row.disconnectReason, type: 'info' as const }
          return h(NTag, { size: 'small', type: info.type }, { default: () => info.label })
        }
      }
    )
    return cols
  })
</script>

<template>
  <div class="h-full flex flex-col p-4">
    <div class="mb-3 flex items-center justify-between">
      <h2 class="text-lg font-bold">审计日志</h2>
      <div class="flex items-center gap-2">
        <n-select
          v-if="auth.isAdmin"
          v-model:value="filterUser"
          :options="userOptions"
          placeholder="用户"
          clearable
          size="small"
          class="w-28"
        />
        <n-select
          v-model:value="filterServer"
          :options="serverOptions"
          placeholder="服务器"
          clearable
          size="small"
          class="w-48"
        />
        <n-date-picker v-model:value="filterTimeRange" type="datetimerange" clearable size="small" class="w-72" />
        <n-select
          v-model:value="filterStatus"
          :options="statusOptions"
          placeholder="状态"
          clearable
          size="small"
          class="w-28"
        />
      </div>
    </div>
    <div v-if="logs.length === 0" class="flex flex-1 items-center justify-center">
      <n-empty description="暂无审计记录" />
    </div>
    <template v-else>
      <n-data-table :columns="columns" :data="logs" :bordered="false" flex-height class="min-h-0 flex-1" />
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
  </div>
</template>
