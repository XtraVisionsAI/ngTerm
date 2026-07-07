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
    inheritedValue: string
  }

  const props = defineProps<{
    show: boolean
    toolId: string
    toolName: string
    serverId: string
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
      if (v) loadConfig()
    }
  )

  async function loadConfig() {
    paramRows.value = props.params.map((p) => ({
      key: p.key,
      label: p.label || p.key,
      usage: p.usage || 'env',
      enabled: true,
      value: '',
      secret: p.secret,
      inheritedValue: p.secret && p.default === '***' ? '已预设全局默认值' : p.default || ''
    }))
    customEnvs.value = []
    configOverride.value = ''

    try {
      const cfg = await api.get<{
        disabledKeys: string[]
        configOverride: string | null
        envOverrides?: Record<string, string>
      } | null>(`/tools/${props.toolId}/server-config?serverId=${props.serverId}`)

      if (cfg) {
        if (cfg.configOverride) configOverride.value = cfg.configOverride
        if (cfg.disabledKeys) {
          for (const row of paramRows.value) {
            if (cfg.disabledKeys.includes(row.key)) {
              row.enabled = false
            }
          }
        }
        if (cfg.envOverrides) {
          const definedKeys = new Set(props.params.map((p) => p.key))
          for (const row of paramRows.value) {
            if (cfg.envOverrides[row.key]) {
              if (row.secret && cfg.envOverrides[row.key] === '***') {
                row.value = ''
                row.inheritedValue = '已配置 (留空保持不变)'
              } else {
                row.value = cfg.envOverrides[row.key]
              }
            }
          }
          for (const [key, value] of Object.entries(cfg.envOverrides)) {
            if (!definedKeys.has(key)) {
              customEnvs.value.push({ key, value })
            }
          }
        }
      }
    } catch {}
  }

  async function handleSave() {
    saving.value = true
    try {
      const envOverrides: Record<string, string> = {}
      const disabledKeys: string[] = []

      for (const row of paramRows.value) {
        if (!row.enabled) {
          disabledKeys.push(row.key)
        } else if (row.value.trim()) {
          envOverrides[row.key] = row.value
        } else if (row.secret && row.inheritedValue.startsWith('已配置')) {
          envOverrides[row.key] = '***'
        }
      }

      for (const custom of customEnvs.value) {
        if (custom.key.trim()) {
          envOverrides[custom.key.trim()] = custom.value
        }
      }

      await api.put(`/tools/${props.toolId}/server-config`, {
        serverId: props.serverId,
        envOverrides: Object.keys(envOverrides).length > 0 ? envOverrides : null,
        disabledKeys: disabledKeys.length > 0 ? disabledKeys : null,
        configOverride: configOverride.value.trim() || null
      })
      message.success('服务器配置已保存')
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
    :title="`${toolName} - 服务器配置`"
    preset="card"
    class="w-160"
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
          show-password-on="click"
          :placeholder="row.inheritedValue ? `继承: ${row.inheritedValue}` : '留空继承全局值'"
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
        <json-editor v-model="configOverride" placeholder="留空继承全局配置" :rows="4" />
      </n-form-item>

      <div class="flex justify-end gap-2">
        <n-button @click="emit('update:show', false)">取消</n-button>
        <n-button type="primary" :loading="saving" attr-type="submit">保存</n-button>
      </div>
    </n-form>
  </n-modal>
</template>
