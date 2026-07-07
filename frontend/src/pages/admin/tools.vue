<script setup lang="ts">
  import { NButton, NDataTable, NPopconfirm, NSpace, NTag, useMessage } from 'naive-ui'
  import { h, onActivated, onMounted, ref } from 'vue'
  import { useApi } from '@/composables/useApi'
  import ToolFormModal from './components/ToolFormModal.vue'

  interface AiTool {
    id: string
    name: string
    displayName: string
    type: string
    options: Record<string, any>
    createdAt: string
  }

  const api = useApi()
  const message = useMessage()
  const tools = ref<AiTool[]>([])
  const loading = ref(false)
  const showModal = ref(false)
  const editingTool = ref<AiTool | null>(null)

  onMounted(loadTools)
  onActivated(loadTools)

  async function loadTools() {
    loading.value = true
    try {
      tools.value = await api.get<AiTool[]>('/admin/tools')
    } catch {}
    loading.value = false
  }

  function openCreate() {
    editingTool.value = null
    showModal.value = true
  }

  function openEdit(tool: AiTool) {
    editingTool.value = tool
    showModal.value = true
  }

  async function handleSubmit(payload: Record<string, unknown>, isEdit: boolean) {
    try {
      if (isEdit && editingTool.value) {
        await api.put(`/admin/tools/${editingTool.value.id}`, payload)
        message.success('已更新')
      } else {
        await api.post('/admin/tools', payload)
        message.success('已创建')
      }
      showModal.value = false
      await loadTools()
    } catch (e: any) {
      message.error(e.message)
    }
  }

  async function handleDelete(id: string) {
    try {
      await api.del(`/admin/tools/${id}`)
      message.success('已删除')
      await loadTools()
    } catch (e: any) {
      message.error(e.message)
    }
  }

  function renderType(row: AiTool) {
    const type = row.type || 'external'
    return h(NTag, { size: 'small', type: type === 'native' ? 'info' : 'default' }, { default: () => type })
  }

  function renderTarget(row: AiTool) {
    const target = row.options?.execution?.target || 'chat'
    const map: Record<string, string> = { chat: '后台', terminal: '终端', both: '可选' }
    return h('span', { style: 'font-size:12px' }, map[target] || target)
  }

  function renderParams(row: AiTool) {
    const params = row.options?.params || []
    if (!params.length) return h('span', { style: 'color:var(--om-text-dimmed);font-size:12px' }, '-')
    const lines = params.map((p: any) => {
      const prefix = p.required ? '*' : ' '
      const value = p.secret ? '<secret>' : p.default || ''
      return `${prefix}${p.key}${value ? `=${value}` : ''}`
    })
    return h(
      'pre',
      { style: 'font-size:12px;margin:0;white-space:pre;line-height:1.6;font-family:monospace' },
      lines.join('\n')
    )
  }

  const columns = [
    { title: '名称', key: 'displayName', width: 140 },
    { title: '标识', key: 'name', width: 130 },
    { title: '类型', key: 'type', width: 80, render: renderType },
    { title: '目标', key: 'target', width: 60, render: renderTarget },
    { title: '参数', key: 'params', render: renderParams },
    {
      title: '操作',
      key: 'actions',
      width: 140,
      render: (row: AiTool) =>
        h(
          NSpace,
          { size: 8 },
          {
            default: () => [
              h(NButton, { size: 'tiny', quaternary: true, onClick: () => openEdit(row) }, { default: () => '编辑' }),
              h(
                NPopconfirm,
                { onPositiveClick: () => handleDelete(row.id) },
                {
                  trigger: () =>
                    h(NButton, { size: 'tiny', quaternary: true, type: 'error' }, { default: () => '删除' }),
                  default: () => '确认删除？'
                }
              )
            ]
          }
        )
    }
  ]
</script>

<template>
  <div class="h-full p-4">
    <div class="mb-3 flex items-center justify-between">
      <h2 class="text-lg font-bold">AI 工具管理</h2>
      <n-button type="primary" size="small" @click="openCreate">添加工具</n-button>
    </div>

    <n-data-table :columns="columns" :data="tools" :loading="loading" :bordered="false" size="small" />

    <tool-form-modal v-model:show="showModal" :tool="editingTool" @submit="handleSubmit" />
  </div>
</template>
