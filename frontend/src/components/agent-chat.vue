<script setup lang="ts">
  import type { AgentStatus, PermissionRequest, UserQuestion } from '@/composables/useAgentSocket'
  import {
    NButton,
    NCheckbox,
    NCollapse,
    NCollapseItem,
    NDropdown,
    NInput,
    NSelect,
    NSpin,
    NTag,
    useMessage
  } from 'naive-ui'
  import { computed, inject, nextTick, onMounted, onUnmounted, ref, watch } from 'vue'
  import AiContextBar from '@/components/ai-context-bar.vue'
  import ChangePreview from '@/components/change-preview.vue'
  import ServerToolConfigForm from '@/components/server-tool-config-form.vue'
  import ToolConfigForm from '@/components/tool-config-form.vue'
  import { useAgentSocket } from '@/composables/useAgentSocket'
  import { useApi } from '@/composables/useApi'
  import { renderMarkdown } from '@/composables/useMarkdown'
  import { composeMessage, useAiContextStore } from '@/stores/aiContext'
  import { useSessionStore } from '@/stores/session'

  interface ParamDef {
    key: string
    label?: string
    required: boolean
    secret: boolean
    default?: string
    usage?: string
  }

  interface ToolInfo {
    id: string
    name: string
    displayName: string
    type: string
    options: Record<string, any>
    configured: boolean
    configuredValues?: Record<string, string>
    userPrefs?: { approvalLevel?: string; target?: string }
  }

  const props = defineProps<{
    sessionId: string
    toolId: string
  }>()

  const emit = defineEmits<{
    (e: 'update:toolId', value: string): void
  }>()

  const api = useApi()
  const message = useMessage()
  const sessionStore = useSessionStore()
  const insertToTerminal = inject<(command: string, autoExec: boolean) => void>('insertToTerminal')
  const inputText = ref('')
  const messagesEl = ref<HTMLDivElement>()
  const agentId = ref<string | null>(null)
  const starting = ref(false)
  const promptInput = ref('')
  const approvalLevel = ref('none')
  const targetChoice = ref<'chat' | 'terminal'>('chat')
  const saveAsServerDefault = ref(false)
  const serverPrefs = ref<{ approvalLevel?: string; target?: string } | null>(null)
  const availableTools = ref<ToolInfo[]>([])
  const showConfigForm = ref(false)

  const selectedTool = computed(() => availableTools.value.find((t) => t.id === props.toolId))

  const toolOptions = computed(() => availableTools.value.map((t) => ({ label: t.displayName, key: t.id })))

  const toolTarget = computed(() => selectedTool.value?.options?.execution?.target || 'chat')
  const isNativeTool = computed(() => selectedTool.value?.type === 'native')
  const externalSupportsApproval = computed(() => {
    const ext = selectedTool.value?.options?.external || {}
    return !!(ext.supports_approval ?? ext.supportsApproval)
  })
  const canChooseApproval = computed(() => isNativeTool.value || externalSupportsApproval.value)
  const forceApprovalAbove = computed(() => selectedTool.value?.options?.execution?.force_approval_above || 'high')

  const targetOptions = computed(() => {
    const admin = toolTarget.value
    if (admin === 'both')
      return [
        { label: '后台', value: 'chat' },
        { label: '终端', value: 'terminal' }
      ]
    if (admin === 'terminal') return [{ label: '终端', value: 'terminal' }]
    return [{ label: '后台', value: 'chat' }]
  })

  const resolvedPrefs = computed(() => {
    const adminDefault = toolTarget.value === 'both' ? 'chat' : toolTarget.value
    const user = selectedTool.value?.userPrefs || {}
    const server = serverPrefs.value || {}
    return {
      approvalLevel: server.approvalLevel || user.approvalLevel || forceApprovalAbove.value,
      target: server.target || user.target || adminDefault
    }
  })

  const APPROVAL_LEVELS = ['none', 'low', 'medium', 'high', 'critical', 'all'] as const
  const APPROVAL_LABELS: Record<string, string> = {
    none: '无需审批',
    low: '≥ Low',
    medium: '≥ Medium',
    high: '≥ High',
    critical: '≥ Critical',
    all: '全部审批'
  }

  const approvalOptions = computed(() => {
    const minIdx = APPROVAL_LEVELS.indexOf(forceApprovalAbove.value as any)
    const floor = minIdx >= 0 ? minIdx : 0
    return APPROVAL_LEVELS.filter((_, i) => i >= floor).map((v) => ({
      label: APPROVAL_LABELS[v],
      value: v
    }))
  })

  const mergedEnvKeys = computed(() => {
    const tool = selectedTool.value
    if (!tool) return []
    const params: ParamDef[] = tool.options?.params || []
    const keys = [...params]
    const definedSet = new Set(keys.map((k) => k.key))
    for (const key of Object.keys(tool.configuredValues || {})) {
      if (!definedSet.has(key)) {
        keys.push({ key, required: false, secret: false })
      }
    }
    return keys
  })

  const currentServerId = computed(() => {
    const tab = sessionStore.tabs.find((t) => t.id === props.sessionId)
    return tab?.serverId || ''
  })

  let socket: ReturnType<typeof useAgentSocket> | null = null
  const connected = ref(false)
  const status = ref<AgentStatus>('idle')
  const messages = ref<ReturnType<typeof useAgentSocket>['messages']['value']>([])
  const tokenUsage = ref({ input: 0, output: 0 })
  const costUsd = ref(0)
  const pendingApproval = ref<PermissionRequest | null>(null)
  const pendingQuestion = ref<UserQuestion | null>(null)

  onMounted(loadTools)

  watch([() => props.toolId, currentServerId], () => {
    loadServerPrefs()
  })

  onUnmounted(() => {
    socket?.close()
  })

  watch(forceApprovalAbove, (val) => {
    approvalLevel.value = val
  })

  watch(resolvedPrefs, (prefs) => {
    approvalLevel.value = prefs.approvalLevel
    targetChoice.value = (prefs.target as 'chat' | 'terminal') || 'chat'
  })

  watch(
    () => props.sessionId,
    () => {
      if (!props.sessionId.startsWith('pending-')) {
        checkAgentStatus()
      }
    },
    { immediate: true }
  )

  async function loadTools() {
    try {
      availableTools.value = await api.get<ToolInfo[]>('/tools')
    } catch {}
    loadServerPrefs()
  }

  async function loadServerPrefs() {
    const serverId = currentServerId.value
    const toolId = props.toolId
    if (!serverId || !toolId) {
      serverPrefs.value = null
      return
    }
    try {
      const cfg = await api.get<{ configOverride?: string } | null>(
        `/tools/${toolId}/server-config?server_id=${serverId}`
      )
      if (cfg?.configOverride) {
        serverPrefs.value = JSON.parse(cfg.configOverride)
      } else {
        serverPrefs.value = null
      }
    } catch {
      serverPrefs.value = null
    }
  }

  async function checkAgentStatus() {
    try {
      const result = await api.get<{ active: boolean; agentId: string }>(`/sessions/${props.sessionId}/agent/status`)
      if (result.active) {
        agentId.value = result.agentId
        connectSocket()
      }
    } catch {}
  }

  async function startAgent() {
    if (!promptInput.value.trim()) return
    starting.value = true
    try {
      const payload: Record<string, any> = {
        prompt: promptInput.value.trim(),
        requireApproval: approvalLevel.value === 'all',
        approvalLevel: approvalLevel.value,
        target: targetChoice.value,
        toolId: props.toolId
      }

      if (saveAsServerDefault.value && currentServerId.value) {
        const prefs: Record<string, string> = {
          approvalLevel: approvalLevel.value,
          target: targetChoice.value
        }
        api
          .put(`/tools/${props.toolId}/server-config`, {
            serverId: currentServerId.value,
            configOverride: JSON.stringify(prefs)
          })
          .catch(() => {})
      }

      const result = await api.post<{ agentId: string }>(`/sessions/${props.sessionId}/agent`, payload)
      agentId.value = result.agentId
      promptInput.value = ''
      connectSocket()
    } catch {
      // error handled
    } finally {
      starting.value = false
    }
  }

  function connectSocket() {
    if (!agentId.value) return
    socket = useAgentSocket(agentId.value)
    socket.connect()

    watch(socket.connected, (v) => {
      connected.value = v
    })
    watch(socket.status, (v) => {
      status.value = v
    })
    watch(
      socket.messages,
      (v) => {
        messages.value = v
        nextTick(scrollToBottom)
      },
      { deep: true }
    )
    watch(
      socket.tokenUsage,
      (v) => {
        tokenUsage.value = v
      },
      { deep: true }
    )
    watch(socket.costUsd, (v) => {
      costUsd.value = v
    })
    watch(socket.pendingApproval, (v) => {
      pendingApproval.value = v
      if (v) nextTick(scrollToBottom)
    })
    watch(socket.pendingQuestion, (v) => {
      pendingQuestion.value = v
      if (v) nextTick(scrollToBottom)
    })
  }

  function answerQuestion(answer: string) {
    socket?.answerQuestion(answer)
    nextTick(scrollToBottom)
  }

  const aiContext = useAiContextStore()
  const contextItems = computed(() => aiContext.forTab(props.sessionId))
  const sendableContext = computed(() => contextItems.value.filter((it) => it.serverId === currentServerId.value))
  const foreignContext = computed(() => contextItems.value.filter((it) => it.serverId !== currentServerId.value))
  const canSend = computed(() => !!inputText.value.trim() || sendableContext.value.length > 0)

  // A source (e.g. "analyse this error" in the terminal) can prefill the prompt.
  watch(
    () => aiContext.pendingPrompt.get(props.sessionId),
    (p) => {
      if (!p) return
      const prompt = aiContext.takePrompt(props.sessionId)
      if (prompt && !inputText.value.trim()) inputText.value = prompt
    },
    { immediate: true }
  )

  function sendMessage() {
    const text = inputText.value.trim()
    if (!socket) return
    if (foreignContext.value.length > 0) {
      // Bound to another server: never silently drop or silently send.
      message.warning(`${foreignContext.value.length} 条上下文来自其他服务器的分屏，不会发送；请移除后重试`)
      return
    }
    const ctx = sendableContext.value
    if (!text && ctx.length === 0) return
    socket.sendMessage(composeMessage(text, ctx))
    for (const it of ctx) aiContext.remove(it.id)
    inputText.value = ''
    nextTick(scrollToBottom)
  }

  function handleApproval(granted: boolean) {
    socket?.respondPermission(granted)
  }

  const withdrawing = ref(false)
  async function withdrawApproval() {
    const id = pendingApproval.value?.secondPerson?.approvalRequestId
    if (!id) return
    withdrawing.value = true
    try {
      await api.post(`/approvals/${id}/cancel`, { reason: '申请人在对话中撤回' })
    } catch (e: any) {
      message.error(e.message)
    } finally {
      withdrawing.value = false
    }
  }

  async function stopAgent() {
    try {
      await api.del(`/sessions/${props.sessionId}/agent`)
    } catch {}
    socket?.close()
    socket = null
    agentId.value = null
    connected.value = false
    status.value = 'idle'
    messages.value = []
    pendingApproval.value = null
    pendingQuestion.value = null
  }

  function scrollToBottom() {
    if (messagesEl.value) {
      messagesEl.value.scrollTop = messagesEl.value.scrollHeight
    }
  }

  function statusLabel(s: AgentStatus): string {
    switch (s) {
      case 'thinking':
        return '思考中...'
      case 'tool_use':
        return '执行工具...'
      case 'waiting_approval':
        return '等待审批'
      case 'waiting_input':
        return '等待回答'
      case 'idle':
        return '就绪'
      case 'disconnected':
        return '已断开'
      case 'error':
        return '错误'
    }
  }

  function statusType(s: AgentStatus): 'success' | 'warning' | 'error' | 'info' {
    switch (s) {
      case 'thinking':
        return 'warning'
      case 'tool_use':
        return 'warning'
      case 'waiting_approval':
        return 'info'
      case 'waiting_input':
        return 'info'
      case 'idle':
        return 'success'
      case 'disconnected':
        return 'error'
      case 'error':
        return 'error'
    }
  }

  function handleKeyDown(e: KeyboardEvent) {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault()
      sendMessage()
    }
  }

  const riskLabel = computed(() => {
    const level = pendingApproval.value?.risk?.level
    switch (level) {
      case 'critical':
        return '严重风险'
      case 'high':
        return '高风险'
      case 'medium':
        return '中风险'
      case 'low':
        return '低风险'
      default:
        return '需要审批'
    }
  })

  const riskBorderClass = computed(() => {
    const level = pendingApproval.value?.risk?.level
    switch (level) {
      case 'critical':
        return 'border-red-500'
      case 'high':
        return 'border-orange-500'
      case 'medium':
        return 'border-yellow-500'
      default:
        return 'border-om-warning'
    }
  })

  const riskTextClass = computed(() => {
    const level = pendingApproval.value?.risk?.level
    switch (level) {
      case 'critical':
        return 'text-red-500'
      case 'high':
        return 'text-orange-500'
      case 'medium':
        return 'text-yellow-500'
      default:
        return 'text-om-warning'
    }
  })

  const msgCtxMenu = ref({ show: false, x: 0, y: 0, content: '', hasCode: false })
  const msgCtxOptions = computed(() => {
    const items: Array<{ label: string; key: string }> = [{ label: '复制文本', key: 'copyText' }]
    if (msgCtxMenu.value.hasCode) {
      items.push({ label: '复制代码', key: 'copyCode' })
    }
    return items
  })

  function handleMsgContextMenu(e: MouseEvent, content: string) {
    e.preventDefault()
    const codeMatch = content.match(/```[\s\S]*?\n([\s\S]*?)```/)
    msgCtxMenu.value = {
      show: true,
      x: e.clientX,
      y: e.clientY,
      content,
      hasCode: !!codeMatch
    }
  }

  async function handleMsgCtxSelect(key: string) {
    msgCtxMenu.value.show = false
    if (key === 'copyText') {
      await navigator.clipboard.writeText(msgCtxMenu.value.content)
    } else if (key === 'copyCode') {
      const blocks = [...msgCtxMenu.value.content.matchAll(/```[\w]*\n([\s\S]*?)```/g)]
      const code = blocks.map((m) => m[1].trim()).join('\n\n')
      await navigator.clipboard.writeText(code)
    }
  }

  function parseBashCommand(input: string): string | null {
    try {
      const parsed = JSON.parse(input)
      return parsed.command || null
    } catch {
      return null
    }
  }

  function isBashTool(name: string): boolean {
    return name === 'Bash' || name === 'bash' || name === 'execute_command'
  }

  async function copyCommand(input: string) {
    const cmd = parseBashCommand(input)
    if (cmd) await navigator.clipboard.writeText(cmd)
  }

  function insertCommand(input: string, autoExec: boolean) {
    const cmd = parseBashCommand(input)
    if (cmd && insertToTerminal) insertToTerminal(cmd, autoExec)
  }
</script>

<template>
  <div class="h-full flex flex-col border-l border-l-om-border border-l-solid bg-om-bg text-om-text">
    <!-- Not started: prompt to start -->
    <template v-if="!agentId">
      <div class="flex flex-1 flex-col items-center justify-center gap-4 p-6">
        <i
          class="i-ri:robot-2-line text-4xl text-om-primary"
          style="display: inline-block; width: 48px; height: 48px"
        />
        <p class="text-sm text-om-text">启动 AI 代理</p>
        <div class="max-w-lg w-full flex flex-col gap-2">
          <div class="flex items-center gap-1">
            <n-dropdown :options="toolOptions" trigger="click" @select="(v: string) => emit('update:toolId', v)">
              <n-button class="min-w-0 flex-1" quaternary>
                <template #icon>
                  <i class="i-ri:robot-2-line" style="display: inline-block; width: 14px; height: 14px" />
                </template>
                <span class="truncate">{{ selectedTool?.displayName || '选择工具' }}</span>
                <i
                  class="i-ri:arrow-down-s-line ml-1 shrink-0"
                  style="display: inline-block; width: 14px; height: 14px"
                />
              </n-button>
            </n-dropdown>
            <n-button size="small" quaternary @click="showConfigForm = true">
              <template #icon>
                <i class="i-ri:settings-3-line" style="display: inline-block; width: 14px; height: 14px" />
              </template>
            </n-button>
          </div>
          <n-input
            v-model:value="promptInput"
            type="textarea"
            :autosize="{ minRows: 3, maxRows: 8 }"
            placeholder="输入初始提示词..."
            @keydown="
              (e: KeyboardEvent) => {
                if (e.key === 'Enter' && e.metaKey) startAgent()
              }
            "
          />
          <div class="flex flex-col gap-2 text-xs">
            <div class="flex items-center gap-3">
              <span class="text-om-dimmed">执行目标:</span>
              <n-select
                v-model:value="targetChoice"
                :options="targetOptions"
                size="tiny"
                style="width: 80px"
                :disabled="targetOptions.length <= 1"
              />
              <template v-if="canChooseApproval">
                <span class="text-om-dimmed">|</span>
                <span class="text-om-dimmed">审批等级:</span>
                <n-select v-model:value="approvalLevel" :options="approvalOptions" size="tiny" style="width: 110px" />
              </template>
              <template v-else>
                <span class="text-om-dimmed">|</span>
                <span class="text-om-dimmed">该工具不支持审批，命令将直接执行</span>
              </template>
            </div>
            <div v-if="currentServerId" class="flex items-center gap-3">
              <n-checkbox v-model:checked="saveAsServerDefault" size="small">
                <span class="text-xs text-om-text">设为服务器默认</span>
              </n-checkbox>
            </div>
          </div>
          <n-button type="primary" :loading="starting" :disabled="!promptInput.trim()" @click="startAgent">
            启动代理
          </n-button>
        </div>
      </div>

      <server-tool-config-form
        v-if="selectedTool && currentServerId"
        v-model:show="showConfigForm"
        :tool-id="selectedTool.id"
        :tool-name="selectedTool.displayName"
        :server-id="currentServerId"
        :params="mergedEnvKeys"
        @saved="loadTools"
      />
      <tool-config-form
        v-else-if="selectedTool"
        v-model:show="showConfigForm"
        :tool-id="selectedTool.id"
        :tool-name="selectedTool.displayName"
        :params="mergedEnvKeys"
        @saved="loadTools"
      />
    </template>

    <!-- Active agent -->
    <template v-else>
      <!-- Header -->
      <div class="flex items-center gap-2 border-b border-om-border px-3 py-2">
        <i class="i-ri:robot-2-line" style="display: inline-block; width: 16px; height: 16px" />
        <span class="text-xs">AI 代理</span>
        <n-tag :type="statusType(status)" size="small">{{ statusLabel(status) }}</n-tag>
        <div class="flex-1" />
        <span v-if="costUsd > 0" class="text-xs text-om-dimmed">${{ costUsd.toFixed(4) }}</span>
        <n-button quaternary size="tiny" type="error" @click="stopAgent">停止</n-button>
      </div>

      <!-- Messages -->
      <div ref="messagesEl" class="min-h-0 flex-1 overflow-y-auto p-4">
        <div class="mx-auto max-w-3xl">
          <div v-for="(msg, i) in messages" :key="i" class="mb-3">
            <!-- User message -->
            <div v-if="msg.role === 'user'" class="flex justify-end">
              <div
                class="max-w-[80%] rounded-lg bg-om-accent px-3 py-2 text-sm text-white"
                @contextmenu.prevent="handleMsgContextMenu($event, msg.content)"
              >
                {{ msg.content }}
              </div>
            </div>

            <!-- System message -->
            <div v-else-if="msg.role === 'system'" class="flex justify-center">
              <span class="rounded bg-om-panel px-2 py-1 text-xs text-om-dimmed">{{ msg.content }}</span>
            </div>

            <!-- Assistant message -->
            <div v-else class="flex justify-start">
              <div class="max-w-[90%]" @contextmenu.prevent="handleMsgContextMenu($event, msg.content)">
                <div
                  v-if="msg.content"
                  class="agent-markdown mb-1 text-sm text-om-text leading-6"
                  v-html="renderMarkdown(msg.content)"
                />
                <!-- Tool use blocks -->
                <div v-if="msg.toolUse && msg.toolUse.length > 0" class="mt-1 flex flex-col gap-1">
                  <template v-for="tool in msg.toolUse" :key="tool.id">
                    <!-- Bash command card -->
                    <div v-if="isBashTool(tool.name)" class="command-card border border-om-border rounded bg-om-panel">
                      <div class="flex items-center gap-2 px-2 py-1">
                        <i class="i-ri:terminal-line block size-3.5 shrink-0 text-om-primary" />
                        <code class="min-w-0 flex-1 truncate text-xs text-om-text">{{
                          parseBashCommand(tool.input) || tool.input
                        }}</code>
                        <n-button quaternary size="tiny" title="复制" @click="copyCommand(tool.input)">
                          <template #icon><i class="i-ri:clipboard-line block size-3" /></template>
                        </n-button>
                        <n-button quaternary size="tiny" title="插入终端" @click="insertCommand(tool.input, false)">
                          <template #icon><i class="i-ri:terminal-box-line block size-3" /></template>
                        </n-button>
                        <n-button quaternary size="tiny" title="插入并执行" @click="insertCommand(tool.input, true)">
                          <template #icon><i class="i-ri:play-line block size-3" /></template>
                        </n-button>
                      </div>
                      <div v-if="tool.result" class="border-t border-om-border px-2 py-1">
                        <pre class="max-h-40 overflow-auto whitespace-pre-wrap break-all text-xs text-om-dimmed"
                          >{{ tool.result.slice(0, 2000) }}{{ tool.result.length > 2000 ? '...' : '' }}</pre>
                      </div>
                      <div v-else-if="!tool.result && tool.id" class="border-t border-om-border px-2 py-1">
                        <n-spin size="tiny" />
                      </div>
                    </div>
                    <!-- Other tools: collapse -->
                    <n-collapse v-else :default-expanded-names="[]" class="agent-collapse">
                      <n-collapse-item :title="`🔧 ${tool.name}`" :name="tool.id">
                        <div class="text-xs font-mono">
                          <div class="mb-1 text-om-primary">输入:</div>
                          <pre
                            class="overflow-x-auto whitespace-pre-wrap break-all rounded bg-om-bg p-2 text-om-success"
                            >{{ tool.input }}</pre>
                          <template v-if="tool.result">
                            <div class="mb-1 mt-2 text-om-primary">结果:</div>
                            <pre class="overflow-x-auto whitespace-pre-wrap break-all rounded bg-om-bg p-2 text-om-text"
                              >{{ tool.result?.slice(0, 2000)
                              }}{{ (tool.result?.length || 0) > 2000 ? '...' : '' }}</pre>
                          </template>
                          <div v-else class="mt-1">
                            <n-spin size="tiny" />
                          </div>
                        </div>
                      </n-collapse-item>
                    </n-collapse>
                  </template>
                </div>
              </div>
            </div>
          </div>

          <!-- Thinking indicator -->
          <div
            v-if="status === 'thinking' || status === 'tool_use'"
            class="flex items-center gap-2 text-xs text-om-dimmed"
          >
            <n-spin size="tiny" />
            <span>{{ statusLabel(status) }}</span>
          </div>

          <!-- ask_user question -->
          <div v-if="pendingQuestion" class="mt-2 border border-om-border rounded bg-om-panel p-3">
            <div class="mb-2 flex items-center gap-2 text-xs text-om-primary">
              <i class="i-ri:question-line" style="display: inline-block; width: 14px; height: 14px" />
              <span class="font-bold">代理提问</span>
            </div>
            <div class="mb-2 whitespace-pre-wrap text-xs text-om-text">{{ pendingQuestion.question }}</div>
            <div v-if="pendingQuestion.options.length > 0" class="flex flex-wrap gap-2">
              <n-button v-for="opt in pendingQuestion.options" :key="opt" size="small" @click="answerQuestion(opt)">
                {{ opt }}
              </n-button>
            </div>
            <div v-else class="text-xs text-om-dimmed">请在下方输入框中回答</div>
          </div>

          <!-- Approval dialog -->
          <div v-if="pendingApproval" class="mt-2 border rounded bg-om-panel p-3" :class="riskBorderClass">
            <div class="mb-2 flex items-center gap-2 text-xs" :class="riskTextClass">
              <i class="i-ri:shield-check-line" style="display: inline-block; width: 14px; height: 14px" />
              <span class="font-bold">{{ riskLabel }}</span>
              <span v-if="pendingApproval.target === 'terminal'" class="ml-auto text-om-dimmed"> 将在终端中执行</span>
            </div>
            <div class="mb-2 text-xs">
              <span class="text-om-primary">工具:</span>
              <span class="ml-1 font-mono">{{ pendingApproval.toolName }}</span>
            </div>
            <div v-if="pendingApproval.risk?.reason" class="mb-2 text-xs text-om-dimmed">
              原因: {{ pendingApproval.risk.reason }}
            </div>
            <div v-if="pendingApproval.description" class="mb-2 text-xs text-om-text">
              {{ pendingApproval.description }}
            </div>
            <change-preview v-if="pendingApproval.preview" :preview="pendingApproval.preview" compact class="mb-2" />
            <n-collapse v-if="pendingApproval.preview && Object.keys(pendingApproval.input).length > 0" class="mb-2">
              <n-collapse-item title="原始参数" name="raw">
                <pre
                  class="max-h-40 overflow-auto whitespace-pre-wrap break-all rounded bg-om-bg p-2 text-xs text-om-success font-mono"
                  >{{ JSON.stringify(pendingApproval.input, null, 2) }}</pre>
              </n-collapse-item>
            </n-collapse>
            <pre
              v-else-if="Object.keys(pendingApproval.input).length > 0"
              class="mb-2 max-h-40 overflow-auto whitespace-pre-wrap break-all rounded bg-om-bg p-2 text-xs text-om-success font-mono"
              >{{ JSON.stringify(pendingApproval.input, null, 2) }}</pre>
            <div v-if="pendingApproval.secondPerson" class="flex flex-wrap items-center gap-2 text-xs">
              <i class="i-ri:loader-4-line animate-spin" style="display: inline-block; width: 14px; height: 14px" />
              <span>策略要求第二人审批，等待审批人处理…</span>
              <span class="text-om-dimmed">申请 {{ pendingApproval.secondPerson.approvalRequestId.slice(0, 8) }}</span>
              <router-link
                class="text-om-primary"
                :to="{ path: '/approvals', query: { request: pendingApproval.secondPerson.approvalRequestId } }"
              >
                查看
              </router-link>
              <n-button size="tiny" class="ml-auto" :loading="withdrawing" @click="withdrawApproval">撤回申请</n-button>
            </div>
            <div v-else class="flex gap-2">
              <n-button size="small" type="success" @click="handleApproval(true)">允许</n-button>
              <n-button size="small" type="error" @click="handleApproval(false)">拒绝</n-button>
            </div>
          </div>
        </div>
      </div>

      <!-- Input -->
      <div class="border-t border-om-border p-3">
        <ai-context-bar :tab-id="sessionId" :server-id="currentServerId" />
        <div class="mx-auto max-w-3xl flex items-end gap-2">
          <n-input
            v-model:value="inputText"
            type="textarea"
            :autosize="{ minRows: 1, maxRows: 4 }"
            :placeholder="contextItems.length ? `附带 ${contextItems.length} 条上下文发送...` : '发送消息...'"
            :disabled="status === 'disconnected'"
            @keydown="handleKeyDown"
          />
          <n-button type="primary" size="small" :disabled="!canSend || status === 'disconnected'" @click="sendMessage">
            <template #icon>
              <i class="i-ri:send-plane-2-fill" style="display: inline-block; width: 14px; height: 14px" />
            </template>
          </n-button>
        </div>
        <div class="mx-auto mt-1 max-w-3xl flex items-center gap-2 text-xs text-om-dimmed">
          <span>{{ tokenUsage.input + tokenUsage.output }} tokens</span>
          <span>Enter 发送 · Shift+Enter 换行</span>
        </div>
      </div>
    </template>

    <n-dropdown
      trigger="manual"
      placement="bottom-start"
      :show="msgCtxMenu.show"
      :x="msgCtxMenu.x"
      :y="msgCtxMenu.y"
      :options="msgCtxOptions"
      @select="handleMsgCtxSelect"
      @clickoutside="msgCtxMenu.show = false"
    />
  </div>
</template>

<style scoped>
  .agent-collapse :deep(.n-collapse-item__header) {
    font-size: 12px;
    padding: 4px 0;
  }
  .agent-collapse :deep(.n-collapse-item__content-inner) {
    padding: 4px 0;
  }

  .agent-markdown :deep(p) {
    margin: 0.25em 0;
  }
  .agent-markdown :deep(h1),
  .agent-markdown :deep(h2),
  .agent-markdown :deep(h3),
  .agent-markdown :deep(h4) {
    margin: 0.5em 0 0.25em;
    font-weight: 600;
  }
  .agent-markdown :deep(h1) {
    font-size: 1.25em;
  }
  .agent-markdown :deep(h2) {
    font-size: 1.1em;
  }
  .agent-markdown :deep(h3) {
    font-size: 1em;
  }
  .agent-markdown :deep(ul),
  .agent-markdown :deep(ol) {
    margin: 0.25em 0;
    padding-left: 1.5em;
  }
  .agent-markdown :deep(li) {
    margin: 0.125em 0;
  }
  .agent-markdown :deep(code) {
    background: var(--om-panel);
    border-radius: 3px;
    padding: 0.1em 0.4em;
    font-size: 0.9em;
    font-family: ui-monospace, monospace;
  }
  .agent-markdown :deep(pre) {
    background: var(--om-panel);
    border-radius: 6px;
    padding: 0.75em 1em;
    margin: 0.5em 0;
    overflow-x: auto;
  }
  .agent-markdown :deep(pre code) {
    background: none;
    padding: 0;
    border-radius: 0;
    font-size: 0.85em;
  }
  .agent-markdown :deep(blockquote) {
    border-left: 3px solid var(--om-border);
    padding-left: 0.75em;
    margin: 0.5em 0;
    color: var(--om-dimmed);
  }
  .agent-markdown :deep(a) {
    color: var(--om-primary);
    text-decoration: underline;
  }
  .agent-markdown :deep(hr) {
    border: none;
    border-top: 1px solid var(--om-border);
    margin: 0.75em 0;
  }
  .agent-markdown :deep(table) {
    border-collapse: collapse;
    margin: 0.5em 0;
    font-size: 0.9em;
  }
  .agent-markdown :deep(th),
  .agent-markdown :deep(td) {
    border: 1px solid var(--om-border);
    padding: 0.3em 0.6em;
  }
  .agent-markdown :deep(th) {
    background: var(--om-panel);
    font-weight: 600;
  }
</style>
