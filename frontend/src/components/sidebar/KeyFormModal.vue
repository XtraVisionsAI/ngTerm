<script setup lang="ts">
  import type { FormInst, FormRules } from 'naive-ui'
  import { NButton, NForm, NFormItem, NInput, NModal, NSpace, NUpload, useMessage } from 'naive-ui'
  import { ref, watch } from 'vue'
  import { useApi } from '@/composables/useApi'

  const props = defineProps<{
    show: boolean
  }>()

  const emit = defineEmits<{
    'update:show': [val: boolean]
    saved: []
  }>()

  const api = useApi()
  const message = useMessage()

  const keyFormRef = ref<FormInst | null>(null)
  const keyForm = ref({ name: '', privateKey: '' })

  const keyRules: FormRules = {
    name: { required: true, message: '请输入密钥名称', trigger: 'blur' },
    privateKey: [
      { required: true, message: '请粘贴私钥或上传文件', trigger: 'blur' },
      {
        validator: (_rule: any, value: string) => {
          if (!value) return true
          if (!value.trimStart().startsWith('-----BEGIN')) {
            return new Error('无效的密钥格式，需要 PEM 格式')
          }
          return true
        },
        trigger: 'blur'
      }
    ]
  }

  watch(
    () => props.show,
    (val) => {
      if (val) {
        keyForm.value = { name: '', privateKey: '' }
      }
    }
  )

  function handleFileRead(file: File) {
    const reader = new FileReader()
    reader.onload = (e) => {
      keyForm.value.privateKey = e.target?.result as string
    }
    reader.readAsText(file)
  }

  async function handleSubmit() {
    try {
      await keyFormRef.value?.validate()
    } catch {
      return
    }
    try {
      await api.post('/keys', keyForm.value)
      emit('update:show', false)
      keyForm.value = { name: '', privateKey: '' }
      message.success('密钥添加成功')
      emit('saved')
    } catch (e: any) {
      message.error(e.message)
    }
  }
</script>

<template>
  <n-modal :show="show" title="添加 SSH 密钥" preset="card" class="w-185" @update:show="emit('update:show', $event)">
    <n-form ref="keyFormRef" :model="keyForm" :rules="keyRules" @submit.prevent="handleSubmit">
      <n-form-item label="名称" path="name">
        <n-input v-model:value="keyForm.name" placeholder="prod-deploy-key" />
      </n-form-item>
      <n-form-item label="私钥" path="privateKey">
        <n-input
          v-model:value="keyForm.privateKey"
          type="textarea"
          :rows="8"
          placeholder="粘贴 PEM 格式私钥，或上传文件"
        />
      </n-form-item>
      <n-form-item label="或上传文件">
        <n-upload :max="1" :default-upload="false" @change="({ file }) => file.file && handleFileRead(file.file)">
          <n-button>选择密钥文件</n-button>
        </n-upload>
      </n-form-item>
      <n-space justify="end">
        <n-button @click="emit('update:show', false)">取消</n-button>
        <n-button type="primary" attr-type="submit">确认</n-button>
      </n-space>
    </n-form>
  </n-modal>
</template>
