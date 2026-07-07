import type { PaneNode } from '@/stores/session'
import { useDebounceFn } from '@vueuse/core'
import { useApi } from './useApi'

export interface UiSessionState {
  viewMode?: 'terminal' | 'agent'
  fileExplorerPath?: string
  paneTree?: PaneNode
  activePaneId?: string
}

export interface UiLayoutState {
  showFiles?: boolean
  showGit?: boolean
  showAgent?: boolean
  sidePanelWidth?: number
  agentPanelWidth?: number
}

export interface UiState {
  layout?: UiLayoutState
  activeSessionId?: string | null
  sessions?: Record<string, UiSessionState>
}

const STORAGE_KEY = 'onemux-ui-state'
const api = useApi()

let cachedState: UiState = {}
let dirty = false

function persistToStorage() {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(cachedState))
  } catch {}
}

export function useUiState() {
  async function loadState(): Promise<UiState> {
    let localState: UiState | null = null
    try {
      const stored = localStorage.getItem(STORAGE_KEY)
      if (stored) localState = JSON.parse(stored)
    } catch {}

    try {
      const remoteState = await api.get<UiState>('/ui-state')
      const state = localState && localState.sessions ? localState : remoteState
      cachedState = state
      dirty = false
      persistToStorage()
      return state
    } catch {
      if (localState) {
        cachedState = localState
        return cachedState
      }
      return {}
    }
  }

  const debouncedSave = useDebounceFn(async (state: UiState) => {
    try {
      await api.put('/ui-state', state)
      dirty = false
    } catch {}
  }, 2000)

  function scheduleSave() {
    dirty = true
    persistToStorage()
    debouncedSave(cachedState)
  }

  function saveState(state: UiState) {
    cachedState = state
    scheduleSave()
  }

  function getState(): UiState {
    return cachedState
  }

  function updateSessionState(sessionId: string, patch: Partial<UiSessionState>) {
    const sessions = cachedState.sessions || {}
    sessions[sessionId] = { ...sessions[sessionId], ...patch }
    cachedState.sessions = sessions
    scheduleSave()
  }

  function updateLayout(patch: Partial<UiLayoutState>) {
    cachedState.layout = { ...cachedState.layout, ...patch }
    scheduleSave()
  }

  function setActiveSession(sessionId: string | null) {
    cachedState.activeSessionId = sessionId
    scheduleSave()
  }

  function removeSession(sessionId: string) {
    if (cachedState.sessions) {
      const { [sessionId]: _, ...rest } = cachedState.sessions
      cachedState.sessions = rest
      scheduleSave()
    }
  }

  function flush() {
    if (!dirty) return
    persistToStorage()
    try {
      const token = localStorage.getItem('onemux_token')
      const headers: Record<string, string> = { 'Content-Type': 'application/json' }
      if (token) headers.Authorization = `Bearer ${token}`
      fetch('/api/ui-state', {
        method: 'PUT',
        headers,
        body: JSON.stringify(cachedState),
        keepalive: true
      })
    } catch {}
  }

  return {
    loadState,
    saveState,
    getState,
    updateSessionState,
    updateLayout,
    setActiveSession,
    removeSession,
    flush
  }
}
