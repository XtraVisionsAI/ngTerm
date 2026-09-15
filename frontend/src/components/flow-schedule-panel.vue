<script setup lang="ts">
  /**
   * Scheduled runs of a read-only flow (UX-08): create a recurring batch on
   * chosen servers, list your schedules for this flow, toggle / run-now /
   * delete. A fire uses the owner's live login to decrypt keys; if the owner
   * is not logged in at fire time the occurrence is skipped and a notification
   * is left. Bumping the flow version pauses a schedule until reviewed.
   */
  import type { Flow, NotifyOn, Schedule } from '@/utils/flows'
  import { NButton, NInput, NInputNumber, NPopconfirm, NSelect, NSpace, NSwitch, NTag, useMessage } from 'naive-ui'
  import { computed, onMounted, ref, watch } from 'vue'
  import { useApi } from '@/composables/useApi'
  import { useServerStore } from '@/stores/server'
  import { batchStatusInfo, intervalLabel, intervalOptions, notifyOnLabel } from '@/utils/flows'
  import { formatTime } from '@/utils/format'

  const props = defineProps<{ flow: Flow; params: Record<string, unknown> }>()
  const api = useApi()
  const message = useMessage()
  const serverStore = useServerStore()

  const schedules = ref<Schedule[]>([])
  const loading = ref(false)

  // create form
  const name = ref('')
  const selectedServers = ref<string[]>([])
  const concurrency = ref(4)
  const intervalSecs = ref(3600)
  const notifyOn = ref<NotifyOn>('failure')
  const creating = ref(false)

  const serverOptions = computed(() =>
    serverStore.servers.map((s) => ({
      label: `${s.alias}${s.groupName ? ` · ${s.groupName}` : ''}`,
      value: s.id,
      disabled: !s.keyId
    }))
  )
  const notifyOptions = (Object.keys(notifyOnLabel) as NotifyOn[]).map((v) => ({ label: notifyOnLabel[v], value: v }))

  function aliasOf(id: string) {
    return serverStore.servers.find((s) => s.id === id)?.alias || id
  }

  async function load() {
    loading.value = true
    try {
      const r = await api.get<{ items: Schedule[] }>('/flow-schedules')
      schedules.value = r.items.filter((s) => s.flowId === props.flow.flowId)
    } catch (e) {
      message.error((e as Error).message)
    } finally {
      loading.value = false
    }
  }
  onMounted(load)
  watch(() => props.flow.flowId, load)

  async function create() {
    if (!name.value.trim() || !selectedServers.value.length) return
    creating.value = true
    try {
      await api.post<Schedule>('/flow-schedules', {
        flowId: props.flow.flowId,
        name: name.value.trim(),
        params: props.params,
        serverIds: selectedServers.value,
        concurrency: concurrency.value,
        intervalSecs: intervalSecs.value,
        notifyOn: notifyOn.value
      })
      message.success('已创建定时任务')
      name.value = ''
      selectedServers.value = []
      await load()
    } catch (e) {
      const err = e as Error & { errors?: string[] }
      message.error(err.errors?.length ? err.errors.join('；') : err.message)
    } finally {
      creating.value = false
    }
  }

  async function toggle(s: Schedule, enabled: boolean) {
    try {
      const updated = await api.post<Schedule>(`/flow-schedules/${s.scheduleId}/toggle`, { enabled })
      Object.assign(s, updated)
    } catch (e) {
      message.error((e as Error).message)
    }
  }

  async function runNow(s: Schedule) {
    try {
      await api.post(`/flow-schedules/${s.scheduleId}/run`, {})
      message.info('已触发；执行结果会在完成后进入通知与运行历史')
      setTimeout(load, 1500)
    } catch (e) {
      message.error((e as Error).message)
    }
  }

  async function remove(s: Schedule) {
    try {
      await api.del(`/flow-schedules/${s.scheduleId}`)
      schedules.value = schedules.value.filter((x) => x.scheduleId !== s.scheduleId)
    } catch (e) {
      message.error((e as Error).message)
    }
  }

  /** A schedule paused because the flow was edited to a newer version. */
  function stale(s: Schedule) {
    return s.flowVersion !== props.flow.version
  }
</script>

<template>
  <div class="border border-om-border rounded p-3 text-xs">
    <div class="mb-2 text-om-dimmed">
      定时批量只读执行：到点时用<b>你的登录会话</b>解密密钥并在所选服务器上执行；若届时你未登录，该次会被跳过并留一条通知。流程定义升级后，任务会自动暂停，待你确认新版本再启用。
    </div>

    <!-- create -->
    <div class="mb-3 border border-om-border rounded p-2">
      <div class="mb-2 flex items-center gap-2">
        <span class="w-16 text-om-dimmed">名称</span>
        <n-input v-model:value="name" size="small" placeholder="例如：每小时磁盘巡检" class="flex-1" />
      </div>
      <div class="mb-2 flex items-center gap-2">
        <span class="w-16 text-om-dimmed">服务器</span>
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
      <div class="mb-2 flex flex-wrap items-center gap-3">
        <div class="flex items-center gap-2">
          <span class="text-om-dimmed">频率</span>
          <n-select v-model:value="intervalSecs" size="small" :options="intervalOptions" style="width: 130px" />
        </div>
        <div class="flex items-center gap-2">
          <span class="text-om-dimmed">并发</span>
          <n-input-number v-model:value="concurrency" size="small" :min="1" :max="8" style="width: 90px" />
        </div>
        <div class="flex items-center gap-2">
          <span class="text-om-dimmed">通知</span>
          <n-select v-model:value="notifyOn" size="small" :options="notifyOptions" style="width: 130px" />
        </div>
        <n-button
          type="primary"
          size="small"
          :disabled="!name.trim() || !selectedServers.length"
          :loading="creating"
          @click="create"
        >
          创建定时任务
        </n-button>
      </div>
    </div>

    <!-- list -->
    <div v-if="loading" class="text-om-dimmed">加载中…</div>
    <div v-else-if="!schedules.length" class="text-om-dimmed">此流程还没有定时任务</div>
    <div v-for="s in schedules" :key="s.scheduleId" class="mb-2 border border-om-border rounded p-2">
      <div class="flex flex-wrap items-center gap-2">
        <n-switch :value="s.enabled" size="small" @update:value="(v: boolean) => toggle(s, v)" />
        <span class="font-semibold">{{ s.name }}</span>
        <n-tag size="tiny" :bordered="false">{{ intervalLabel(s.intervalSecs) }}</n-tag>
        <n-tag size="tiny" :bordered="false">{{ s.serverIds.length }} 台 · 并发 {{ s.concurrency }}</n-tag>
        <n-tag size="tiny" :bordered="false">{{ notifyOnLabel[s.notifyOn] }}</n-tag>
        <n-tag v-if="stale(s)" size="tiny" type="warning" :bordered="false"
          >定义已升级至 v{{ props.flow.version }}（记录于 v{{ s.flowVersion }}）</n-tag
        >
        <n-space size="small" class="ml-auto">
          <n-button size="tiny" quaternary @click="runNow(s)">立即运行</n-button>
          <n-popconfirm @positive-click="remove(s)">
            <template #trigger><n-button size="tiny" type="error" quaternary>删除</n-button></template>
            删除该定时任务？已产生的运行记录会保留。
          </n-popconfirm>
        </n-space>
      </div>
      <div class="mt-1 text-om-dimmed">
        目标：{{ s.serverIds.slice(0, 12).map(aliasOf).join('、')
        }}<span v-if="s.serverIds.length > 12"> 等 {{ s.serverIds.length }} 台</span>
      </div>
      <div class="mt-1 flex flex-wrap items-center gap-x-4 gap-y-1 text-om-dimmed">
        <span>下次：{{ s.enabled ? formatTime(s.nextRunAt) : '已暂停' }}</span>
        <span v-if="s.lastRunAt">上次：{{ formatTime(s.lastRunAt) }}</span>
        <span v-if="s.lastStatus" class="flex items-center gap-1">
          结果：<n-tag
            v-if="batchStatusInfo[s.lastStatus as keyof typeof batchStatusInfo]"
            size="tiny"
            :type="batchStatusInfo[s.lastStatus as keyof typeof batchStatusInfo].type"
            :bordered="false"
            >{{ batchStatusInfo[s.lastStatus as keyof typeof batchStatusInfo].label }}</n-tag
          ><span v-else>{{ s.lastStatus }}</span>
        </span>
      </div>
    </div>
  </div>
</template>
