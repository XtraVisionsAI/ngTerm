<script setup lang="ts">
  import type { DataTableColumns } from 'naive-ui'
  import type { AuditEvent, RuntimeMetrics, StartupReport } from '@/utils/audit'
  import {
    NAlert,
    NButton,
    NDataTable,
    NDescriptions,
    NDescriptionsItem,
    NEmpty,
    NSpace,
    NTag,
    useMessage
  } from 'naive-ui'
  import { h, onMounted, ref } from 'vue'
  import { useApi } from '@/composables/useApi'
  import { formatBytes, warningLabel } from '@/utils/audit'
  import { formatTime } from '@/utils/format'

  const api = useApi()
  const message = useMessage()

  const items = ref<AuditEvent[]>([])
  const current = ref<StartupReport | null>(null)
  const loading = ref(false)

  interface IntegrityReport {
    checkedAt: string
    appendOnlyGuardsPresent: boolean
    streamsChecked: number
    streamGaps: { streamId: string; afterSeq: number; nextSeq: number }[]
    recordingsChecked: number
    recordingsWithProblems: { recordingId: string; sessionId: string; problems: unknown[] }[]
    limitation: string
  }
  const integrity = ref<IntegrityReport | null>(null)
  const checking = ref(false)

  async function runIntegrityCheck() {
    checking.value = true
    try {
      integrity.value = await api.get<IntegrityReport>('/audit/integrity?recordings=100')
    } catch (e) {
      message.error((e as Error).message)
    } finally {
      checking.value = false
    }
  }

  const metrics = ref<RuntimeMetrics | null>(null)
  const metricsError = ref<string | null>(null)

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
    // Metrics are shown separately: a failure here must not look like "all clear".
    try {
      metrics.value = await api.get<RuntimeMetrics>('/admin/metrics')
      metricsError.value = null
    } catch (e) {
      metrics.value = null
      metricsError.value = (e as Error).message
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
      <n-space size="small">
        <n-button size="small" :loading="checking" @click="runIntegrityCheck">完整性检查</n-button>
        <n-button size="small" :loading="loading" @click="load">刷新</n-button>
      </n-space>
    </div>
    <n-alert
      v-if="integrity"
      :type="
        !integrity.appendOnlyGuardsPresent || integrity.streamGaps.length || integrity.recordingsWithProblems.length
          ? 'error'
          : 'success'
      "
      class="mb-3"
      :title="`完整性检查 · ${formatTime(integrity.checkedAt)}`"
      closable
      @close="integrity = null"
    >
      <div class="text-xs">
        <p>追加写保护：{{ integrity.appendOnlyGuardsPresent ? '触发器在位' : '触发器缺失（数据库可能被直接改动）' }}</p>
        <p>
          事件流：检查 {{ integrity.streamsChecked }} 条，
          <template v-if="integrity.streamGaps.length">
            发现 {{ integrity.streamGaps.length }} 处序号缺口：
            {{ integrity.streamGaps.map((g) => `${g.streamId} #${g.afterSeq}→#${g.nextSeq}`).join('；') }}
          </template>
          <template v-else>序号连续</template>
        </p>
        <p>
          录像：校验最近 {{ integrity.recordingsChecked }} 段，
          <template v-if="integrity.recordingsWithProblems.length">
            {{ integrity.recordingsWithProblems.length }} 段有缺块或校验失败：
            {{ integrity.recordingsWithProblems.map((r) => r.recordingId).join('，') }}
          </template>
          <template v-else>全部通过</template>
        </p>
        <p class="mt-1 opacity-60">{{ integrity.limitation }}</p>
      </div>
    </n-alert>
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
    <n-alert v-if="metricsError" type="error" class="mb-3" title="运行指标不可用">
      {{ metricsError }}。指标读取失败不代表系统正常。
    </n-alert>
    <n-alert
      v-else-if="metrics"
      :type="metrics.warnings.length ? 'warning' : 'success'"
      class="mb-3"
      :title="`运行指标 · ${formatTime(metrics.takenAt)}`"
    >
      <p v-if="metrics.warnings.length" class="mb-2 text-xs">
        需要处理：{{ metrics.warnings.map((w) => warningLabel[w] || w).join('；') }}
      </p>
      <n-descriptions size="small" :column="3" label-placement="left" class="text-xs">
        <n-descriptions-item label="审计写入失败">
          {{ metrics.auditWriteFailures }}
          <span v-if="metrics.lastAuditWriteError" class="ml-1 opacity-60"
            >（最近 {{ formatTime(metrics.lastAuditWriteError.at) }}：{{ metrics.lastAuditWriteError.error }}）</span
          >
        </n-descriptions-item>
        <n-descriptions-item label="录像丢弃事件">
          {{ metrics.recording.eventsDropped }}
          <span class="ml-1 opacity-60">（队列容量 {{ metrics.recording.queueCapacity }}）</span>
        </n-descriptions-item>
        <n-descriptions-item label="磁盘余量">
          <template v-if="metrics.disk">
            {{ formatBytes(metrics.disk.freeBytes) }} / {{ formatBytes(metrics.disk.totalBytes) }}
            <span class="ml-1 opacity-60">（阈值 {{ formatBytes(metrics.disk.lowThresholdBytes) }}）</span>
          </template>
          <template v-else>未知</template>
        </n-descriptions-item>
        <n-descriptions-item label="运行中操作">
          {{ metrics.operations.running }}
          <span class="ml-1 opacity-60"
            >（超过 {{ Math.round(metrics.operations.staleAfterSecs / 60) }} 分钟：{{
              metrics.operations.stale
            }}）</span
          >
        </n-descriptions-item>
        <n-descriptions-item label="活跃会话">{{ metrics.activeSessions }}</n-descriptions-item>
        <n-descriptions-item label="活跃 Agent">{{ metrics.activeAgents }}</n-descriptions-item>
      </n-descriptions>
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
