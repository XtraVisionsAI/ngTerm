<script setup lang="ts">
  import type { DataTableColumns } from 'naive-ui'
  import type { AuditEvent, StartupReport } from '@/utils/audit'
  import { NAlert, NButton, NDataTable, NEmpty, NTag, useMessage } from 'naive-ui'
  import { h, onMounted, ref } from 'vue'
  import { useApi } from '@/composables/useApi'
  import { formatTime } from '@/utils/format'

  const api = useApi()
  const message = useMessage()

  const items = ref<AuditEvent[]>([])
  const current = ref<StartupReport | null>(null)
  const loading = ref(false)

  async function load() {
    loading.value = true
    try {
      const data = await api.get<{ items: AuditEvent[]; current: StartupReport }>('/audit/system?limit=100')
      items.value = data.items
      current.value = data.current
    } catch (e) {
      message.error((e as Error).message)
    } finally {
      loading.value = false
    }
  }
  onMounted(load)

  function num(ev: AuditEvent, key: string): number {
    const v = ev.payload[key]
    return typeof v === 'number' ? v : 0
  }

  const columns: DataTableColumns<AuditEvent> = [
    { title: '时间', key: 'occurredAt', width: 170, render: (r) => formatTime(r.occurredAt) },
    {
      title: '事件',
      key: 'eventType',
      width: 110,
      render: (r) =>
        r.eventType === 'system.startup'
          ? h(NTag, { size: 'small', type: 'info' }, { default: () => '启动' })
          : h(NTag, { size: 'small' }, { default: () => '正常关停' })
    },
    {
      title: '上次关停',
      key: 'previous',
      width: 110,
      render: (r) => {
        if (r.eventType !== 'system.startup') return '-'
        const v = r.payload.previousShutdownClean
        if (v === null || v === undefined) return h('span', { class: 'opacity-60' }, '首次启动')
        return v
          ? h(NTag, { size: 'small', type: 'success' }, { default: () => '正常' })
          : h(NTag, { size: 'small', type: 'error' }, { default: () => '异常退出' })
      }
    },
    {
      title: '恢复 / 关闭的记录',
      key: 'recovered',
      render: (r) => {
        if (r.eventType === 'system.startup') {
          const parts = [
            `${num(r, 'sessionsClosed')} 个会话标记为中断`,
            `${num(r, 'operationsInterrupted')} 个操作标记为中断`,
            `${num(r, 'recordingsInterrupted')} 段录像标记为中断`
          ]
          return parts.join('，')
        }
        return `请求关闭 ${num(r, 'sessionsRequested')} 个会话，${num(r, 'sessionsUnconfirmed')} 个未及时确认，补关 ${num(r, 'sessionsClosed')} 个会话、${num(r, 'operationsInterrupted')} 个操作`
      }
    },
    {
      title: '录制',
      key: 'recording',
      width: 80,
      render: (r) => {
        if (r.eventType !== 'system.startup') return '-'
        return r.payload.recordingEnabled ? '开启' : h('span', { class: 'text-amber-500' }, '关闭')
      }
    }
  ]
</script>

<template>
  <div class="h-full flex flex-col">
    <div class="mb-3 flex items-center justify-between">
      <p class="text-xs opacity-60">
        每次服务启动与正常关停都会记录；启动时若发现上一进程未正常关停，其遗留的会话、操作与录像会被标记为中断而不是完整。
      </p>
      <n-button size="small" :loading="loading" @click="load">刷新</n-button>
    </div>
    <n-alert
      v-if="current && current.previousShutdownClean === false"
      type="warning"
      class="mb-3"
      title="上一进程异常退出"
    >
      本次启动于 {{ formatTime(current.startedAt) }}，恢复时关闭了 {{ current.sessionsClosed }} 个会话、
      {{ current.operationsInterrupted }} 个操作、{{ current.recordingsInterrupted }}
      段录像。这些记录的结果不可信，已标记为中断。
    </n-alert>
    <n-alert v-if="current && !current.recordingEnabled" type="warning" class="mb-3">
      终端录制当前处于关闭状态，本次运行的会话不会有录像。
    </n-alert>
    <div v-if="!loading && items.length === 0" class="flex flex-1 items-center justify-center">
      <n-empty description="暂无系统事件" />
    </div>
    <n-data-table
      v-else
      :columns="columns"
      :data="items"
      :loading="loading"
      :bordered="false"
      :row-key="(r: AuditEvent) => r.eventId"
      flex-height
      class="min-h-0 flex-1"
    />
  </div>
</template>
