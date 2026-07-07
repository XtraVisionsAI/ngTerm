<script setup lang="ts">
  import type { FormInst, FormRules } from 'naive-ui'
  import type { Server } from '@/stores/server'
  import { NButton, NForm, NFormItem, NInput, NInputNumber, NModal, NSelect, NSpace, useMessage } from 'naive-ui'
  import { computed, ref, watch } from 'vue'
  import { useApi } from '@/composables/useApi'
  import { useServerStore } from '@/stores/server'

  const props = defineProps<{
    show: boolean
    editingServer: Server | null
  }>()

  const emit = defineEmits<{
    'update:show': [val: boolean]
    saved: []
  }>()

  const api = useApi()
  const message = useMessage()
  const serverStore = useServerStore()

  const serverFormRef = ref<FormInst | null>(null)
  const serverForm = ref({
    groupName: null as string | null,
    alias: '',
    host: '',
    port: 22,
    username: 'root',
    keyId: null as string | null,
    aiToolId: null as string | null
  })

  const serverRules: FormRules = {
    alias: { required: true, message: '请输入别名', trigger: 'blur' },
    host: { required: true, message: '请输入主机地址', trigger: 'blur' },
    username: { required: true, message: '请输入用户名', trigger: 'blur' }
  }

  const groupOptions = computed(() => serverStore.groups.map((g) => ({ label: g, value: g })))
  const keyOptions = computed(() => serverStore.keys.map((k) => ({ label: k.name, value: k.id })))

  watch(
    () => props.show,
    (val) => {
      if (val) {
        if (props.editingServer) {
          const s = props.editingServer
          serverForm.value = {
            groupName: s.groupName || null,
            alias: s.alias,
            host: s.host,
            port: s.port,
            username: s.username,
            keyId: s.keyId,
            aiToolId: s.aiToolId
          }
        } else {
          serverForm.value = {
            groupName: null,
            alias: '',
            host: '',
            port: 22,
            username: 'root',
            keyId: null,
            aiToolId: null
          }
        }
      }
    }
  )

  async function handleSubmit() {
    try {
      await serverFormRef.value?.validate()
    } catch {
      return
    }
    try {
      const payload = { ...serverForm.value, groupName: serverForm.value.groupName || '' }
      if (props.editingServer) {
        await api.put(`/servers/${props.editingServer.id}`, payload)
        message.success('服务器已更新')
      } else {
        await api.post('/servers', payload)
        message.success('服务器添加成功')
      }
      emit('update:show', false)
      emit('saved')
    } catch (e: any) {
      message.error(e.message)
    }
  }
</script>

<template>
  <n-modal
    :show="show"
    :title="editingServer ? '编辑服务器' : '添加服务器'"
    preset="card"
    class="w-130"
    @update:show="emit('update:show', $event)"
  >
    <n-form ref="serverFormRef" :model="serverForm" :rules="serverRules" @submit.prevent="handleSubmit">
      <div class="flex gap-3">
        <n-form-item label="别名" path="alias" class="flex-1">
          <n-input v-model:value="serverForm.alias" placeholder="web-01" />
        </n-form-item>
        <n-form-item label="分组" path="groupName" class="flex-1">
          <n-select
            v-model:value="serverForm.groupName"
            :options="groupOptions"
            placeholder="选择或输入分组"
            filterable
            tag
            clearable
          />
        </n-form-item>
      </div>
      <div class="flex gap-3">
        <n-form-item label="主机" path="host" class="flex-1">
          <n-input v-model:value="serverForm.host" placeholder="10.0.1.10" />
        </n-form-item>
        <n-form-item label="端口" path="port" class="w-28">
          <n-input-number v-model:value="serverForm.port" :min="1" :max="65535" />
        </n-form-item>
      </div>
      <div class="flex gap-3">
        <n-form-item label="用户名" path="username" class="flex-1">
          <n-input v-model:value="serverForm.username" placeholder="root" />
        </n-form-item>
        <n-form-item label="SSH 密钥" class="flex-1">
          <n-select v-model:value="serverForm.keyId" :options="keyOptions" placeholder="选择密钥" clearable />
        </n-form-item>
        <n-form-item label="AI 工具" class="w-40">
          <n-select
            v-model:value="serverForm.aiToolId"
            :options="serverStore.aiToolOptions"
            placeholder="不启用"
            clearable
          />
        </n-form-item>
      </div>
      <n-space justify="end">
        <n-button @click="emit('update:show', false)">取消</n-button>
        <n-button type="primary" attr-type="submit">
          {{ editingServer ? '保存' : '创建' }}
        </n-button>
      </n-space>
    </n-form>
  </n-modal>
</template>
