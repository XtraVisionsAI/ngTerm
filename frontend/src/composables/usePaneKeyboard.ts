import type { PaneNode } from '@/stores/session'
import { onMounted, onUnmounted } from 'vue'

export interface PaneKeyboardOptions {
  getSessionId: () => string | null
  getActivePaneId: () => string | undefined
  getPaneTree: () => PaneNode | undefined
  splitPane: (paneId: string, direction: 'horizontal' | 'vertical') => void
  closePane: (paneId: string) => void
}

export function usePaneKeyboard(opts: PaneKeyboardOptions) {
  function handleKeydown(e: KeyboardEvent) {
    const sessionId = opts.getSessionId()
    if (!sessionId) return
    const currentPaneId = opts.getActivePaneId() || sessionId

    if (e.ctrlKey && e.shiftKey && e.key === 'D') {
      e.preventDefault()
      opts.splitPane(currentPaneId, 'horizontal')
    } else if (e.ctrlKey && e.shiftKey && e.key === 'E') {
      e.preventDefault()
      opts.splitPane(currentPaneId, 'vertical')
    } else if (e.ctrlKey && e.shiftKey && e.key === 'W') {
      e.preventDefault()
      if (opts.getPaneTree() && opts.getActivePaneId()) {
        opts.closePane(opts.getActivePaneId()!)
      }
    }
  }

  onMounted(() => document.addEventListener('keydown', handleKeydown))
  onUnmounted(() => document.removeEventListener('keydown', handleKeydown))

  return { handleKeydown }
}
