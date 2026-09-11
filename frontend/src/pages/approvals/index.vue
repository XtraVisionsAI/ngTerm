<script setup lang="ts">
  import type { DataTableColumns } from 'naive-ui'
  import type { ApprovalRequest, ApprovalStatus } from '@/utils/approvals'
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
    NTabPane,
    NTabs,
    NTag,
    useMessage
  } from 'naive-ui'
  import { computed, h, onActivated, onBeforeUnmount, onMounted, ref, watch } from 'vue'
  import { useRoute } from 'vue-router'
  import { useApi } from '@/composables/useApi'
  import { useAuthStore } from '@/stores/auth'
  import { approvalKindLabel, approvalStatusInfo, riskLevelLabel, secondsUntil } from '@/utils/approvals'
  import { formatTime } from '@/utils/format'

  const api = useApi()
  const auth = useAuthStore()
  const route = useRoute()
  const message = useMessage()

  type View = 'inbox' | 'mine' | 'all'
  const view = ref<View>(auth.isAdmin ? 'all' : 'inbox')
  const items = ref<ApprovalRequest[]>([])
  const total = ref(0)
  const page = ref(1)
  const pageSize = ref(20)
  const loading = ref(false)
  const filterStatus = ref<ApprovalStatus | null>(null)
  const filterKind = ref<string | null>(null)

  const statusOptions = Object.entries(approvalStatusInfo).map(([value, info]) => ({ label: info.label, value }))
  const kindOptions = Object.entries(approvalKindLabel).map(([value, label]) => ({ label, value }))

  async function load() {
    loading.value = true
    try {
      const params = new URLSearchParams()
      params.set('view', view.value)
      params.set('limit', String(pageSize.value))
      params.set('offset', String((page.value - 1) * pageSize.value))
      if (filterStatus.value) params.set('status', filterStatus.value)
      if (filterKind.value) params.set('kind', filterKind.value)
      const data = await api.get<{ items: ApprovalRequest[]; total: number }>(`/approvals?${params}`)
      items.value = data.items
      total.value = data.total
    } catch (e) {
      message.error((e as Error).message)
    } finally {
      loading.value = false
    }
  }

  watch([view, filterStatus, filterKind], () => {
    page.value = 1
    load()
  })
  watch(page, load)

  // Refresh the list while pending requests are shown so expiry and other
  // people's decisions appear without a manual reload.
  let timer: ReturnType<typeof setInterval> | null = null
  onMounted(() => {
    load()
    openFromRoute()
    timer = setInterval(() => {
      if (
        view.value === 'inbox' ||
        filterStatus.value === 'pending' ||
        items.value.some((i) => i.status === 'pending')
      ) {
        load()
      }
    }, 15000)
  })
  onActivated(() => {
    load()
    openFromRoute()
  })
  onBeforeUnmount(() => {
    if (timer) clearInterval(timer)
  })

  // --- Detail drawer ---
  const showDetail = ref(false)
  const detail = ref<ApprovalRequest | null>(null)
  const comment = ref('')
  const deciding = ref(false)

  async function openDetail(id: string) {
    try {
      detail.value = await api.get<ApprovalRequest>(`/approvals/${id}`)
      comment.value = ''
      showDetail.value = true
    } catch (e) {
      message.error((e as Error).message)
    }
  }

  function openFromRoute() {
    const id = route.query.request
    if (typeof id === 'string' && id) openDetail(id)
  }
  watch(() => route.query.request, openFromRoute)

  async function decide(approve: boolean) {
    if (!detail.value) return
    deciding.value = true
    try {
      detail.value = await api.post<ApprovalRequest>(`/approvals/${detail.value.requestId}/decide`, {
        approve,
        comment: comment.value.trim() || undefined
      })
      message.success(approve ? '已批准' : '已拒绝')
      load()
    } catch (e) {
      message.error((e as Error).message)
    } finally {
      deciding.value = false
    }
  }

  async function cancelRequest() {
    if (!detail.value) return
    deciding.value = true
    try {
      detail.value = await api.post<ApprovalRequest>(`/approvals/${detail.value.requestId}/cancel`, {
        reason: comment.value.trim() || undefined
      })
      message.success('已撤回')
      load()
    } catch (e) {
      message.error((e as Error).message)
    } finally {
      deciding.value = false
    }
  }

  const snapshotText = computed(() => (detail.value ? JSON.stringify(detail.value.snapshot, null, 2) : ''))

  function remaining(r: ApprovalRequest): string {
    if (r.status !== 'pending') return '-'
    const s = secondsUntil(r.expiresAt)
    if (s <= 0) return '即将过期'
    if (s < 60) return `${s}s`
    return `${Math.floor(s / 60)} 分钟`
  }

  function riskTag(level: string | null) {
    if (!level) return null
    const type =
      level === 'critical' ? 'error' : level === 'high' ? 'error' : level === 'medium' ? 'warning' : 'default'
    return h(NTag, { size: 'small', type, bordered: false }, () => `${riskLevelLabel[level] || level}风险`)
  }

  const columns = computed<DataTableColumns<ApprovalRequest>>(() => [
    {
      title: '状态',
      key: 'status',
      width: 90,
      render: (r) => {
        const info = approvalStatusInfo[r.status]
        return h(NTag, { size: 'small', type: info.type, bordered: false }, () => info.label)
      }
    },
    { title: '类型', key: 'kind', width: 90, render: (r) => approvalKindLabel[r.kind] },
    {
      title: '摘要',
      key: 'summary',
      ellipsis: { tooltip: true },
      render: (r) =>
        h('div', { class: 'flex items-center gap-2' }, [
          riskTag(r.riskLevel),
          h('span', { class: 'font-mono text-xs' }, r.summary)
        ])
    },
    { title: '申请人', key: 'requester', width: 110, render: (r) => r.requesterUsername || r.requesterUserId },
    {
      title: '目标',
      key: 'target',
      width: 160,
      ellipsis: { tooltip: true },
      render: (r) => {
        const host = r.serverAlias || r.serverId || '-'
        return r.remoteUser ? `${r.remoteUser}@${host}` : host
      }
    },
    { title: '发起时间', key: 'createdAt', width: 160, render: (r) => formatTime(r.createdAt) },
    { title: '剩余', key: 'remaining', width: 90, render: (r) => remaining(r) },
    {
      title: '',
      key: 'actions',
      width: 80,
      render: (r) =>
        h(
          NButton,
          { size: 'tiny', type: r.canDecide ? 'primary' : 'default', onClick: () => openDetail(r.requestId) },
          () => (r.canDecide ? '审批' : '详情')
        )
    }
  ])
</script>

<template>
  <div class="h-full flex flex-col p-4">
    <div class="mb-2 flex items-center justify-between">
      <h2 class="text-lg font-bold">审批</h2>
      <n-button size="small" :loading="loading" @click="load">刷新</n-button>
    </div>
    <n-tabs v-model:value="view" type="line" size="small" class="mb-2">
      <n-tab-pane name="inbox" tab="待我审批" />
      <n-tab-pane name="mine" tab="我的申请" />
      <n-tab-pane name="all" tab="全部记录" />
    </n-tabs>
    <n-space class="mb-2" size="small">
      <n-select
        v-model:value="filterStatus"
        :options="statusOptions"
        placeholder="状态"
        clearable
        size="small"
        style="width: 120px"
      />
      <n-select
        v-model:value="filterKind"
        :options="kindOptions"
        placeholder="类型"
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
        :row-key="(r: ApprovalRequest) => r.requestId"
        size="small"
        :bordered="false"
      />
    </div>
    <div class="mt-2 flex justify-end">
      <n-pagination v-model:page="page" :page-size="pageSize" :item-count="total" size="small" />
    </div>

    <n-drawer v-model:show="showDetail" :width="520" placement="right">
      <n-drawer-content v-if="detail" title="审批详情" closable>
        <n-descriptions :column="1" label-placement="left" size="small" bordered>
          <n-descriptions-item label="状态">
            <n-tag size="small" :type="approvalStatusInfo[detail.status].type" :bordered="false">
              {{ approvalStatusInfo[detail.status].label }}
            </n-tag>
          </n-descriptions-item>
          <n-descriptions-item label="类型">{{ approvalKindLabel[detail.kind] }}</n-descriptions-item>
          <n-descriptions-item label="申请人">{{
            detail.requesterUsername || detail.requesterUserId
          }}</n-descriptions-item>
          <n-descriptions-item label="目标">
            {{ detail.remoteUser ? `${detail.remoteUser}@` : '' }}{{ detail.serverAlias || detail.serverId || '-' }}
            <span v-if="detail.serverGroup" class="ml-1 text-om-dimmed">({{ detail.serverGroup }})</span>
          </n-descriptions-item>
          <n-descriptions-item v-if="detail.operationKind" label="操作类型">{{
            detail.operationKind
          }}</n-descriptions-item>
          <n-descriptions-item v-if="detail.riskLevel" label="风险">
            {{ riskLevelLabel[detail.riskLevel] || detail.riskLevel }}
            <span v-if="detail.riskReason" class="ml-1 text-om-dimmed">{{ detail.riskReason }}</span>
          </n-descriptions-item>
          <n-descriptions-item label="摘要">
            <span class="break-all text-xs font-mono">{{ detail.summary }}</span>
          </n-descriptions-item>
          <n-descriptions-item label="发起时间">{{ formatTime(detail.createdAt) }}</n-descriptions-item>
          <n-descriptions-item label="过期时间">{{ formatTime(detail.expiresAt) }}</n-descriptions-item>
          <n-descriptions-item v-if="detail.decidedAt" label="审批">
            {{ detail.decidedByUsername || detail.decidedBy }} · {{ formatTime(detail.decidedAt) }}
            <div v-if="detail.decisionComment" class="text-om-dimmed">{{ detail.decisionComment }}</div>
          </n-descriptions-item>
          <n-descriptions-item v-if="detail.consumedAt" label="已执行">
            {{ formatTime(detail.consumedAt) }}
            <span v-if="detail.consumedOperationId" class="ml-1 text-xs text-om-dimmed font-mono">
              {{ detail.consumedOperationId }}
            </span>
          </n-descriptions-item>
          <n-descriptions-item v-if="detail.cancelledAt" label="撤回">
            {{ formatTime(detail.cancelledAt) }}
            <div v-if="detail.cancelReason" class="text-om-dimmed">{{ detail.cancelReason }}</div>
          </n-descriptions-item>
          <n-descriptions-item label="策略版本">
            <span class="text-xs font-mono">{{ detail.policyVersion }}</span>
          </n-descriptions-item>
          <n-descriptions-item label="快照哈希">
            <span class="break-all text-xs font-mono">{{ detail.snapshotHash }}</span>
          </n-descriptions-item>
        </n-descriptions>

        <div class="mb-1 mt-3 text-xs text-om-dimmed">冻结快照（执行时必须与此一致）</div>
        <pre class="max-h-60 overflow-auto whitespace-pre-wrap break-all rounded bg-om-bg p-2 text-xs font-mono">{{
          snapshotText
        }}</pre>

        <template v-if="detail.canDecide || detail.canCancel">
          <n-input
            v-model:value="comment"
            class="mt-3"
            type="textarea"
            size="small"
            :autosize="{ minRows: 2, maxRows: 4 }"
            :placeholder="detail.canDecide ? '审批意见（可选）' : '撤回原因（可选）'"
            maxlength="500"
          />
          <n-space class="mt-3" justify="end">
            <n-button v-if="detail.canCancel" size="small" :loading="deciding" @click="cancelRequest"
              >撤回申请</n-button
            >
            <n-button v-if="detail.canDecide" size="small" type="error" :loading="deciding" @click="decide(false)">
              拒绝
            </n-button>
            <n-button v-if="detail.canDecide" size="small" type="success" :loading="deciding" @click="decide(true)">
              批准
            </n-button>
          </n-space>
        </template>
      </n-drawer-content>
    </n-drawer>
  </div>
</template>
