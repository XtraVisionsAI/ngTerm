import { ref } from 'vue'
import { useAuthStore } from '@/stores/auth'

const BASE_DELAY = 1000
const MAX_DELAY = 30000
const AUTH_CHECK_AFTER_FAILURES = 3

export function useWebSocket(sessionId: string, token: string) {
  const auth = useAuthStore()
  const connected = ref(false)
  const reconnecting = ref(false)
  const disconnected = ref(false)
  let ws: WebSocket | null = null
  let onData: ((data: Uint8Array) => void) | null = null
  let onDisconnect: (() => void) | null = null
  let onResetCb: (() => void) | null = null
  let retryCount = 0
  let consecutiveFailures = 0
  let retryTimer: ReturnType<typeof setTimeout> | null = null
  let intentionalClose = false
  let didOpen = false

  function buildUrl() {
    const protocol = location.protocol === 'https:' ? 'wss:' : 'ws:'
    return `${protocol}//${location.host}/ws/terminal/${sessionId}?token=${token}`
  }

  function connect() {
    intentionalClose = false
    didOpen = false
    ws = new WebSocket(buildUrl())

    ws.onopen = () => {
      connected.value = true
      reconnecting.value = false
      retryCount = 0
      consecutiveFailures = 0
      didOpen = true
    }

    ws.onclose = () => {
      connected.value = false
      ws = null
      if (!didOpen) {
        consecutiveFailures++
      }
      if (!intentionalClose && !disconnected.value) {
        if (consecutiveFailures >= AUTH_CHECK_AFTER_FAILURES) {
          checkAuthAndMaybeReconnect()
        } else {
          scheduleReconnect()
        }
      }
    }

    ws.onerror = () => {
      // onclose will fire after this
    }

    ws.onmessage = (event) => {
      try {
        const msg = JSON.parse(event.data)
        if (msg.type === 'output' && msg.data && onData) {
          const bytes = Uint8Array.from(atob(msg.data), (c) => c.charCodeAt(0))
          onData(bytes)
        } else if (msg.type === 'reset') {
          // Server is about to replay its retained scrollback (initial
          // connect, reconnect, or after we fell behind): drop what we have
          // so the replay does not duplicate it.
          onResetCb?.()
        } else if (msg.type === 'disconnected') {
          disconnected.value = true
          intentionalClose = true
          onDisconnect?.()
        }
      } catch {}
    }
  }

  async function checkAuthAndMaybeReconnect() {
    const valid = await auth.check()
    if (!valid) {
      disconnected.value = true
      window.location.hash = '#/login'
    } else {
      consecutiveFailures = 0
      scheduleReconnect()
    }
  }

  function scheduleReconnect() {
    reconnecting.value = true
    const delay = Math.min(BASE_DELAY * 2 ** retryCount, MAX_DELAY)
    retryCount++

    retryTimer = setTimeout(() => {
      connect()
    }, delay)
  }

  function send(data: Uint8Array) {
    if (ws?.readyState === WebSocket.OPEN) {
      const b64 = btoa(String.fromCharCode(...data))
      ws.send(JSON.stringify({ type: 'input', data: b64 }))
    }
  }

  function resize(cols: number, rows: number) {
    if (ws?.readyState === WebSocket.OPEN) {
      ws.send(JSON.stringify({ type: 'resize', cols, rows }))
    }
  }

  function close() {
    intentionalClose = true
    if (retryTimer) {
      clearTimeout(retryTimer)
      retryTimer = null
    }
    if (ws && ws.readyState === WebSocket.OPEN) {
      ws.close()
    }
    ws = null
    connected.value = false
    reconnecting.value = false
  }

  function onOutput(fn: (data: Uint8Array) => void) {
    onData = fn
  }

  function onSessionDisconnect(fn: () => void) {
    onDisconnect = fn
  }

  function onReset(fn: () => void) {
    onResetCb = fn
  }

  return {
    connected,
    reconnecting,
    disconnected,
    connect,
    send,
    resize,
    close,
    onOutput,
    onReset,
    onSessionDisconnect
  }
}
