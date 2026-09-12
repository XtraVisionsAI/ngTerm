import { defineStore } from 'pinia'
import { computed, ref } from 'vue'

/**
 * Context the user collects from the terminal, the file explorer or the git
 * panel to send with the next AI message (UX-01).
 *
 * Every item is bound to where it came from: the tab whose agent it is meant
 * for, the pane/session it was captured in and that session's server. The
 * chat refuses to send an item whose server is not the one its agent runs
 * against, so a selection made in one split pane cannot be sent to another
 * server's agent by accident.
 */
export type AiContextKind = 'terminal' | 'file' | 'diff'

export interface AiContextItem {
  id: string
  kind: AiContextKind
  /** Short label shown on the chip (path, "终端选区", "git diff"). */
  title: string
  text: string
  tabId: string
  paneId: string
  sessionId: string
  serverId: string
  serverAlias: string
  createdAt: number
  /** Set once the user applied redaction; the count is what it masked. */
  redactedCount?: number
  /** Set when the user trimmed the text by hand. */
  edited?: boolean
}

/** Soft cap per item; the websocket frame limit is 1 MiB for the whole message. */
export const MAX_ITEM_CHARS = 200_000

let seq = 0

export const useAiContextStore = defineStore('aiContext', () => {
  const items = ref<AiContextItem[]>([])
  /** A prompt a source wants prefilled in the chat of `tabId` (e.g. "analyse this error"). */
  const pendingPrompt = ref(new Map<string, string>())

  const byTab = computed(() => {
    const m: Record<string, AiContextItem[]> = {}
    for (const it of items.value) (m[it.tabId] ||= []).push(it)
    return m
  })

  function forTab(tabId: string): AiContextItem[] {
    return byTab.value[tabId] || []
  }

  function add(item: Omit<AiContextItem, 'id' | 'createdAt'>): AiContextItem {
    const full: AiContextItem = {
      ...item,
      text: item.text.length > MAX_ITEM_CHARS ? item.text.slice(0, MAX_ITEM_CHARS) : item.text,
      id: `ctx-${Date.now()}-${++seq}`,
      createdAt: Date.now()
    }
    if (item.text.length > MAX_ITEM_CHARS) full.edited = true
    items.value.push(full)
    return full
  }

  function update(id: string, patch: Partial<Pick<AiContextItem, 'text' | 'redactedCount' | 'edited' | 'title'>>) {
    const it = items.value.find((i) => i.id === id)
    if (it) Object.assign(it, patch)
  }

  function remove(id: string) {
    items.value = items.value.filter((i) => i.id !== id)
  }

  function clearTab(tabId: string) {
    items.value = items.value.filter((i) => i.tabId !== tabId)
    pendingPrompt.value.delete(tabId)
  }

  /** Session ids are re-keyed when a pending pane connects; follow them. */
  function renameSession(oldId: string, newId: string) {
    for (const it of items.value) {
      if (it.tabId === oldId) it.tabId = newId
      if (it.paneId === oldId) it.paneId = newId
      if (it.sessionId === oldId) it.sessionId = newId
    }
    const p = pendingPrompt.value.get(oldId)
    if (p !== undefined) {
      pendingPrompt.value.set(newId, p)
      pendingPrompt.value.delete(oldId)
    }
  }

  function setPrompt(tabId: string, prompt: string) {
    pendingPrompt.value.set(tabId, prompt)
  }

  function takePrompt(tabId: string): string | undefined {
    const p = pendingPrompt.value.get(tabId)
    if (p !== undefined) pendingPrompt.value.delete(tabId)
    return p
  }

  return { items, pendingPrompt, forTab, add, update, remove, clearTab, renameSession, setPrompt, takePrompt }
})

const KIND_LABEL: Record<AiContextKind, string> = { terminal: '终端选区', file: '文件', diff: 'git diff' }

export function kindLabel(kind: AiContextKind): string {
  return KIND_LABEL[kind]
}

/**
 * Serialise the user's text plus context items into the single message the
 * agent receives. Both the native engine and CLI agents take a plain string,
 * so the context travels as clearly delimited fenced blocks with a header
 * naming its origin. Redaction is not applied here: the user does that (or
 * chooses not to) while reviewing.
 */
export function composeMessage(userText: string, ctx: AiContextItem[]): string {
  if (ctx.length === 0) return userText
  const parts: string[] = []
  if (userText) parts.push(userText)
  ctx.forEach((it, i) => {
    const origin = `${it.serverAlias || it.serverId} · 会话 ${it.sessionId.slice(0, 8)}`
    const fence = pickFence(it.text)
    const lang = it.kind === 'diff' ? 'diff' : it.kind === 'file' ? langFor(it.title) : 'text'
    parts.push(
      `--- 附加上下文 ${i + 1}/${ctx.length}：${kindLabel(it.kind)}${it.kind === 'file' ? ` ${it.title}` : ''}（${origin}）` +
        `${it.redactedCount ? `，已脱敏 ${it.redactedCount} 处` : ''}${it.edited ? '，已裁剪' : ''} ---\n` +
        `${fence}${lang}\n${it.text}\n${fence}`
    )
  })
  return parts.join('\n\n')
}

/** A fence longer than any run of backticks in the text. */
function pickFence(text: string): string {
  let longest = 0
  for (const m of text.matchAll(/`+/g)) longest = Math.max(longest, m[0].length)
  return '`'.repeat(Math.max(3, longest + 1))
}

function langFor(path: string): string {
  const ext = path.split('.').pop()?.toLowerCase() || ''
  const map: Record<string, string> = {
    rs: 'rust',
    ts: 'ts',
    js: 'js',
    vue: 'vue',
    py: 'python',
    sh: 'bash',
    bash: 'bash',
    zsh: 'bash',
    json: 'json',
    yaml: 'yaml',
    yml: 'yaml',
    toml: 'toml',
    md: 'md',
    conf: 'ini',
    ini: 'ini',
    service: 'ini',
    sql: 'sql',
    go: 'go',
    java: 'java',
    xml: 'xml',
    html: 'html',
    css: 'css'
  }
  return map[ext] || ''
}
