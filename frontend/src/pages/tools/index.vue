<script setup lang="ts">
  import { NButton, NDataTable, NSpace, NTag } from 'naive-ui'
  import { h, onMounted, ref } from 'vue'
  import ToolConfigForm from '@/components/tool-config-form.vue'
  import { useApi } from '@/composables/useApi'

  interface ParamDef {
    key: string
    label?: string
    required: boolean
    secret: boolean
    default?: string
    usage: string
  }

  interface ToolInfo {
    id: string
    name: string
    displayName: string
    type: string
    options: Record<string, any>
    configured: boolean
    configuredValues?: Record<string, string>
  }

  const api = useApi()
  const tools = ref<ToolInfo[]>([])
  const loading = ref(false)
  const showConfigForm = ref(false)
  const editingTool = ref<ToolInfo | null>(null)

  onMounted(loadTools)

  async function loadTools() {
    loading.value = true
    try {
      tools.value = await api.get<ToolInfo[]>('/tools')
    } catch {}
    loading.value = false
  }

  function getParams(tool: ToolInfo): ParamDef[] {
    return tool.options?.params || []
  }

  function openConfig(tool: ToolInfo) {
    editingTool.value = tool
    showConfigForm.value = true
  }

  function renderParams(row: ToolInfo) {
    const params = getParams(row)
    const values = row.configuredValues || {}
    const customKeys = Object.keys(values).filter((k) => !params.some((p) => p.key === k))
    const allKeys = [...params.map((p) => p.key), ...customKeys]
    if (!allKeys.length) return h('span', { style: 'color:var(--om-text-dimmed);font-size:12px' }, '-')
    return h(
      'pre',
      { style: 'font-size:12px;margin:0;white-space:pre;line-height:1.6;font-family:monospace' },
      allKeys.map((key) => {
        const def = params.find((p) => p.key === key)
        const val = values[key]
        const prefix = def?.required ? '*' : ' '
        let display: string
        let color: string
        if (val !== undefined) {
          display = `${prefix}${key}=${val}`
          color = 'var(--om-success)'
        } else {
          display = `${prefix}${key}=${def?.default || ''}`
          color = 'var(--om-text-dimmed)'
        }
        return h('div', { key, style: `color:${color}` }, display)
      })
    )
  }

  const columns = [
    { title: '工具', key: 'displayName' },
    { title: '标识', key: 'name', width: 140 },
    {
      title: '参数',
      key: 'params',
      render: renderParams
    },
    {
      title: '状态',
      key: 'configured',
      width: 100,
      render: (row: ToolInfo) => {
        const params = getParams(row)
        const values = row.configuredValues || {}
        const missingRequired = params.filter((p) => p.required && !values[p.key])
        const isReady = missingRequired.length === 0
        return h(
          NTag,
          { type: isReady ? 'success' : 'warning', size: 'small' },
          { default: () => (isReady ? '就绪' : '未配置') }
        )
      }
    },
    {
      title: '操作',
      key: 'actions',
      width: 100,
      render: (row: ToolInfo) =>
        h(
          NSpace,
          { size: 8 },
          {
            default: () => [
              h(NButton, { size: 'tiny', quaternary: true, onClick: () => openConfig(row) }, { default: () => '配置' })
            ]
          }
        )
    }
  ]
</script>

<template>
  <div class="h-full p-4">
    <div class="mb-3">
      <h2 class="text-lg font-bold">工具配置</h2>
      <p class="mt-1 text-xs text-om-dimmed">管理 AI 工具的凭证和配置</p>
    </div>

    <n-data-table :columns="columns" :data="tools" :loading="loading" :bordered="false" size="small" />

    <tool-config-form
      v-if="editingTool"
      v-model:show="showConfigForm"
      :tool-id="editingTool.id"
      :tool-name="editingTool.displayName"
      :params="getParams(editingTool)"
      @saved="loadTools"
    />
  </div>
</template>
