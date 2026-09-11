<script setup lang="ts">
  import type { DataTableColumns } from 'naive-ui'
  import type { ApprovalPolicy, ApprovalRole } from '@/utils/approvals'
  import {
    NButton,
    NDataTable,
    NForm,
    NFormItem,
    NInputNumber,
    NModal,
    NPopconfirm,
    NSelect,
    NSpace,
    NSwitch,
    NTag,
    useMessage
  } from 'naive-ui'
  import { computed, h, onActivated, onMounted, ref } from 'vue'
  import { useApi } from '@/composables/useApi'
  import { policyOperationKinds, scopeKindLabel, scopeLabel } from '@/utils/approvals'
  import { formatTime } from '@/utils/format'

  const api = useApi()
  const message = useMessage()

  const users = ref<{ id: string; username: string }[]>([])
  const groups = ref<string[]>([])
  const servers = ref<{ id: string; alias?: string; host: string }[]>([])
  const roles = ref<ApprovalRole[]>([])
  const policies = ref<ApprovalPolicy[]>([])
  const loading = ref(false)

  async function loadAll() {
    loading.value = true
    try {
      const [u, r, p] = await Promise.all([
        api.get<{ id: string; username: string }[]>('/admin/users'),
        api.get<{ items: ApprovalRole[] }>('/admin/approval-roles'),
        api.get<{ items: ApprovalPolicy[] }>('/admin/approval-policies')
      ])
      users.value = u
      roles.value = r.items
      policies.value = p.items
    } catch (e) {
      message.error((e as Error).message)
    } finally {
      loading.value = false
    }
    try {
      groups.value = await api.get<string[]>('/groups')
    } catch {}
    try {
      servers.value = await api.get<{ id: string; alias?: string; host: string }[]>('/servers')
    } catch {}
  }
  onMounted(loadAll)
  onActivated(loadAll)

  const scopeOptions = Object.entries(scopeKindLabel).map(([value, label]) => ({ label, value }))
  const userOptions = computed(() => users.value.map((u) => ({ label: u.username, value: u.id })))
  const groupOptions = computed(() => groups.value.map((g) => ({ label: g, value: g })))
  const serverOptions = computed(() => servers.value.map((s) => ({ label: s.alias || s.host, value: s.id })))
  const riskOptions = [
    { label: '不额外要求（沿用引擎审批）', value: '' },
    { label: '低风险及以上', value: 'low' },
    { label: '中风险及以上', value: 'medium' },
    { label: '高风险及以上', value: 'high' },
    { label: '仅严重风险', value: 'critical' }
  ]

  // --- Roles ---
  const showRoleModal = ref(false)
  const roleForm = ref({ userId: null as string | null, scopeKind: 'all', scopeId: null as string | null })

  async function grantRole() {
    if (!roleForm.value.userId) return message.warning('请选择用户')
    if (roleForm.value.scopeKind !== 'all' && !roleForm.value.scopeId) return message.warning('请选择范围')
    try {
      await api.post('/admin/approval-roles', {
        userId: roleForm.value.userId,
        role: 'approver',
        scopeKind: roleForm.value.scopeKind,
        scopeId: roleForm.value.scopeKind === 'all' ? '' : roleForm.value.scopeId
      })
      message.success('已授予审批人角色')
      showRoleModal.value = false
      roleForm.value = { userId: null, scopeKind: 'all', scopeId: null }
      loadAll()
    } catch (e) {
      message.error((e as Error).message)
    }
  }

  async function revokeRole(r: ApprovalRole) {
    try {
      await api.del('/admin/approval-roles', {
        userId: r.userId,
        role: r.role,
        scopeKind: r.scopeKind,
        scopeId: r.scopeId
      })
      message.success('已撤销')
      loadAll()
    } catch (e) {
      message.error((e as Error).message)
    }
  }

  const roleColumns: DataTableColumns<ApprovalRole> = [
    { title: '用户', key: 'username', render: (r) => r.username || r.userId },
    {
      title: '角色',
      key: 'role',
      width: 100,
      render: () => h(NTag, { size: 'small', bordered: false }, () => '审批人')
    },
    { title: '范围', key: 'scope', render: (r) => scopeLabel(r.scopeKind, r.scopeId) },
    { title: '授予时间', key: 'grantedAt', width: 160, render: (r) => formatTime(r.grantedAt) },
    {
      title: '',
      key: 'actions',
      width: 80,
      render: (r) =>
        h(
          NPopconfirm,
          { onPositiveClick: () => revokeRole(r) },
          {
            trigger: () => h(NButton, { size: 'tiny', type: 'error', quaternary: true }, () => '撤销'),
            default: () => '撤销该用户在此范围的审批人角色？'
          }
        )
    }
  ]

  // --- Policies ---
  const showPolicyModal = ref(false)
  const policyForm = ref({
    scopeKind: 'all',
    scopeId: null as string | null,
    sessionAdmission: false,
    operationMinRisk: '',
    operationKinds: [] as string[],
    ttlSecs: 1800
  })

  function editPolicy(p?: ApprovalPolicy) {
    policyForm.value = p
      ? {
          scopeKind: p.scopeKind,
          scopeId: p.scopeKind === 'all' ? null : p.scopeId,
          sessionAdmission: p.sessionAdmission,
          operationMinRisk: p.operationMinRisk || '',
          operationKinds: [...p.operationKinds],
          ttlSecs: p.ttlSecs
        }
      : {
          scopeKind: 'all',
          scopeId: null,
          sessionAdmission: false,
          operationMinRisk: '',
          operationKinds: [],
          ttlSecs: 1800
        }
    showPolicyModal.value = true
  }

  async function savePolicy() {
    const f = policyForm.value
    if (f.scopeKind !== 'all' && !f.scopeId) return message.warning('请选择范围')
    try {
      await api.put('/admin/approval-policies', {
        scopeKind: f.scopeKind,
        scopeId: f.scopeKind === 'all' ? '' : f.scopeId,
        sessionAdmission: f.sessionAdmission,
        operationMinRisk: f.operationMinRisk || null,
        operationKinds: f.operationKinds,
        ttlSecs: f.ttlSecs
      })
      message.success('策略已保存')
      showPolicyModal.value = false
      loadAll()
    } catch (e) {
      message.error((e as Error).message)
    }
  }

  async function deletePolicy(p: ApprovalPolicy) {
    try {
      const params = new URLSearchParams({ scopeKind: p.scopeKind, scopeId: p.scopeId })
      await api.del(`/admin/approval-policies?${params}`)
      message.success('已删除')
      loadAll()
    } catch (e) {
      message.error((e as Error).message)
    }
  }

  function kindLabels(kinds: string[]): string {
    if (kinds.length === 0) return '-'
    return kinds.map((k) => policyOperationKinds.find((o) => o.value === k)?.label || k).join('、')
  }

  const policyColumns: DataTableColumns<ApprovalPolicy> = [
    { title: '范围', key: 'scope', width: 200, render: (p) => scopeLabel(p.scopeKind, p.scopeId) },
    {
      title: '会话准入',
      key: 'sessionAdmission',
      width: 90,
      render: (p) =>
        h(NTag, { size: 'small', type: p.sessionAdmission ? 'warning' : 'default', bordered: false }, () =>
          p.sessionAdmission ? '需审批' : '否'
        )
    },
    {
      title: 'Agent 操作',
      key: 'operationMinRisk',
      width: 130,
      render: (p) => (p.operationMinRisk ? riskOptions.find((o) => o.value === p.operationMinRisk)?.label : '引擎默认')
    },
    {
      title: '受管操作',
      key: 'operationKinds',
      ellipsis: { tooltip: true },
      render: (p) => kindLabels(p.operationKinds)
    },
    { title: '有效期', key: 'ttlSecs', width: 90, render: (p) => `${Math.round(p.ttlSecs / 60)} 分钟` },
    {
      title: '版本',
      key: 'version',
      width: 110,
      render: (p) => h('span', { class: 'font-mono text-xs' }, p.version || '')
    },
    {
      title: '',
      key: 'actions',
      width: 120,
      render: (p) =>
        h(NSpace, { size: 4 }, () => [
          h(NButton, { size: 'tiny', onClick: () => editPolicy(p) }, () => '编辑'),
          h(
            NPopconfirm,
            { onPositiveClick: () => deletePolicy(p) },
            {
              trigger: () => h(NButton, { size: 'tiny', type: 'error', quaternary: true }, () => '删除'),
              default: () => '删除该范围的策略？将回退到更宽范围的策略。'
            }
          )
        ])
    }
  ]
</script>

<template>
  <div class="h-full overflow-auto p-4">
    <div class="mb-3 flex items-center justify-between">
      <h2 class="text-lg font-bold">审批配置</h2>
      <n-button size="small" :loading="loading" @click="loadAll">刷新</n-button>
    </div>

    <div class="mb-2 flex items-center justify-between">
      <div>
        <div class="font-semibold">审批策略</div>
        <div class="text-xs text-om-dimmed"
          >按服务器 → 分组 → 全部的顺序取最具体的一条。策略变更后已发起的申请保留发起时的策略版本。</div
        >
      </div>
      <n-button size="small" type="primary" @click="editPolicy()">新建策略</n-button>
    </div>
    <n-data-table
      :columns="policyColumns"
      :data="policies"
      :loading="loading"
      :row-key="(p: ApprovalPolicy) => `${p.scopeKind}:${p.scopeId}`"
      size="small"
      :bordered="false"
      class="mb-6"
    />

    <div class="mb-2 flex items-center justify-between">
      <div>
        <div class="font-semibold">审批人</div>
        <div class="text-xs text-om-dimmed">审批人不能批准自己的申请；管理员始终可以审批。</div>
      </div>
      <n-button size="small" type="primary" @click="showRoleModal = true">授予审批人</n-button>
    </div>
    <n-data-table
      :columns="roleColumns"
      :data="roles"
      :loading="loading"
      :row-key="(r: ApprovalRole) => `${r.userId}:${r.scopeKind}:${r.scopeId}`"
      size="small"
      :bordered="false"
    />

    <n-modal v-model:show="showRoleModal" preset="card" title="授予审批人角色" style="width: 420px">
      <n-form label-placement="left" label-width="70">
        <n-form-item label="用户">
          <n-select v-model:value="roleForm.userId" :options="userOptions" filterable placeholder="选择用户" />
        </n-form-item>
        <n-form-item label="范围">
          <n-select
            v-model:value="roleForm.scopeKind"
            :options="scopeOptions"
            @update:value="roleForm.scopeId = null"
          />
        </n-form-item>
        <n-form-item v-if="roleForm.scopeKind === 'group'" label="分组">
          <n-select
            v-model:value="roleForm.scopeId"
            :options="groupOptions"
            filterable
            tag
            placeholder="选择或输入分组"
          />
        </n-form-item>
        <n-form-item v-if="roleForm.scopeKind === 'server'" label="服务器">
          <n-select v-model:value="roleForm.scopeId" :options="serverOptions" filterable placeholder="选择服务器" />
        </n-form-item>
      </n-form>
      <template #footer>
        <n-space justify="end">
          <n-button size="small" @click="showRoleModal = false">取消</n-button>
          <n-button size="small" type="primary" @click="grantRole">授予</n-button>
        </n-space>
      </template>
    </n-modal>

    <n-modal v-model:show="showPolicyModal" preset="card" title="审批策略" style="width: 520px">
      <n-form label-placement="left" label-width="110">
        <n-form-item label="范围">
          <n-select
            v-model:value="policyForm.scopeKind"
            :options="scopeOptions"
            @update:value="policyForm.scopeId = null"
          />
        </n-form-item>
        <n-form-item v-if="policyForm.scopeKind === 'group'" label="分组">
          <n-select
            v-model:value="policyForm.scopeId"
            :options="groupOptions"
            filterable
            tag
            placeholder="选择或输入分组"
          />
        </n-form-item>
        <n-form-item v-if="policyForm.scopeKind === 'server'" label="服务器">
          <n-select v-model:value="policyForm.scopeId" :options="serverOptions" filterable placeholder="选择服务器" />
        </n-form-item>
        <n-form-item label="会话准入审批">
          <n-switch v-model:value="policyForm.sessionAdmission" />
          <span class="ml-2 text-xs text-om-dimmed"
            >人工打开交互会话前需审批人批准；会话全程录像，逐条命令不再审批。</span
          >
        </n-form-item>
        <n-form-item label="Agent 操作">
          <n-select v-model:value="policyForm.operationMinRisk" :options="riskOptions" />
        </n-form-item>
        <n-form-item label="受管操作">
          <n-select
            v-model:value="policyForm.operationKinds"
            :options="policyOperationKinds"
            multiple
            placeholder="文件/Git 等受管通道中需要第二人审批的操作"
          />
        </n-form-item>
        <n-form-item label="有效期（秒）">
          <n-input-number v-model:value="policyForm.ttlSecs" :min="30" :max="86400" :step="60" />
        </n-form-item>
      </n-form>
      <template #footer>
        <n-space justify="end">
          <n-button size="small" @click="showPolicyModal = false">取消</n-button>
          <n-button size="small" type="primary" @click="savePolicy">保存</n-button>
        </n-space>
      </template>
    </n-modal>
  </div>
</template>
