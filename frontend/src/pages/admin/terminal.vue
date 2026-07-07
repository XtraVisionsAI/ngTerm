<script setup lang="ts">
  import type TerminalView from '@/components/terminal-view.vue'
  import type { PaneContext } from '@/composables/usePaneTree'
  import type { PaneNode } from '@/stores/session'
  import { NButton, NDropdown } from 'naive-ui'
  import { onMounted, provide, ref, watch } from 'vue'
  import { useRouter } from 'vue-router'
  import AgentChat from '@/components/agent-chat.vue'
  import { FileExplorer } from '@/components/file-explorer'
  import ResizeHandle from '@/components/layout/ResizeHandle.vue'
  import TerminalPane from '@/components/terminal-pane.vue'
  import { useApi } from '@/composables/useApi'
  import { usePaneKeyboard } from '@/composables/usePaneKeyboard'
  import {
    closePane as closePaneFn,
    collectSessionIds,
    handleRatioChange,
    splitPane as splitPaneFn,
    updatePaneNodeId
  } from '@/composables/usePaneTree'
  import { useTerminalTheme } from '@/composables/useTerminalTheme'
  import { useSessionStore } from '@/stores/session'

  const STORAGE_KEY = 'onemux-admin-terminal'

  const api = useApi()
  const router = useRouter()
  const sessionStore = useSessionStore()
  const { currentThemeKey, bgColor, themeOptions, handleThemeSelect } = useTerminalTheme()

  provide('terminalThemeKey', currentThemeKey)

  const terminalRefs = ref<Record<string, InstanceType<typeof TerminalView>>>({})
  provide('terminalRefs', terminalRefs)

  const sessionId = ref<string | null>(null)
  const aiToolId = ref<string | null>(null)
  const aiToolOptions = ref<Array<{ label: string; value: string }>>([])
  const showFiles = ref(false)
  const showAgent = ref(false)
  const agentPanelWidth = ref(400)
  const paneTree = ref<PaneNode>()
  const activePaneId = ref<string>()

  provide('insertToTerminal', (command: string, autoExec: boolean) => {
    const paneId = activePaneId.value || sessionId.value
    if (paneId && terminalRefs.value[paneId]) {
      terminalRefs.value[paneId].pasteCommand(command, autoExec)
    }
  })

  function saveState() {
    try {
      localStorage.setItem(
        STORAGE_KEY,
        JSON.stringify({
          paneTree: paneTree.value,
          activePaneId: activePaneId.value,
          aiToolId: aiToolId.value,
          showAgent: showAgent.value,
          agentPanelWidth: agentPanelWidth.value
        })
      )
    } catch {}
  }

  function loadSavedState(): {
    paneTree?: PaneNode
    activePaneId?: string
    aiToolId?: string
    showAgent?: boolean
    agentPanelWidth?: number
  } | null {
    try {
      const stored = localStorage.getItem(STORAGE_KEY)
      return stored ? JSON.parse(stored) : null
    } catch {
      return null
    }
  }

  function getPaneCtx(): PaneContext {
    return {
      getPaneTree: () => paneTree.value,
      setPaneTree: (tree) => {
        paneTree.value = tree
      },
      getSessionId: () => sessionId.value || '',
      getServerId: () => 'local',
      setActivePaneId: (id) => {
        activePaneId.value = id
      },
      setStatus: (id, status) => sessionStore.setStatus(id, status),
      deleteFn: (id) => api.del(`/sessions/${id}`).catch(() => {}),
      onChanged: saveState
    }
  }

  function doSplit(paneId: string, direction: 'horizontal' | 'vertical') {
    splitPaneFn(getPaneCtx(), paneId, direction)
  }

  function doClose(paneId: string) {
    closePaneFn(getPaneCtx(), paneId)
  }

  function doRatioChange(splitId: string, ratio: number) {
    handleRatioChange(getPaneCtx(), splitId, ratio)
  }

  provide('createPaneSession', async (_serverId: string, pendingId: string, cols: number, rows: number) => {
    const session = await api.post<{ id: string; aiToolId?: string }>('/admin/terminal', {
      cols,
      rows,
      aiToolId: aiToolId.value,
      parentSessionId: sessionId.value
    })
    if (paneTree.value && updatePaneNodeId(paneTree.value, pendingId, session.id)) {
      if (activePaneId.value === pendingId) activePaneId.value = session.id
      if (terminalRefs.value[pendingId]) {
        terminalRefs.value[session.id] = terminalRefs.value[pendingId]
        const { [pendingId]: _, ...rest } = terminalRefs.value
        terminalRefs.value = rest as any
      }
      sessionStore.setStatus(session.id, 'connecting')
      saveState()
    }
    return session
  })

  provide('reconnectPaneSession', async (oldSessionId: string, cols: number, rows: number) => {
    const session = await api.post<{ id: string; aiToolId?: string }>('/admin/terminal', {
      cols,
      rows,
      aiToolId: aiToolId.value,
      parentSessionId: sessionId.value
    })
    if (paneTree.value && updatePaneNodeId(paneTree.value, oldSessionId, session.id)) {
      if (activePaneId.value === oldSessionId) activePaneId.value = session.id
      if (sessionId.value === oldSessionId) sessionId.value = session.id
      if (terminalRefs.value[oldSessionId]) {
        terminalRefs.value[session.id] = terminalRefs.value[oldSessionId]
        const { [oldSessionId]: _, ...rest } = terminalRefs.value
        terminalRefs.value = rest as any
      }
      saveState()
    }
    return session
  })

  usePaneKeyboard({
    getSessionId: () => sessionId.value,
    getActivePaneId: () => activePaneId.value,
    getPaneTree: () => paneTree.value,
    splitPane: doSplit,
    closePane: doClose
  })

  onMounted(async () => {
    await loadAiTool()
    await restoreOrCreate()
  })

  async function loadAiTool() {
    try {
      const tools = await api.get<{ id: string; displayName: string }[]>('/tools')
      aiToolOptions.value = tools.map((t) => ({ label: t.displayName, value: t.id }))
      const saved = loadSavedState()
      if (saved?.aiToolId && tools.some((t) => t.id === saved.aiToolId)) {
        aiToolId.value = saved.aiToolId
      } else if (tools.length > 0) {
        aiToolId.value = tools[0].id
      }
      if (saved?.showAgent !== undefined) showAgent.value = saved.showAgent
      if (saved?.agentPanelWidth) agentPanelWidth.value = saved.agentPanelWidth
    } catch {}
  }

  async function restoreOrCreate() {
    try {
      const sessions = await api.get<{ id: string; parentSessionId?: string | null }[]>('/sessions')
      const rootSessions = sessions.filter((s) => !s.parentSessionId)

      if (rootSessions.length > 0) {
        sessionId.value = rootSessions[0].id
        const saved = loadSavedState()
        if (saved?.paneTree) {
          const treeSessionIds = collectSessionIds(saved.paneTree)
          const activeIds = new Set(sessions.map((s) => s.id))
          if (treeSessionIds.every((id) => activeIds.has(id))) {
            paneTree.value = saved.paneTree
            activePaneId.value = saved.activePaneId
            saveState()
            return
          }
        }
        if (sessions.length > 1) {
          const childSessions = sessions.filter((s) => s.parentSessionId === sessionId.value)
          if (childSessions.length > 0) {
            const existingPane: PaneNode = {
              id: sessionId.value,
              type: 'terminal',
              sessionId: sessionId.value,
              serverId: 'local'
            }
            let tree: PaneNode = existingPane
            for (const child of childSessions) {
              const childPane: PaneNode = { id: child.id, type: 'terminal', sessionId: child.id, serverId: 'local' }
              tree = {
                id: `split-${Date.now()}`,
                type: 'split',
                direction: 'horizontal',
                children: [tree, childPane],
                ratio: 0.5
              }
            }
            paneTree.value = tree
            activePaneId.value = childSessions[childSessions.length - 1].id
            saveState()
            return
          }
        }
        paneTree.value = {
          id: rootSessions[0].id,
          type: 'terminal',
          sessionId: rootSessions[0].id,
          serverId: 'local'
        }
      } else {
        const session = await api.post<{ id: string; aiToolId?: string }>('/admin/terminal', {
          aiToolId: aiToolId.value
        })
        sessionId.value = session.id
        if (session.aiToolId) aiToolId.value = session.aiToolId
        paneTree.value = {
          id: session.id,
          type: 'terminal',
          sessionId: session.id,
          serverId: 'local'
        }
      }
      saveState()
    } catch {}
  }

  async function closeTerminal() {
    const ids = paneTree.value ? collectSessionIds(paneTree.value) : []
    if (sessionId.value && !ids.includes(sessionId.value)) {
      ids.push(sessionId.value)
    }
    await Promise.all(ids.map((id) => api.del(`/sessions/${id}`).catch(() => {})))
    localStorage.removeItem(STORAGE_KEY)
    router.push('/admin')
  }

  function handleAgentResize(delta: number) {
    agentPanelWidth.value = Math.max(300, Math.min(800, agentPanelWidth.value - delta))
  }

  watch([paneTree, activePaneId, showAgent, agentPanelWidth, aiToolId], () => saveState(), { deep: true })
</script>

<template>
  <div class="h-full flex flex-col">
    <div class="flex items-center justify-between px-4 py-2" :style="{ borderBottom: `1px solid ${bgColor}` }">
      <span class="text-sm font-medium">服务器终端 (localhost)</span>
      <div class="flex items-center gap-1">
        <!-- Agent toggle -->
        <n-button
          v-if="aiToolId && sessionId"
          size="tiny"
          quaternary
          :type="showAgent ? 'primary' : 'default'"
          @click="showAgent = !showAgent"
        >
          <template #icon>
            <i class="i-ri:robot-2-line" style="display: inline-block; width: 14px; height: 14px" />
          </template>
        </n-button>
        <!-- File browser toggle -->
        <n-button
          v-if="sessionId"
          size="tiny"
          quaternary
          :type="showFiles ? 'primary' : 'default'"
          @click="showFiles = !showFiles"
        >
          <template #icon>
            <i class="i-ri:folder-line" style="display: inline-block; width: 14px; height: 14px" />
          </template>
        </n-button>
        <!-- Theme -->
        <n-dropdown :options="themeOptions" trigger="click" @select="handleThemeSelect">
          <n-button size="tiny" quaternary>
            <template #icon>
              <i class="i-ri:palette-line" style="display: inline-block; width: 14px; height: 14px" />
            </template>
          </n-button>
        </n-dropdown>
        <n-button quaternary size="small" @click="closeTerminal">
          <template #icon>
            <i class="i-ri:close-line" style="display: inline-block; width: 18px; height: 18px" />
          </template>
        </n-button>
      </div>
    </div>
    <div class="min-h-0 flex flex-1">
      <!-- Terminal -->
      <div class="min-w-0 flex-1" :style="{ background: bgColor }">
        <terminal-pane
          v-if="paneTree"
          :node="paneTree"
          :active-pane-id="activePaneId"
          :theme-key="currentThemeKey"
          @split="(id, dir) => doSplit(id, dir)"
          @close="(id) => doClose(id)"
          @focus="(id) => (activePaneId = id)"
          @ratio-change="(id, r) => doRatioChange(id, r)"
        />
      </div>
      <!-- File browser side panel -->
      <div v-if="showFiles && sessionId" class="relative z-10 h-full w-80">
        <file-explorer :session-id="sessionId" />
      </div>
      <!-- Agent panel (side by side) -->
      <template v-if="showAgent && aiToolId && sessionId">
        <resize-handle direction="horizontal" @resize="handleAgentResize" />
        <div
          class="relative z-10 h-full shadow-[-4px_0_12px_rgba(0,0,0,0.3)]"
          :style="{ width: `${agentPanelWidth}px` }"
        >
          <agent-chat v-model:tool-id="aiToolId" :session-id="sessionId" />
        </div>
      </template>
    </div>
  </div>
</template>
