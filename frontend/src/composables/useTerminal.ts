import type { Ref } from 'vue'
import { FitAddon } from '@xterm/addon-fit'
import { SearchAddon } from '@xterm/addon-search'
import { WebLinksAddon } from '@xterm/addon-web-links'
import { WebglAddon } from '@xterm/addon-webgl'
import { Terminal } from '@xterm/xterm'
import { onBeforeUnmount, onMounted, ref } from 'vue'
import { getAppTheme, getStoredThemeId } from './terminalThemes'

export function useTerminal(
  containerRef: Ref<HTMLElement | null>,
  options?: {
    onData?: (data: string) => void
    onResize?: (cols: number, rows: number) => void
  }
) {
  const terminal = ref<Terminal | null>(null)
  let fitAddon: FitAddon | null = null
  let searchAddon: SearchAddon | null = null

  function init() {
    if (!containerRef.value) return

    const term = new Terminal({
      fontSize: 14,
      fontFamily: '"FiraCode Nerd Font Mono", Menlo, Monaco, "Courier New", monospace',
      theme: getAppTheme(getStoredThemeId()).terminal,
      cursorBlink: true,
      allowProposedApi: true
    })

    fitAddon = new FitAddon()
    searchAddon = new SearchAddon()
    term.loadAddon(fitAddon)
    term.loadAddon(searchAddon)
    term.loadAddon(new WebLinksAddon())

    term.open(containerRef.value)

    try {
      term.loadAddon(new WebglAddon())
    } catch {
      // WebGL not supported, fall back to canvas
    }

    fitAddon.fit()

    if (options?.onData) {
      term.onData(options.onData)
    }

    if (options?.onResize) {
      term.onResize(({ cols, rows }) => options.onResize!(cols, rows))
    }

    terminal.value = term

    const resizeObserver = new ResizeObserver(() => {
      fitAddon?.fit()
    })
    resizeObserver.observe(containerRef.value)
  }

  function write(data: Uint8Array | string) {
    terminal.value?.write(data)
  }

  function setTheme(key: string) {
    if (terminal.value) {
      terminal.value.options.theme = getAppTheme(key).terminal
    }
  }

  function dispose() {
    terminal.value?.dispose()
    terminal.value = null
  }

  function getDimensions() {
    const term = terminal.value
    if (term) return { cols: term.cols, rows: term.rows }
    return { cols: 80, rows: 24 }
  }

  function findNext(query: string): boolean {
    if (!searchAddon || !query) return false
    return searchAddon.findNext(query)
  }

  function findPrevious(query: string): boolean {
    if (!searchAddon || !query) return false
    return searchAddon.findPrevious(query)
  }

  function clearSearch() {
    searchAddon?.clearDecorations()
  }

  onMounted(init)
  onBeforeUnmount(dispose)

  return { terminal, write, setTheme, dispose, getDimensions, findNext, findPrevious, clearSearch }
}
