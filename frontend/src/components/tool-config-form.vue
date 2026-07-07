<script setup lang="ts">
  import { NButton, NCheckbox, NForm, NFormItem, NInput, NModal, NTag, useMessage } from 'naive-ui'
  import { ref, watch } from 'vue'
  import JsonEditor from '@/components/json-editor.vue'
  import { useApi } from '@/composables/useApi'

  interface ParamDef {
    key: string
    label?: string
    required: boolean
    secret: boolean
    default?: string
    usage?: string
  }

  interface ParamRow {
    key: string
    label: string
    usage: string
    enabled: boolean
    value: string
    secret: boolean
    defaultValue: string
  }

  const props = defineProps<{
    show: boolean
    toolId: string
    toolName: string
    params: ParamDef[]
  }>()

  const emit = defineEmits<{
    (e: 'update:show', val: boolean): void
    (e: 'saved'): void
  }>()

  const api = useApi()
  const message = useMessage()
  const paramRows = ref<ParamRow[]>([])
  const customEnvs = ref<Array<{ key: string; value: string }>>([])
  const configOverride = ref('')
  const saving = ref(false)

  watch(
    () => props.show,
    (v) => {
      if (v) {
        paramRows.value = props.params.map((p) => ({
          key: p.key,
          label: p.label || p.key,
          usage: p.usage || 'env',
          enabled: true,
          value: '',
          secret: p.secret,
          defaultValue: p.secret && p.default === '***' ? '已预设全局默认值' : p.default || ''
        }))
        customEnvs.value = []
        configOverride.value = ''
        loadExisting()
      }
    },
    { immediate: true }
  )

  async function loadExisting() {
    try {
      const cfg = await api.get<{
        configOverride: string | null
        disabledKeys?: string[]
        envValues?: Record<string, string>
      }>(`/tools/${props.toolId}/config`)
      if (cfg.configOverride) configOverride.value = cfg.configOverride
      if (cfg.disabledKeys) {
        for (const row of paramRows.value) {
          if (cfg.disabledKeys.includes(row.key)) {
            row.enabled = false
          }
        }
      }
      if (cfg.envValues) {
        const definedKeys = new Set(props.params.map((p) => p.key))
        for (const row of paramRows.value) {
          if (cfg.envValues[row.key]) {
            if (row.secret && cfg.envValues[row.key] === '***') {
              row.value = ''
              row.defaultValue = '已配置 (留空保持不变)'
            } else {
              row.value = cfg.envValues[row.key]
            }
          }
        }
        for (const [key, value] of Object.entries(cfg.envValues)) {
          if (!definedKeys.has(key)) {
            customEnvs.value.push({ key, value })
          }
        }
      }
    } catch {}
  }

  async function handleSave() {
    saving.value = true
    try {
      const envValues: Record<string, string> = {}
      const disabledKeys: string[] = []

      for (const row of paramRows.value) {
        if (!row.enabled) {
          disabledKeys.push(row.key)
        } else if (row.value.trim()) {
          envValues[row.key] = row.value
        } else if (row.secret && row.defaultValue.startsWith('已配置')) {
          envValues[row.key] = '***'
        }
      }

      for (const custom of customEnvs.value) {
        if (custom.key.trim()) {
          envValues[custom.key.trim()] = custom.value
        }
      }

      const hasValues = Object.values(envValues).some((v) => v.trim())
      await api.put(`/tools/${props.toolId}/config`, {
        configOverride: configOverride.value.trim() || null,
        envValues: hasValues ? envValues : null,
        disabledKeys: disabledKeys.length > 0 ? disabledKeys : null
      })
      message.success('配置已保存')
      emit('update:show', false)
      emit('saved')
    } catch (e: any) {
      message.error(e.message)
    } finally {
      saving.value = false
    }
  }
</script>

<template>
  <n-modal
    :show="show"
    :title="`${toolName} 配置`"
    preset="card"
    class="w-140"
    @update:show="emit('update:show', $event)"
  >
    <n-form label-placement="top" @submit.prevent="handleSave">
      <div class="mb-2 text-sm text-om-text">参数配置</div>
      <div v-for="row in paramRows" :key="row.key" class="mb-3 flex items-center gap-2">
        <n-checkbox v-model:checked="row.enabled" class="w-52 whitespace-nowrap">
          <span class="text-sm">{{ row.label }}</span>
          <n-tag v-if="row.usage === 'config'" size="tiny" class="ml-1" :bordered="false">config</n-tag>
        </n-checkbox>
        <n-input
          v-if="row.enabled"
          v-model:value="row.value"
          :type="row.secret ? 'password' : 'text'"
          size="small"
          :placeholder="row.defaultValue || '可选'"
          show-password-on="click"
          class="flex-1"
        />
        <span v-else class="text-xs text-om-danger">已禁用</span>
      </div>
      <!-- Custom params -->
      <div v-for="(env, idx) in customEnvs" :key="`custom-${idx}`" class="mb-3 flex items-center gap-2">
        <n-input v-model:value="env.key" placeholder="KEY" size="small" style="width: 45%" />
        <n-input v-model:value="env.value" placeholder="VALUE" size="small" style="width: 45%" />
        <n-button size="tiny" quaternary type="error" @click="customEnvs.splice(idx, 1)">
          <template #icon>
            <i class="i-ri:delete-bin-line" style="display: inline-block; width: 14px; height: 14px" />
          </template>
        </n-button>
      </div>
      <n-button size="tiny" dashed class="mb-4" @click="customEnvs.push({ key: '', value: '' })"> + 添加参数 </n-button>

      <n-form-item label="配置覆盖 (JSON)">
        <json-editor v-model="configOverride" placeholder="留空使用标准模板" :rows="4" />
      </n-form-item>
      <div class="flex justify-end gap-2">
        <n-button @click="emit('update:show', false)">取消</n-button>
        <n-button type="primary" :loading="saving" attr-type="submit">保存</n-button>
      </div>
    </n-form>
  </n-modal>
</template>
