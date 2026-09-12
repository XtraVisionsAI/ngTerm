<script setup lang="ts">
  import { NDropdown } from 'naive-ui'
  import { computed, inject, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
  import { getAppTheme, getStoredThemeId } from '@/composables/terminalThemes'
  import { useApi } from '@/composables/useApi'
  import { postWithAdmission } from '@/composables/useSessionAdmission'
  import { useTerminal } from '@/composables/useTerminal'
  import { useWebSocket } from '@/composables/useWebSocket'
  import { useAuthStore } from '@/stores/auth'
  import { useSessionStore } from '@/stores/session'

  const props = defineProps<{
    sessionId: string
    serverId?: string
    active: boolean
    themeKey?: string
    createSessionFn?: (cols: number, rows: number) => Promise<{ id: string }>
  }>()

  const emit = defineEmits<{
    split: [direction: 'horizontal' | 'vertical']
    close: []
    'session-changed': [newId: string]
  }>()

  const auth = useAuthStore()
  const sessionStore = useSessionStore()
  const api = useApi()
  const containerRef = ref<HTMLElement | null>(null)
  /** Provided by the terminal page: queue text as AI context for this pane's tab. */
  const addAiContext = inject<((paneId: string, text: string, opts?: { analyze?: boolean }) => void) | undefined>(
    'addAiContext',
    undefined
  )
  const reconnectPaneSession = inject<(oldId: string, cols: number, rows: number) => Promise<{ id: string }>>(
    'reconnectPaneSession',
    undefined as any
  )

  const realSessionId = ref<string | null>(null)
  const connectError = ref<string | null>(null)
  const isDisconnected = ref(false)
  const isConnecting = computed(() => sessionStore.statuses[props.sessionId] === 'connecting')

  const ctxMenu = ref({ show: false, x: 0, y: 0 })
  const ctxOptions = ref<Array<{ label: string; key: string }>>([])
  const showSearch = ref(false)
  const searchQuery = ref('')
  const searchInputRef = ref<HTMLInputElement | null>(null)

  let ws: ReturnType<typeof useWebSocket> | null = null
  let reconnectTimer: ReturnType<typeof setTimeout> | null = null

  function scheduleAutoReconnect() {
    if (reconnectTimer) clearTimeout(reconnectTimer)
    reconnectTimer = setTimeout(() => {
      reconnectTimer = null
      if (isDisconnected.value) reconnect()
    }, 1500)
  }

  const { terminal, write, setTheme, getDimensions, findNext, findPrevious, clearSearch } = useTerminal(containerRef, {
    onData: (data) => {
      if (isDisconnected.value) {
        if (data.includes('\r') || data.includes('\n')) {
          reconnect()
        }
        return
      }
      if (!ws || ws.disconnected.value) return
      const bytes = new TextEncoder().encode(data)
      ws.send(bytes)
    },
    onResize: (cols, rows) => {
      ws?.resize(cols, rows)
    }
  })

  function openSearch() {
    showSearch.value = true
    nextTick(() => searchInputRef.value?.focus())
  }

  function closeSearch() {
    showSearch.value = false
    searchQuery.value = ''
    clearSearch()
  }

  function handleSearchKeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      closeSearch()
    } else if (e.key === 'Enter') {
      if (e.shiftKey) {
        findPrevious(searchQuery.value)
      } else {
        findNext(searchQuery.value)
      }
    }
  }

  function handleSearchInput() {
    if (searchQuery.value) {
      findNext(searchQuery.value)
    } else {
      clearSearch()
    }
  }

  function handleContextMenu(e: MouseEvent) {
    e.preventDefault()
    const items: Array<{ label: string; key: string } | { type: string; key: string }> = []
    if (terminal.value?.hasSelection()) {
      items.push({ label: '复制', key: 'copy' })
      if (addAiContext) {
        items.push({ label: '添加选区到 AI 上下文', key: 'aiContext' })
        items.push({ label: '让 AI 分析此报错', key: 'aiAnalyze' })
      }
    }
    items.push({ label: '粘贴', key: 'paste' })
    items.push({ label: '全选', key: 'selectAll' })
    items.push({ label: '清屏', key: 'clear' })
    items.push({ type: 'divider', key: 'd1' })
    items.push({ label: '向右分屏', key: 'splitH' })
    items.push({ label: '向下分屏', key: 'splitV' })
    items.push({ label: '关闭面板', key: 'closePane' })
    ctxOptions.value = items as any
    ctxMenu.value = { show: true, x: e.clientX, y: e.clientY }
  }

  async function handleCtxSelect(key: string) {
    ctxMenu.value.show = false
    const term = terminal.value
    if (!term) return
    switch (key) {
      case 'copy':
        await navigator.clipboard.writeText(term.getSelection())
        term.clearSelection()
        break
      case 'paste': {
        const text = await navigator.clipboard.readText()
        term.paste(text)
        break
      }
      case 'aiContext':
      case 'aiAnalyze': {
        const text = term.getSelection()
        if (text.trim() && addAiContext) {
          addAiContext(realSessionId.value || props.sessionId, text, { analyze: key === 'aiAnalyze' })
        }
        term.clearSelection()
        break
      }
      case 'selectAll':
        term.selectAll()
        break
      case 'clear':
        term.clear()
        break
      case 'splitH':
        emit('split', 'horizontal')
        break
      case 'splitV':
        emit('split', 'vertical')
        break
      case 'closePane':
        emit('close')
        break
    }
  }

  watch(
    () => props.themeKey,
    (key) => {
      if (key) setTheme(key)
    }
  )

  const bgColor = computed(() => getAppTheme(props.themeKey || getStoredThemeId()).terminal.background || '#000')

  onMounted(async () => {
    if (props.createSessionFn || props.sessionId.startsWith('pending-')) {
      await createSession()
    } else {
      realSessionId.value = props.sessionId
      startWebSocket()
    }
  })

  async function createSession() {
    try {
      const { cols, rows } = getDimensions()
      let session: { id: string; serverId?: string; serverAlias?: string; serverHost?: string; aiToolId?: string }
      if (props.createSessionFn) {
        session = await props.createSessionFn(cols, rows)
      } else {
        session = await postWithAdmission<{
          id: string
          serverId: string
          serverAlias: string
          serverHost: string
          aiToolId?: string
        }>(api, '/sessions', { serverId: props.serverId, cols, rows }, (requestId, msg) => {
          terminal.value?.write(
            `\r\n\x1B[93m${msg}\x1B[0m\r\n\x1B[90m审批申请 ${requestId}，批准后将自动连接…\x1B[0m\r\n`
          )
        })
        sessionStore.updateTabSession(props.sessionId, session as any)
      }
      realSessionId.value = session.id
      startWebSocket()
    } catch (e: any) {
      connectError.value = e.message || '连接失败'
      sessionStore.setStatus(props.sessionId, 'disconnected')
      if (terminal.value) {
        terminal.value.write(`\r\n\x1B[91m连接失败: ${connectError.value}\x1B[0m\r\n`)
      }
    }
  }

  function startWebSocket() {
    const sid = realSessionId.value!
    ws = useWebSocket(sid, auth.token!)

    ws.onOutput((data) => {
      write(data)
    })

    ws.onReset(() => {
      terminal.value?.reset()
    })

    ws.onSessionDisconnect(() => {
      const id = realSessionId.value || props.sessionId
      sessionStore.setStatus(id, 'disconnected')
      isDisconnected.value = true
      if (terminal.value) {
        terminal.value.options.cursorBlink = false
        terminal.value.write('\r\n\x1B[90m[连接已断开，正在重新连接...]\x1B[0m\r\n')
      }
      scheduleAutoReconnect()
    })

    watch(
      () => ws!.reconnecting.value,
      (val) => {
        if (val) {
          const id = realSessionId.value || props.sessionId
          sessionStore.setStatus(id, 'reconnecting')
        }
      }
    )

    watch(
      () => ws!.connected.value,
      (val) => {
        if (val) {
          const id = realSessionId.value || props.sessionId
          sessionStore.setStatus(id, 'connected')
          const { cols, rows } = getDimensions()
          ws!.resize(cols, rows)
        }
      }
    )

    ws.connect()
  }

  async function reconnect() {
    ws?.close()
    ws = null
    isDisconnected.value = false
    connectError.value = null
    if (terminal.value) {
      terminal.value.clear()
      terminal.value.options.cursorBlink = true
    }
    if (props.createSessionFn) {
      await createSession()
      if (realSessionId.value) {
        emit('session-changed', realSessionId.value)
      }
    } else if (reconnectPaneSession) {
      try {
        const { cols, rows } = getDimensions()
        const session = await reconnectPaneSession(realSessionId.value || props.sessionId, cols, rows)
        realSessionId.value = session.id
        startWebSocket()
      } catch (e: any) {
        connectError.value = e.message || '重连失败'
        isDisconnected.value = true
      }
    } else {
      startWebSocket()
    }
  }

  onBeforeUnmount(() => {
    if (reconnectTimer) clearTimeout(reconnectTimer)
    ws?.close()
  })

  watch(terminal, (term) => {
    if (term) {
      term.attachCustomKeyEventHandler((e) => {
        if (e.ctrlKey && e.key === 'f' && e.type === 'keydown') {
          openSearch()
          return false
        }
        return true
      })
    }
  })

  function pasteCommand(command: string, autoExec = false) {
    const term = terminal.value
    if (!term) return
    term.paste(autoExec ? `${command}\n` : command)
  }

  defineExpose({ pasteCommand, reconnect })
</script>

<template>
  <div class="relative h-full w-full p-2" :style="{ background: bgColor }" @contextmenu="handleContextMenu">
    <div ref="containerRef" class="h-full w-full" />
    <!-- Search overlay -->
    <div
      v-if="showSearch"
      class="absolute right-4 top-2 z-20 flex items-center gap-1 border border-om-border rounded bg-om-panel px-2 py-1 shadow"
    >
      <input
        ref="searchInputRef"
        v-model="searchQuery"
        class="w-40 bg-transparent text-xs text-om-text outline-none placeholder:text-om-dimmed"
        placeholder="搜索..."
        @input="handleSearchInput"
        @keydown="handleSearchKeydown"
      />
      <button class="p-0.5 text-om-dimmed hover:text-om-text" title="上一个" @click="findPrevious(searchQuery)">
        <i class="i-ri:arrow-up-s-line block size-3.5" />
      </button>
      <button class="p-0.5 text-om-dimmed hover:text-om-text" title="下一个" @click="findNext(searchQuery)">
        <i class="i-ri:arrow-down-s-line block size-3.5" />
      </button>
      <button class="p-0.5 text-om-dimmed hover:text-om-text" title="关闭" @click="closeSearch">
        <i class="i-ri:close-line block size-3.5" />
      </button>
    </div>
    <!-- Connecting state -->
    <div v-if="isConnecting && !connectError" class="absolute inset-0 z-10 flex items-center justify-center">
      <span class="text-sm text-om-dimmed">正在连接...</span>
    </div>
    <!-- Reconnecting overlay -->
    <div v-if="ws?.reconnecting.value" class="absolute inset-0 z-10 flex items-center justify-center bg-black/60">
      <div class="flex items-center gap-2 text-sm text-white">
        <span>正在重连...</span>
      </div>
    </div>
    <!-- Disconnected overlay -->
    <div v-else-if="isDisconnected" class="absolute inset-0 z-10 flex items-center justify-center pb-[20%] pt-0">
      <div
        class="w-64 flex flex-col items-center gap-3 border border-om-border rounded-xl bg-om-panel/95 px-12 py-6 shadow-xl backdrop-blur-sm"
      >
        <i class="i-ri:link-unlink block size-8 text-om-dimmed" />
        <span class="text-sm text-om-text font-medium">连接已断开</span>
        <button
          class="mt-1 border border-om-border rounded-md bg-om-bg px-5 py-1.5 text-sm text-om-text shadow-sm transition hover:border-om-primary hover:bg-om-hover"
          @click="reconnect"
        >
          重新连接
        </button>
        <span class="text-xs text-om-dimmed">按 Enter 快速重连</span>
      </div>
    </div>
    <n-dropdown
      trigger="manual"
      placement="bottom-start"
      :show="ctxMenu.show"
      :x="ctxMenu.x"
      :y="ctxMenu.y"
      :options="ctxOptions"
      @select="handleCtxSelect"
      @clickoutside="ctxMenu.show = false"
    />
  </div>
</template>
