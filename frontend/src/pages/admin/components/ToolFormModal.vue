<script setup lang="ts">
  import type { McpServerItem } from '@/components/mcp-server-form.vue'
  import type { SkillItem } from '@/components/skill-form.vue'
  import { NButton, NCheckbox, NForm, NFormItem, NInput, NModal, NSelect, NSpace, NTabPane, NTabs } from 'naive-ui'
  import { computed, ref, watch } from 'vue'
  import JsonEditor from '@/components/json-editor.vue'
  import McpServerForm from '@/components/mcp-server-form.vue'
  import SkillForm from '@/components/skill-form.vue'

  interface ParamDef {
    key: string
    label: string
    required: boolean
    secret: boolean
    default?: string
    usage: 'env' | 'config'
  }

  interface AiTool {
    id: string
    name: string
    displayName: string
    type: string
    options: Record<string, any>
    createdAt: string
  }

  const props = defineProps<{
    show: boolean
    tool: AiTool | null
  }>()

  const emit = defineEmits<{
    'update:show': [value: boolean]
    submit: [payload: Record<string, unknown>, isEdit: boolean]
  }>()

  function toSlug(s: string): string {
    return s
      .trim()
      .toLowerCase()
      .replace(/[^a-z0-9一-鿿]+/g, '-')
      .replace(/^-|-$/g, '')
  }

  const form = ref({
    displayName: '',
    toolType: 'external' as 'external' | 'native',
    detectCmd: '',
    installCmd: '',
    launchCmd: '',
    configPath: '',
    configTpl: '{}',
    engineJson: '{}',
    target: 'chat' as string,
    forceApprovalAbove: 'high' as string,
    params: [] as ParamDef[],
    mcpServers: [] as McpServerItem[],
    skills: [] as SkillItem[]
  })

  const generatedName = computed(() => toSlug(form.value.displayName))
  const isNative = computed(() => form.value.toolType === 'native')

  const typeOptions = [
    { label: 'External CLI', value: 'external' },
    { label: 'Native Engine', value: 'native' }
  ]

  const targetOptions = [
    { label: '后台 (chat)', value: 'chat' },
    { label: '终端 (terminal)', value: 'terminal' },
    { label: '用户可选 (both)', value: 'both' }
  ]

  const approvalOptions = [
    { label: 'Low', value: 'low' },
    { label: 'Medium', value: 'medium' },
    { label: 'High', value: 'high' },
    { label: 'Critical', value: 'critical' }
  ]

  watch(
    () => props.show,
    (val) => {
      if (!val) return
      if (props.tool) {
        const opts = props.tool.options || {}
        const ext = opts.external || {}
        const exec = opts.execution || {}
        const params: ParamDef[] = (opts.params || []).map((p: any) => ({ ...p }))
        const engineVal = opts.engine || {}

        // Parse MCP servers from engine config
        const mcpServers: McpServerItem[] = (engineVal.mcp_servers || []).map((s: any) => ({
          name: s.name || '',
          command: s.command || '',
          args: (s.args || []).join(' '),
          env: Object.entries(s.env || {})
            .map(([k, v]) => `${k}=${v}`)
            .join(', ')
        }))

        // Parse skills from engine config
        const skills: SkillItem[] = (engineVal.skills || []).map((s: any) => ({
          name: s.name || '',
          description: s.description || '',
          prompt: s.prompt || ''
        }))

        form.value = {
          displayName: props.tool.displayName,
          toolType: (props.tool.type || 'external') as 'external' | 'native',
          detectCmd: ext.detect_cmd || ext.detectCmd || '',
          installCmd: ext.install_cmd || ext.installCmd || '',
          launchCmd: ext.launch_cmd || ext.launchCmd || '',
          configPath: ext.config_path || ext.configPath || '',
          configTpl: ext.config_tpl || ext.configTpl || '{}',
          engineJson: JSON.stringify(engineVal, null, 2),
          target: exec.target || 'chat',
          forceApprovalAbove: exec.force_approval_above || exec.forceApprovalAbove || 'high',
          params,
          mcpServers,
          skills
        }
      } else {
        form.value = {
          displayName: '',
          toolType: 'external',
          detectCmd: '',
          installCmd: '',
          launchCmd: '',
          configPath: '',
          configTpl: '{}',
          engineJson: '{}',
          target: 'chat',
          forceApprovalAbove: 'high',
          params: [],
          mcpServers: [],
          skills: []
        }
      }
    }
  )

  function addParam() {
    form.value.params.push({
      key: '',
      label: '',
      required: false,
      secret: false,
      usage: isNative.value ? 'config' : 'env'
    })
  }

  function removeParam(index: number) {
    form.value.params.splice(index, 1)
  }

  function handleSubmit() {
    const validParams = form.value.params.filter((p) => p.key.trim())

    const options: Record<string, any> = {
      params: validParams,
      execution: {
        target: form.value.target,
        force_approval_above: form.value.forceApprovalAbove
      }
    }

    if (form.value.toolType === 'external') {
      options.external = {
        detect_cmd: form.value.detectCmd,
        install_cmd: form.value.installCmd,
        launch_cmd: form.value.launchCmd,
        config_tpl: form.value.configTpl || '{}',
        config_path: form.value.configPath || null
      }
    } else {
      let engine: Record<string, any> = {}
      try {
        engine = JSON.parse(form.value.engineJson)
      } catch {
        engine = {}
      }

      // Merge MCP servers from form into engine config
      const mcpServers = form.value.mcpServers
        .filter((s) => s.name.trim() && s.command.trim())
        .map((s) => {
          const env: Record<string, string> = {}
          if (s.env.trim()) {
            s.env.split(',').forEach((pair) => {
              const [k, ...v] = pair.trim().split('=')
              if (k && v.length) env[k.trim()] = v.join('=').trim()
            })
          }
          return {
            name: s.name.trim(),
            command: s.command.trim(),
            args: s.args.trim() ? s.args.trim().split(/\s+/) : [],
            ...(Object.keys(env).length > 0 ? { env } : {})
          }
        })
      if (mcpServers.length > 0) {
        engine.mcp_servers = mcpServers
      } else {
        delete engine.mcp_servers
      }

      // Merge skills from form into engine config
      const skills = form.value.skills
        .filter((s) => s.name.trim())
        .map((s) => ({
          name: s.name.trim(),
          description: s.description.trim(),
          prompt: s.prompt
        }))
      if (skills.length > 0) {
        engine.skills = skills
      } else {
        delete engine.skills
      }

      options.engine = engine
    }

    const payload = {
      name: generatedName.value,
      displayName: form.value.displayName,
      toolType: form.value.toolType,
      options
    }
    emit('submit', payload, !!props.tool)
  }
</script>

<template>
  <n-modal
    :show="show"
    :title="tool ? '编辑工具' : '添加工具'"
    preset="card"
    class="w-220"
    @update:show="emit('update:show', $event)"
  >
    <n-form :model="form" label-placement="left" label-width="100" @submit.prevent="handleSubmit">
      <div class="flex gap-4">
        <n-form-item label="名称" class="flex-1">
          <n-input v-model:value="form.displayName" placeholder="Claude Code" />
        </n-form-item>
        <n-form-item label="类型" class="w-60">
          <n-select v-model:value="form.toolType" :options="typeOptions" />
        </n-form-item>
      </div>

      <div class="flex gap-4">
        <n-form-item label="执行目标" class="flex-1">
          <n-select v-model:value="form.target" :options="targetOptions" />
        </n-form-item>
        <n-form-item v-if="isNative" label="强制审批阈值" class="flex-1">
          <n-select v-model:value="form.forceApprovalAbove" :options="approvalOptions" />
        </n-form-item>
      </div>

      <!-- External tool fields -->
      <template v-if="!isNative">
        <n-form-item label="检测命令">
          <n-input v-model:value="form.detectCmd" placeholder="command -v claude" />
        </n-form-item>
        <n-form-item label="安装命令">
          <n-input v-model:value="form.installCmd" placeholder="npm i -g @anthropic-ai/claude-code" />
        </n-form-item>
        <n-form-item label="启动命令">
          <n-input v-model:value="form.launchCmd" placeholder="claude -p --output-format stream-json ..." />
        </n-form-item>
        <n-form-item label="配置路径">
          <n-input v-model:value="form.configPath" placeholder="~/.claude/settings.json (optional)" />
        </n-form-item>
        <n-form-item label="配置模板">
          <json-editor v-model="form.configTpl" placeholder="{}" :rows="4" />
        </n-form-item>
      </template>

      <!-- Native engine fields -->
      <template v-else>
        <n-tabs type="segment" size="small" class="mb-4">
          <n-tab-pane name="mcp" tab="MCP Servers">
            <mcp-server-form v-model="form.mcpServers" />
          </n-tab-pane>
          <n-tab-pane name="skills" tab="Skills">
            <skill-form v-model="form.skills" />
          </n-tab-pane>
          <n-tab-pane name="advanced" tab="高级配置">
            <json-editor v-model="form.engineJson" placeholder='{"llm": {...}, "tools": {...}}' :rows="10" />
          </n-tab-pane>
        </n-tabs>
      </template>

      <!-- Params (shared) -->
      <n-form-item label="参数定义">
        <div class="w-full">
          <div class="mb-1 flex flex-nowrap items-center gap-2 text-xs text-om-dimmed">
            <span style="width: 120px; flex: none">Label</span>
            <span style="width: 180px; flex: none">Key</span>
            <span style="width: 180px; flex: none">Default</span>
            <span style="width: 88px; flex: none">Usage</span>
            <span style="width: 50px; flex: none">必填</span>
            <span style="width: 50px; flex: none">敏感</span>
          </div>
          <div v-for="(param, idx) in form.params" :key="idx" class="mb-2 flex flex-nowrap items-center gap-2">
            <n-input v-model:value="param.label" placeholder="label" size="small" style="width: 120px; flex: none" />
            <n-input
              v-model:value="param.key"
              placeholder="KEY / llm.model"
              size="small"
              style="width: 180px; flex: none"
            />
            <n-input
              v-model:value="param.default"
              placeholder="default"
              size="small"
              style="width: 180px; flex: none"
            />
            <n-select
              v-model:value="param.usage"
              :options="[
                { label: 'env', value: 'env' },
                { label: 'config', value: 'config' }
              ]"
              size="small"
              style="width: 88px; flex: none"
            />
            <n-checkbox
              v-model:checked="param.required"
              size="small"
              style="width: 50px; flex: none"
              class="whitespace-nowrap"
            >
              <span class="text-xs">必填</span>
            </n-checkbox>
            <n-checkbox
              v-model:checked="param.secret"
              size="small"
              style="width: 50px; flex: none"
              class="whitespace-nowrap"
            >
              <span class="text-xs">敏感</span>
            </n-checkbox>
            <n-button size="tiny" quaternary type="error" class="shrink-0" @click="removeParam(idx)">
              <template #icon>
                <i class="i-ri:delete-bin-line" style="display: inline-block; width: 14px; height: 14px" />
              </template>
            </n-button>
          </div>
          <n-button size="tiny" dashed @click="addParam">+ 添加参数</n-button>
        </div>
      </n-form-item>

      <n-space justify="end">
        <n-button @click="emit('update:show', false)">取消</n-button>
        <n-button type="primary" attr-type="submit">{{ tool ? '保存' : '创建' }}</n-button>
      </n-space>
    </n-form>
  </n-modal>
</template>
