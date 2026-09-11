<script setup lang="ts">
  import type { PaneContext } from '@/composables/usePaneTree'
  import type { SessionTab } from '@/stores/session'
  import { NBadge, NButton, NDropdown, NEmpty, NSpace, NTabPane, NTabs, NTooltip, useMessage } from 'naive-ui'
  import { computed, onMounted, onUnmounted, provide, ref, watch } from 'vue'
  import AgentChat from '@/components/agent-chat.vue'
  import { FileExplorer } from '@/components/file-explorer'
  import GitPanel from '@/components/git-panel.vue'
  import ResizeHandle from '@/components/layout/ResizeHandle.vue'
  import TerminalPane from '@/components/terminal-pane.vue'
  import TerminalView from '@/components/terminal-view.vue'
  import { useApi } from '@/composables/useApi'
  import {
    closePane as closePaneFn,
    collectSessionIds,
    handleRatioChange,
    splitPane as splitPaneFn,
    updatePaneNodeId
  } from '@/composables/usePaneTree'
  import { postWithAdmission } from '@/composables/useSessionAdmission'
  import { useTerminalTheme } from '@/composables/useTerminalTheme'
  import { useUiState } from '@/composables/useUiState'
  import { useSessionStore } from '@/stores/session'

  const sessionStore = useSessionStore()
  const api = useApi()
  const message = useMessage()

  function onAdmissionWait(requestId: string, msg: string) {
    message.info(`${msg}（申请 ${requestId.slice(0, 8)}），批准后自动连接`, { duration: 8000 })
  }
  const uiState = useUiState()

  const { currentThemeKey, themeOptions, handleThemeSelect } = useTerminalTheme()
  provide('terminalThemeKey', currentThemeKey)

  const terminalRefs = ref<Record<string, InstanceType<typeof TerminalView>>>({})
  provide('terminalRefs', terminalRefs)
  provide('insertToTerminal', (command: string, autoExec: boolean) => {
    const tabId = sessionStore.activeTabId
    if (!tabId) return
    const tab = sessionStore.tabs.find((t) => t.id === tabId)
    const paneId = tab?.activePaneId || tabId
    if (terminalRefs.value[paneId]) {
      terminalRefs.value[paneId].pasteCommand(command, autoExec)
    }
  })

  provide('createPaneSession', async (serverId: string, pendingId: string, cols: number, rows: number) => {
    let parentSessionId: string | undefined
    for (const tab of sessionStore.tabs) {
      if (tab.paneTree && collectSessionIds(tab.paneTree).includes(pendingId)) {
        parentSessionId = tab.id
        break
      }
    }
    const session = await postWithAdmission<SessionTab>(
      api,
      '/sessions',
      { serverId, cols, rows, parentSessionId },
      onAdmissionWait
    )
    for (const tab of sessionStore.tabs) {
      if (tab.paneTree && updatePaneNodeId(tab.paneTree, pendingId, session.id)) {
        if (tab.activePaneId === pendingId) tab.activePaneId = session.id
        if (terminalRefs.value[pendingId]) {
          terminalRefs.value[session.id] = terminalRefs.value[pendingId]
          const { [pendingId]: _, ...rest } = terminalRefs.value
          terminalRefs.value = rest as any
        }
        sessionStore.setStatus(session.id, 'connecting')
        savePaneState(tab.id)
        break
      }
    }
    return session
  })

  provide('reconnectPaneSession', async (oldSessionId: string, cols: number, rows: number) => {
    let serverId = ''
    let parentTabId = ''
    for (const tab of sessionStore.tabs) {
      if (tab.id === oldSessionId) {
        serverId = tab.serverId
        parentTabId = tab.id
        break
      }
      if (tab.paneTree && collectSessionIds(tab.paneTree).includes(oldSessionId)) {
        serverId = tab.serverId
        parentTabId = tab.id
        break
      }
    }
    if (!serverId) throw new Error('找不到对应的服务器')
    const session = await postWithAdmission<SessionTab>(
      api,
      '/sessions',
      {
        serverId,
        cols,
        rows,
        parentSessionId: parentTabId !== oldSessionId ? parentTabId : undefined
      },
      onAdmissionWait
    )
    for (const tab of sessionStore.tabs) {
      if (tab.paneTree && updatePaneNodeId(tab.paneTree, oldSessionId, session.id)) {
        if (tab.activePaneId === oldSessionId) tab.activePaneId = session.id
        if (terminalRefs.value[oldSessionId]) {
          terminalRefs.value[session.id] = terminalRefs.value[oldSessionId]
          const { [oldSessionId]: _, ...rest } = terminalRefs.value
          terminalRefs.value = rest as any
        }
        savePaneState(tab.id)
        break
      }
    }
    return session
  })

  const showFiles = ref(false)
  const showGit = ref(false)
  const showAgent = ref(false)
  const sidePanelWidth = ref(320)
  const agentPanelWidth = ref(400)
  const restored = ref(false)

  const sidePanel = computed(() => {
    if (showFiles.value) return 'files'
    if (showGit.value) return 'git'
    return null
  })

  const activeTab = computed({
    get: () => sessionStore.activeTabId,
    set: (val) => {
      if (val) sessionStore.setActive(val)
    }
  })

  const activeTabHasAi = computed(() => {
    const tab = sessionStore.tabs.find((t) => t.id === sessionStore.activeTabId)
    return !!tab?.aiToolId
  })

  onMounted(async () => {
    document.addEventListener('keydown', handleKeydown)
    try {
      const [sessions, savedState] = await Promise.all([api.get<SessionTab[]>('/sessions'), uiState.loadState()])

      const activeIds = sessions.map((s) => s.id)
      sessionStore.removeStale(activeIds)

      const tabSessions = sessions.filter((s) => !s.parentSessionId)
      sessionStore.restoreTabs(tabSessions, savedState.sessions)

      if (savedState.sessions) {
        for (const tab of sessionStore.tabs) {
          const ss = savedState.sessions[tab.id]
          if (ss?.paneTree) {
            tab.paneTree = ss.paneTree
            tab.activePaneId = ss.activePaneId
          }
        }
      }

      if (savedState.layout) {
        showFiles.value = savedState.layout.showFiles ?? false
        showGit.value = savedState.layout.showGit ?? false
        showAgent.value = savedState.layout.showAgent ?? false
        sidePanelWidth.value = savedState.layout.sidePanelWidth ?? 320
        agentPanelWidth.value = savedState.layout.agentPanelWidth ?? 400
      }

      if (savedState.activeSessionId && activeIds.includes(savedState.activeSessionId)) {
        sessionStore.setActive(savedState.activeSessionId)
      }
    } catch {}
    restored.value = true
  })

  function handleBeforeUnload() {
    uiState.flush()
  }

  onMounted(() => {
    window.addEventListener('beforeunload', handleBeforeUnload)
  })

  onUnmounted(() => {
    window.removeEventListener('beforeunload', handleBeforeUnload)
    document.removeEventListener('keydown', handleKeydown)
  })

  watch([showFiles, showGit, showAgent, sidePanelWidth, agentPanelWidth], () => {
    if (!restored.value) return
    uiState.updateLayout({
      showFiles: showFiles.value,
      showGit: showGit.value,
      showAgent: showAgent.value,
      sidePanelWidth: sidePanelWidth.value,
      agentPanelWidth: agentPanelWidth.value
    })
  })

  watch(
    () => sessionStore.activeTabId,
    (id) => {
      if (!restored.value) return
      uiState.setActiveSession(id)
    }
  )

  async function handleClose(tabId: string) {
    if (!tabId.startsWith('pending-')) {
      const tab = sessionStore.tabs.find((t) => t.id === tabId)
      if (tab?.paneTree) {
        const ids = collectSessionIds(tab.paneTree)
        for (const id of ids) {
          if (id !== tabId) api.del(`/sessions/${id}`).catch(() => {})
        }
      }
      try {
        await api.del(`/sessions/${tabId}`)
      } catch {}
    }
    sessionStore.removeTab(tabId)
    uiState.removeSession(tabId)
  }

  function togglePanel(panel: 'files' | 'git') {
    if (panel === 'files') {
      showFiles.value = !showFiles.value
      if (showFiles.value) showGit.value = false
    } else {
      showGit.value = !showGit.value
      if (showGit.value) showFiles.value = false
    }
  }

  function toggleAgent() {
    showAgent.value = !showAgent.value
  }

  function savePaneState(tabId: string) {
    const tab = sessionStore.tabs.find((t) => t.id === tabId)
    if (!tab) return
    uiState.updateSessionState(tabId, {
      paneTree: tab.paneTree,
      activePaneId: tab.activePaneId
    })
  }

  function handleSidePanelResize(delta: number) {
    sidePanelWidth.value = Math.max(200, Math.min(600, sidePanelWidth.value - delta))
  }

  function handleAgentResize(delta: number) {
    agentPanelWidth.value = Math.max(300, Math.min(800, agentPanelWidth.value - delta))
  }

  function handleFilePathChange(sessionId: string, path: string) {
    uiState.updateSessionState(sessionId, { fileExplorerPath: path })
  }

  function getTabPaneCtx(tabId: string): PaneContext | null {
    const tab = sessionStore.tabs.find((t) => t.id === tabId)
    if (!tab) return null
    return {
      getPaneTree: () => tab.paneTree,
      setPaneTree: (tree) => {
        tab.paneTree = tree
      },
      getSessionId: () => tab.id,
      getServerId: () => tab.serverId,
      setActivePaneId: (id) => {
        tab.activePaneId = id
      },
      setStatus: (id, status) => sessionStore.setStatus(id, status),
      deleteFn: (id) => api.del(`/sessions/${id}`).catch(() => {}),
      onChanged: () => savePaneState(tabId)
    }
  }

  function splitPane(tabId: string, paneId: string, direction: 'horizontal' | 'vertical') {
    const ctx = getTabPaneCtx(tabId)
    if (!ctx) return
    splitPaneFn(ctx, paneId, direction)
  }

  function closePane(tabId: string, paneId: string) {
    const ctx = getTabPaneCtx(tabId)
    if (!ctx) return
    closePaneFn(ctx, paneId)
  }

  function handlePaneRatioChange(tabId: string, splitId: string, ratio: number) {
    const ctx = getTabPaneCtx(tabId)
    if (!ctx) return
    handleRatioChange(ctx, splitId, ratio)
  }

  const tabCtxMenu = ref({ show: false, x: 0, y: 0, tabId: '' })
  const tabCtxOptions = [
    { label: '关闭', key: 'close' },
    { label: '关闭其他', key: 'closeOthers' },
    { label: '关闭右侧', key: 'closeRight' },
    { type: 'divider', key: 'd1' },
    { label: '向右分屏', key: 'splitH' },
    { label: '向下分屏', key: 'splitV' },
    { type: 'divider', key: 'd2' },
    { label: '复制连接信息', key: 'copyInfo' }
  ]

  function handleTabContextMenu(e: MouseEvent, tab: SessionTab) {
    e.preventDefault()
    tabCtxMenu.value = { show: true, x: e.clientX, y: e.clientY, tabId: tab.id }
  }

  async function handleTabCtxSelect(key: string) {
    tabCtxMenu.value.show = false
    const tabId = tabCtxMenu.value.tabId
    const tabs = sessionStore.tabs
    const idx = tabs.findIndex((t) => t.id === tabId)
    if (idx === -1) return

    switch (key) {
      case 'close':
        handleClose(tabId)
        break
      case 'closeOthers':
        for (const t of tabs.filter((t) => t.id !== tabId)) {
          await handleClose(t.id)
        }
        break
      case 'closeRight':
        for (const t of tabs.slice(idx + 1)) {
          await handleClose(t.id)
        }
        break
      case 'copyInfo': {
        const tab = tabs[idx]
        await navigator.clipboard.writeText(`${tab.serverAlias} (${tab.serverHost})`)
        break
      }
      case 'splitH':
        splitPane(tabId, tabs[idx].activePaneId || tabId, 'horizontal')
        break
      case 'splitV':
        splitPane(tabId, tabs[idx].activePaneId || tabId, 'vertical')
        break
    }
  }

  function handleKeydown(e: KeyboardEvent) {
    const tabs = sessionStore.tabs
    if (tabs.length === 0) return

    if (e.ctrlKey && e.key === 'Tab') {
      e.preventDefault()
      const currentIdx = tabs.findIndex((t) => t.id === sessionStore.activeTabId)
      if (e.shiftKey) {
        const prev = (currentIdx - 1 + tabs.length) % tabs.length
        sessionStore.setActive(tabs[prev].id)
      } else {
        const next = (currentIdx + 1) % tabs.length
        sessionStore.setActive(tabs[next].id)
      }
    }

    const tabId = sessionStore.activeTabId
    if (!tabId) return
    const tab = tabs.find((t) => t.id === tabId)
    if (!tab) return

    if (e.ctrlKey && e.shiftKey && e.key === 'D') {
      e.preventDefault()
      splitPane(tabId, tab.activePaneId || tabId, 'horizontal')
    } else if (e.ctrlKey && e.shiftKey && e.key === 'E') {
      e.preventDefault()
      splitPane(tabId, tab.activePaneId || tabId, 'vertical')
    } else if (e.ctrlKey && e.shiftKey && e.key === 'W') {
      e.preventDefault()
      if (tab.paneTree && tab.activePaneId) {
        closePane(tabId, tab.activePaneId)
      }
    }
  }
</script>

<template>
  <div class="h-full flex flex-col">
    <!-- Terminal tabs -->
    <div v-if="sessionStore.tabs.length > 0" class="min-h-0 flex-1">
      <n-tabs
        v-model:value="activeTab"
        type="card"
        closable
        class="h-full"
        pane-class="flex-1 min-h-0 !p-0"
        @close="handleClose"
      >
        <template #suffix>
          <!-- Agent toggle -->
          <n-button
            v-if="activeTabHasAi"
            size="tiny"
            quaternary
            class="mr-1"
            :type="showAgent ? 'primary' : 'default'"
            @click="toggleAgent"
          >
            <template #icon>
              <i class="i-ri:robot-2-line" style="display: inline-block; width: 14px; height: 14px" />
            </template>
          </n-button>
          <!-- Side panel buttons -->
          <n-button
            size="tiny"
            quaternary
            class="mr-1"
            :type="showFiles ? 'primary' : 'default'"
            @click="togglePanel('files')"
          >
            <template #icon>
              <i class="i-ri:folder-line" style="display: inline-block; width: 14px; height: 14px" />
            </template>
          </n-button>
          <n-button
            size="tiny"
            quaternary
            class="mr-1"
            :type="showGit ? 'primary' : 'default'"
            @click="togglePanel('git')"
          >
            <template #icon>
              <i class="i-ri:git-branch-line" style="display: inline-block; width: 14px; height: 14px" />
            </template>
          </n-button>
          <!-- Theme -->
          <n-dropdown :options="themeOptions" trigger="click" @select="handleThemeSelect">
            <n-button size="tiny" quaternary class="mr-2">
              <template #icon>
                <i class="i-ri:palette-line" style="display: inline-block; width: 14px; height: 14px" />
              </template>
            </n-button>
          </n-dropdown>
        </template>
        <n-tab-pane
          v-for="tab in sessionStore.tabs"
          :key="tab.id"
          :name="tab.id"
          display-directive="show:lazy"
          class="h-full"
        >
          <template #tab>
            <n-tooltip>
              <template #trigger>
                <n-space
                  :size="4"
                  align="center"
                  class="max-w-40 truncate"
                  @contextmenu.prevent="handleTabContextMenu($event, tab)"
                >
                  <n-badge
                    dot
                    :type="
                      sessionStore.statuses[tab.id] === 'disconnected'
                        ? 'error'
                        : sessionStore.statuses[tab.id] === 'connecting' ||
                            sessionStore.statuses[tab.id] === 'reconnecting'
                          ? 'warning'
                          : 'success'
                    "
                  />
                  <span class="truncate">{{ tab.serverAlias }}</span>
                </n-space>
              </template>
              {{ tab.serverAlias }} ({{ tab.serverHost }})
            </n-tooltip>
          </template>
          <!-- Main content area -->
          <div class="h-full flex">
            <!-- Terminal (always takes remaining space) -->
            <div class="min-w-0 flex-1">
              <terminal-pane
                v-if="tab.paneTree"
                :node="tab.paneTree"
                :active-pane-id="tab.activePaneId"
                :theme-key="currentThemeKey"
                @split="(id, dir) => splitPane(tab.id, id, dir)"
                @close="(id) => closePane(tab.id, id)"
                @focus="(id) => (tab.activePaneId = id)"
                @ratio-change="(id, r) => handlePaneRatioChange(tab.id, id, r)"
              />
              <terminal-view
                v-else
                :ref="
                  (el: any) => {
                    if (el) terminalRefs[tab.id] = el
                  }
                "
                :session-id="tab.id"
                :server-id="tab.serverId"
                :active="tab.id === activeTab"
                :theme-key="currentThemeKey"
                @split="(dir) => splitPane(tab.id, tab.id, dir)"
                @close="handleClose(tab.id)"
              />
            </div>
            <!-- Side panel: file browser / git -->
            <template v-if="sidePanel && !tab.id.startsWith('pending-')">
              <resize-handle direction="horizontal" @resize="handleSidePanelResize" />
              <div
                class="relative z-10 h-full shadow-[-4px_0_12px_rgba(0,0,0,0.3)]"
                :style="{ width: `${sidePanelWidth}px` }"
              >
                <file-explorer
                  v-if="sidePanel === 'files'"
                  :session-id="tab.id"
                  :initial-path="uiState.getState().sessions?.[tab.id]?.fileExplorerPath"
                  @path-change="(p: string) => handleFilePathChange(tab.id, p)"
                />
                <git-panel v-else-if="sidePanel === 'git'" :session-id="tab.id" />
              </div>
            </template>
            <!-- Agent panel (side by side) -->
            <template v-if="showAgent && tab.aiToolId && !tab.id.startsWith('pending-')">
              <resize-handle direction="horizontal" @resize="handleAgentResize" />
              <div
                class="relative z-10 h-full shadow-[-4px_0_12px_rgba(0,0,0,0.3)]"
                :style="{ width: `${agentPanelWidth}px` }"
              >
                <agent-chat v-model:tool-id="tab.aiToolId" :session-id="tab.id" />
              </div>
            </template>
          </div>
        </n-tab-pane>
      </n-tabs>
    </div>

    <!-- Empty state -->
    <div v-else class="flex flex-1 items-center justify-center">
      <n-empty description="暂无活跃连接" />
    </div>

    <n-dropdown
      trigger="manual"
      placement="bottom-start"
      :show="tabCtxMenu.show"
      :x="tabCtxMenu.x"
      :y="tabCtxMenu.y"
      :options="tabCtxOptions"
      @select="handleTabCtxSelect"
      @clickoutside="tabCtxMenu.show = false"
    />
  </div>
</template>
