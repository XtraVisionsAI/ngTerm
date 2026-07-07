import { defineStore } from 'pinia'
import { ref } from 'vue'

export interface TerminalPane {
  id: string
  type: 'terminal'
  sessionId: string
  serverId: string
}

export interface SplitPane {
  id: string
  type: 'split'
  direction: 'horizontal' | 'vertical'
  children: [PaneNode, PaneNode]
  ratio: number
}

export type PaneNode = TerminalPane | SplitPane

export interface SessionTab {
  id: string
  serverId: string
  serverAlias: string
  serverHost: string
  aiToolId?: string | null
  parentSessionId?: string | null
  paneTree?: PaneNode
  activePaneId?: string
}

export type SessionStatus = 'connecting' | 'connected' | 'disconnected' | 'reconnecting'

export type ViewMode = 'terminal' | 'agent'

export const useSessionStore = defineStore('session', () => {
  const tabs = ref<SessionTab[]>([])
  const activeTabId = ref<string | null>(null)
  const statuses = ref<Record<string, SessionStatus>>({})
  const viewModes = ref<Record<string, ViewMode>>({})

  function addTab(session: SessionTab, status: SessionStatus = 'connected') {
    tabs.value.push(session)
    statuses.value[session.id] = status
    viewModes.value[session.id] = session.aiToolId ? 'agent' : 'terminal'
    activeTabId.value = session.id
  }

  function updateTabSession(oldId: string, newSession: SessionTab) {
    const idx = tabs.value.findIndex((t) => t.id === oldId)
    if (idx !== -1) {
      tabs.value[idx] = newSession
      statuses.value[newSession.id] = 'connected'
      viewModes.value[newSession.id] = viewModes.value[oldId] || 'terminal'
      const { [oldId]: _, ...rest } = statuses.value
      statuses.value = rest
      const { [oldId]: __, ...restModes } = viewModes.value
      viewModes.value = restModes
      if (activeTabId.value === oldId) {
        activeTabId.value = newSession.id
      }
    }
  }

  function removeTab(id: string) {
    tabs.value = tabs.value.filter((t) => t.id !== id)
    const { [id]: _, ...rest } = statuses.value
    statuses.value = rest
    const { [id]: __, ...restModes } = viewModes.value
    viewModes.value = restModes
    if (activeTabId.value === id) {
      activeTabId.value = tabs.value.length > 0 ? tabs.value[0].id : null
    }
  }

  function setActive(id: string) {
    activeTabId.value = id
  }

  function setStatus(id: string, status: SessionStatus) {
    statuses.value[id] = status
  }

  function setViewMode(id: string, mode: ViewMode) {
    viewModes.value[id] = mode
  }

  function clearAll() {
    tabs.value = []
    activeTabId.value = null
    statuses.value = {}
    viewModes.value = {}
  }

  function removeStale(activeIds: string[]) {
    const activeSet = new Set(activeIds)
    const stale = tabs.value.filter((t) => !t.id.startsWith('pending-') && !activeSet.has(t.id))
    for (const t of stale) {
      removeTab(t.id)
    }
  }

  function restoreTabs(sessions: SessionTab[], sessionStates?: Record<string, { viewMode?: 'terminal' | 'agent' }>) {
    const existingIds = new Set(tabs.value.map((t) => t.id))
    for (const session of sessions) {
      if (existingIds.has(session.id)) continue
      tabs.value.push(session)
      statuses.value[session.id] = 'connecting'
      const saved = sessionStates?.[session.id]
      viewModes.value[session.id] = saved?.viewMode || (session.aiToolId ? 'agent' : 'terminal')
    }
    if (!activeTabId.value && tabs.value.length > 0) {
      activeTabId.value = tabs.value[0].id
    }
  }

  return {
    tabs,
    activeTabId,
    statuses,
    viewModes,
    addTab,
    updateTabSession,
    removeTab,
    setActive,
    setStatus,
    setViewMode,
    clearAll,
    removeStale,
    restoreTabs
  }
})
