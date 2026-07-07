<script setup lang="ts">
  import type { FormInst, FormRules } from 'naive-ui'
  import {
    NButton,
    NDataTable,
    NEmpty,
    NForm,
    NFormItem,
    NInput,
    NModal,
    NPopconfirm,
    NSpace,
    NTag,
    useMessage
  } from 'naive-ui'
  import { h, onActivated, onMounted, ref } from 'vue'
  import { useApi } from '@/composables/useApi'
  import { formatTime } from '@/utils/format'

  interface UserInfo {
    id: string
    username: string
    role: string
    createdAt: string
  }

  const api = useApi()
  const message = useMessage()

  const users = ref<UserInfo[]>([])
  const showCreateModal = ref(false)
  const showResetModal = ref(false)
  const resetUserId = ref('')
  const resetUsername = ref('')
  const formRef = ref<FormInst | null>(null)
  const resetFormRef = ref<FormInst | null>(null)

  const createForm = ref({ username: '', password: '' })
  const resetForm = ref({ newPassword: '' })

  const createRules: FormRules = {
    username: { required: true, message: '请输入用户名', trigger: 'blur' }
  }

  const resetRules: FormRules = {
    newPassword: [
      { required: true, message: '请输入新密码', trigger: 'blur' },
      { min: 6, message: '密码至少 6 位', trigger: 'blur' }
    ]
  }

  onMounted(loadUsers)
  onActivated(loadUsers)

  async function loadUsers() {
    users.value = await api.get<UserInfo[]>('/admin/users')
  }

  const columns = [
    { title: '用户名', key: 'username' },
    {
      title: '角色',
      key: 'role',
      width: 100,
      render: (row: UserInfo) =>
        h(NTag, { size: 'small', type: row.role === 'admin' ? 'warning' : 'info' }, { default: () => row.role })
    },
    {
      title: '创建时间',
      key: 'createdAt',
      width: 180,
      render: (row: UserInfo) => formatTime(row.createdAt)
    },
    {
      title: '操作',
      key: 'actions',
      width: 200,
      render: (row: UserInfo) =>
        h(NSpace, { size: 'small' }, () => [
          h(NButton, { size: 'small', quaternary: true, onClick: () => openReset(row) }, { default: () => '重置密码' }),
          h(
            NPopconfirm,
            { onPositiveClick: () => handleDelete(row.id) },
            {
              trigger: () => h(NButton, { size: 'small', type: 'error', quaternary: true }, { default: () => '删除' }),
              default: () => `确认删除用户 "${row.username}"？其所有密钥将被清除。`
            }
          )
        ])
    }
  ]

  function openReset(user: UserInfo) {
    resetUserId.value = user.id
    resetUsername.value = user.username
    resetForm.value = { newPassword: '' }
    showResetModal.value = true
  }

  async function handleCreate() {
    try {
      await formRef.value?.validate()
    } catch {
      return
    }
    try {
      const res = await api.post<{ userId: string; password: string }>('/admin/users', createForm.value)
      showCreateModal.value = false
      createForm.value = { username: '', password: '' }
      message.success(`用户已创建，初始密码: ${res.password}`)
      await loadUsers()
    } catch (e: any) {
      message.error(e.message)
    }
  }

  async function handleReset() {
    try {
      await resetFormRef.value?.validate()
    } catch {
      return
    }
    try {
      await api.post(`/admin/users/${resetUserId.value}/reset-password`, resetForm.value)
      showResetModal.value = false
      message.success(`已重置 ${resetUsername.value} 的密码（其 SSH 密钥已清除）`)
      await loadUsers()
    } catch (e: any) {
      message.error(e.message)
    }
  }

  async function handleDelete(id: string) {
    try {
      await api.del(`/admin/users/${id}`)
      message.success('用户已删除')
      await loadUsers()
    } catch (e: any) {
      message.error(e.message)
    }
  }
</script>

<template>
  <div class="h-full flex flex-col p-4">
    <div class="mb-3 flex items-center justify-between">
      <h2 class="text-lg font-bold">用户管理</h2>
      <n-button type="primary" size="small" @click="showCreateModal = true"> 创建用户 </n-button>
    </div>
    <div v-if="users.length === 0" class="flex flex-1 items-center justify-center">
      <n-empty description="暂无用户">
        <template #extra>
          <n-button size="small" type="primary" @click="showCreateModal = true"> 创建第一个用户 </n-button>
        </template>
      </n-empty>
    </div>
    <n-data-table v-else :columns="columns" :data="users" :bordered="false" flex-height class="min-h-0 flex-1" />

    <n-modal v-model:show="showCreateModal" title="创建用户" preset="card" class="w-100">
      <n-form ref="formRef" :model="createForm" :rules="createRules" @submit.prevent="handleCreate">
        <n-form-item label="用户名" path="username">
          <n-input v-model:value="createForm.username" placeholder="新用户名" />
        </n-form-item>
        <n-form-item label="初始密码（留空自动生成）">
          <n-input v-model:value="createForm.password" type="password" placeholder="可选" show-password-on="click" />
        </n-form-item>
        <n-space justify="end">
          <n-button @click="showCreateModal = false"> 取消 </n-button>
          <n-button type="primary" attr-type="submit"> 创建 </n-button>
        </n-space>
      </n-form>
    </n-modal>

    <n-modal v-model:show="showResetModal" :title="`重置密码: ${resetUsername}`" preset="card" class="w-100">
      <p class="mb-4 text-sm text-orange-500"> 注意：重置密码将清除该用户的所有 SSH 密钥。 </p>
      <n-form ref="resetFormRef" :model="resetForm" :rules="resetRules" @submit.prevent="handleReset">
        <n-form-item label="新密码" path="newPassword">
          <n-input
            v-model:value="resetForm.newPassword"
            type="password"
            placeholder="新密码"
            show-password-on="click"
          />
        </n-form-item>
        <n-space justify="end">
          <n-button @click="showResetModal = false"> 取消 </n-button>
          <n-button type="warning" attr-type="submit"> 确认重置 </n-button>
        </n-space>
      </n-form>
    </n-modal>
  </div>
</template>
