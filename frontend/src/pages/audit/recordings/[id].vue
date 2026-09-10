<script setup lang="ts">
  import type { ChunkProblem, RecordedEvent, RecordingChunk, RecordingMeta } from '@/utils/audit'
  import { Terminal } from '@xterm/xterm'
  import { NAlert, NButton, NSelect, NSlider, NSpace, NTag, NTooltip, useMessage } from 'naive-ui'
  import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
  import { useRoute, useRouter } from 'vue-router'
  import { getAppTheme, getStoredThemeId } from '@/composables/terminalThemes'
  import { useApi } from '@/composables/useApi'
  import { formatMs, integrityInfo } from '@/utils/audit'
  import { formatTime } from '@/utils/format'

  const route = useRoute()
  const router = useRouter()
  const api = useApi()
  const message = useMessage()

  const recordingId = (route.params as { id: string }).id
  const container = ref<HTMLElement | null>(null)

  // --- Progressive loading -------------------------------------------------
  // A recording is stored as ordered chunks. The player fetches them in
  // batches: enough to start, then ahead of the playhead, then whatever a
  // seek needs. Terminal state is cumulative, so a seek to time T requires
  // every chunk before T; nothing is skipped.
  const BATCH = 8
  /** Prefetch when the loaded part ends this close (in playback ms) to the playhead. */
  const PREFETCH_AHEAD_MS = 15_000

  interface EventsResponse {
    recording: RecordingMeta
    events: RecordedEvent[]
    problems: ChunkProblem[]
    chunks: RecordingChunk[]
    chunksRead: number
  }

  const meta = ref<RecordingMeta | null>(null)
  const chunks = ref<RecordingChunk[]>([])
  const problems = ref<ChunkProblem[]>([])
  /** Events are kept outside reactivity: they can number in the millions. */
  let events: RecordedEvent[] = []
  const eventCount = ref(0)
  /** Highest chunk `seq` whose events are in `events`. */
  const loadedThroughSeq = ref(0)
  const loading = ref(true)
  const loadingMore = ref(false)
  const buffering = ref(false)
  const loadError = ref<string | null>(null)
  let inflight: Promise<void> | null = null
  /** Seek target that could not be honoured because loading failed. */
  let pendingSeek: number | null = null
  /** Gap markers and input count of the loaded part (timeline annotations). */
  const gapMarks = ref<RecordedEvent[]>([])
  const inputCount = ref(0)

  const lastSeq = computed(() => (chunks.value.length ? chunks.value[chunks.value.length - 1].seq : 0))
  const fullyLoaded = computed(() => chunks.value.length > 0 && loadedThroughSeq.value >= lastSeq.value)
  /** End of the loaded part on the recording's clock. */
  const loadedEndMs = computed(() => {
    let end = 0
    for (const c of chunks.value) if (c.seq <= loadedThroughSeq.value) end = Math.max(end, c.endMs)
    return end
  })
  const loadedChunkCount = computed(() => chunks.value.filter((c) => c.seq <= loadedThroughSeq.value).length)

  function mergeProblems(incoming: ChunkProblem[]) {
    const key = (p: ChunkProblem) => `${p.kind}:${'seq' in p ? p.seq : p.expected_seq}`
    const seen = new Set(problems.value.map(key))
    for (const p of incoming) {
      const k = key(p)
      if (!seen.has(k)) {
        seen.add(k)
        problems.value.push(p)
      }
    }
  }

  function appendEvents(incoming: RecordedEvent[]) {
    for (const ev of incoming) {
      events.push(ev)
      if (ev.k === 'g') gapMarks.value.push(ev)
      else if (ev.k === 'i') inputCount.value++
    }
    eventCount.value = events.length
  }

  /** Load the next batch of chunks after `loadedThroughSeq`. Serialised. */
  function loadNextBatch(): Promise<void> {
    if (inflight) return inflight
    inflight = (async () => {
      loadingMore.value = true
      try {
        const fromSeq = loadedThroughSeq.value + 1
        const toSeq = fromSeq + BATCH - 1
        const data = await api.get<EventsResponse>(
          `/audit/recordings/${recordingId}/events?fromSeq=${fromSeq}&toSeq=${toSeq}`
        )
        if (!meta.value) {
          meta.value = data.recording
          createTerminal(data.recording.cols || 80, data.recording.rows || 24)
        }
        chunks.value = data.chunks
        mergeProblems(data.problems)
        appendEvents(data.events)
        // Chunks beyond the last one do not exist; clamp so `fullyLoaded` is truthful.
        const last = data.chunks.length ? data.chunks[data.chunks.length - 1].seq : 0
        loadedThroughSeq.value = Math.min(toSeq, Math.max(last, loadedThroughSeq.value))
        loadError.value = null
      } catch (e) {
        loadError.value = (e as Error).message
        throw e
      } finally {
        loadingMore.value = false
        inflight = null
      }
    })()
    return inflight
  }

  /** Make sure every chunk up to and including time `ms` is loaded. */
  async function ensureLoadedUntil(ms: number) {
    while (!fullyLoaded.value && loadedEndMs.value < ms) {
      await loadNextBatch()
    }
  }

  async function retryLoad() {
    loadError.value = null
    try {
      await loadNextBatch()
      if (pendingSeek !== null) {
        const t = pendingSeek
        pendingSeek = null
        await seek(t)
      }
    } catch {
      /* shown via loadError */
    }
  }

  // --- Playback --------------------------------------------------------------
  const playing = ref(false)
  const speed = ref(1)
  const position = ref(0) // ms into the recording
  const duration = computed(() => {
    const lastChunk = chunks.value.length ? chunks.value[chunks.value.length - 1].endMs : 0
    return Math.max(meta.value?.durationMs ?? 0, lastChunk, 1)
  })

  const speedOptions = [0.5, 1, 2, 4, 8].map((v) => ({ label: `${v}x`, value: v }))

  // Playback is read-only rendering: the terminal never accepts input, never
  // opens links, and whatever the recording contains is only ever drawn.
  let term: Terminal | null = null
  let nextIndex = 0
  let timer: ReturnType<typeof setTimeout> | null = null
  let lastTick = 0

  function decode(b64: string): Uint8Array {
    const bin = atob(b64)
    const out = new Uint8Array(bin.length)
    for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i)
    return out
  }

  function createTerminal(cols: number, rows: number) {
    term?.dispose()
    term = new Terminal({
      cols,
      rows,
      disableStdin: true,
      cursorBlink: false,
      convertEol: false,
      scrollback: 5000,
      allowProposedApi: false,
      fontSize: 13,
      fontFamily: '"FiraCode Nerd Font Mono", Menlo, Monaco, "Courier New", monospace',
      theme: getAppTheme(getStoredThemeId()).terminal
    })
    if (container.value) term.open(container.value)
  }

  function apply(ev: RecordedEvent) {
    if (!term) return
    switch (ev.k) {
      case 'o':
        if (ev.d) term.write(decode(ev.d))
        break
      case 'r':
        if (ev.c && ev.r) term.resize(ev.c, ev.r)
        break
      default:
        // input metadata, gaps and markers are shown on the timeline only
        break
    }
  }

  /** Apply loaded events up to `target` without fetching anything. */
  function applyUntil(target: number) {
    if (target < position.value) {
      // Terminal state is not reversible: rebuild from the start.
      createTerminal(meta.value?.cols || 80, meta.value?.rows || 24)
      nextIndex = 0
    }
    while (nextIndex < events.length && events[nextIndex].t <= target) {
      apply(events[nextIndex])
      nextIndex++
    }
    position.value = target
  }

  /**
   * Render everything up to `target`, fetching chunks first if needed. While
   * chunks load the player shows "buffering"; if loading fails the playhead
   * stops at the loaded edge and the failure is shown, never an empty screen
   * pretending the recording ended there.
   */
  async function seek(target: number) {
    target = Math.max(0, Math.min(target, duration.value))
    if (!fullyLoaded.value && loadedEndMs.value < target) {
      buffering.value = true
      try {
        await ensureLoadedUntil(target)
      } catch {
        pendingSeek = target
        pause()
        applyUntil(Math.min(target, loadedEndMs.value))
        return
      } finally {
        buffering.value = false
      }
    }
    applyUntil(target)
  }

  function prefetchIfClose() {
    if (fullyLoaded.value || loadingMore.value || loadError.value) return
    if (loadedEndMs.value - position.value < PREFETCH_AHEAD_MS * speed.value) {
      loadNextBatch().catch(() => {
        /* surfaced via loadError; playback stops at the loaded edge */
      })
    }
  }

  async function tick() {
    if (!playing.value) return
    const now = performance.now()
    const advance = (now - lastTick) * speed.value
    lastTick = now
    const target = position.value + advance
    if (target >= duration.value) {
      await seek(duration.value)
      playing.value = false
      return
    }
    prefetchIfClose()
    await seek(target)
    if (!playing.value) return
    // A seek that had to buffer took real time; do not fast-forward to catch up.
    lastTick = performance.now()
    timer = setTimeout(tick, 33)
  }

  async function play() {
    if (position.value >= duration.value) await seek(0)
    playing.value = true
    lastTick = performance.now()
    void tick()
  }

  function pause() {
    playing.value = false
    if (timer) {
      clearTimeout(timer)
      timer = null
    }
  }

  function toggle() {
    if (playing.value) pause()
    else void play()
  }

  function step(ms: number) {
    pause()
    void seek(position.value + ms)
  }

  async function onSlider(v: number) {
    const wasPlaying = playing.value
    pause()
    await seek(v)
    if (wasPlaying && !loadError.value) void play()
  }

  // Timeline markers from the loaded part: where input happened, and where
  // the stream has holes. Labelled as partial until everything is loaded.
  const sliderMarks = computed(() => {
    const marks: Record<number, string> = {}
    for (const g of gapMarks.value) marks[g.t] = `丢失 ${g.n}`
    if (!fullyLoaded.value && loadedEndMs.value > 0) marks[loadedEndMs.value] = '已加载至此'
    return marks
  })

  const integrity = computed(() => integrityInfo(meta.value?.integrity))
  const problemText = computed(() =>
    problems.value
      .map((p) => {
        if (p.kind === 'missing') return `第 ${p.seq} 块文件缺失`
        if (p.kind === 'corrupt') return `第 ${p.seq} 块校验失败（${p.detail}）`
        return `索引缺少第 ${p.expected_seq} 块`
      })
      .join('；')
  )
  const controlsDisabled = computed(() => loading.value || (!meta.value && !!loadError.value))

  onMounted(async () => {
    try {
      await loadNextBatch()
      const t = Number(route.query.t)
      if (Number.isFinite(t) && t > 0) {
        await seek(t)
      }
    } catch (e) {
      message.error((e as Error).message)
    } finally {
      loading.value = false
    }
  })

  onBeforeUnmount(() => {
    pause()
    term?.dispose()
    term = null
    events = []
  })

  watch(speed, () => {
    lastTick = performance.now()
  })

  function onKey(e: KeyboardEvent) {
    if (e.target instanceof HTMLInputElement) return
    if (e.code === 'Space') {
      e.preventDefault()
      toggle()
    } else if (e.code === 'ArrowLeft') {
      step(-5000)
    } else if (e.code === 'ArrowRight') {
      step(5000)
    }
  }
</script>

<template>
  <div class="h-full flex flex-col p-4" tabindex="0" @keydown="onKey">
    <div class="mb-3 flex flex-wrap items-center justify-between gap-2">
      <div class="flex items-center gap-3">
        <n-button size="small" quaternary @click="router.push('/audit')">← 返回审计</n-button>
        <h2 class="text-lg font-bold">终端录像回放</h2>
        <template v-if="meta">
          <n-tooltip>
            <template #trigger>
              <n-tag size="small" :type="integrity.type">{{ integrity.label }}</n-tag>
            </template>
            {{ integrity.detail || '采集期间没有丢失事件' }}
          </n-tooltip>
          <n-tag v-if="meta.status === 'interrupted'" size="small" type="warning">录制被中断</n-tag>
          <n-tag v-if="meta.status === 'recording'" size="small" type="info">仍在录制，内容不完整</n-tag>
          <n-tag v-if="buffering" size="small" type="info" role="status">缓冲中…</n-tag>
          <n-tag v-else-if="loadingMore" size="small" role="status">预取中…</n-tag>
        </template>
      </div>
      <div v-if="meta" class="text-xs opacity-70" aria-live="polite">
        开始于 {{ formatTime(meta.startedAt) }} · {{ meta.cols }}×{{ meta.rows }} · 已加载 {{ loadedChunkCount }}/{{
          chunks.length
        }}
        块（{{ eventCount }} 个事件） · 输入{{
          meta.inputPolicy === 'content' ? '含内容' : meta.inputPolicy === 'none' ? '未记录' : '仅记录时间与长度'
        }}
        <span v-if="inputCount"> · {{ inputCount }} 次输入{{ fullyLoaded ? '' : '（已加载部分）' }}</span>
      </div>
    </div>

    <n-alert v-if="loadError" type="error" class="mb-3" :title="meta ? '后续内容加载失败' : '录像加载失败'">
      {{ loadError }}。
      <template v-if="meta"
        >回放停在已加载的 {{ formatMs(loadedEndMs) }} 处；之后的内容并非为空，只是尚未取到。</template
      >
      <n-button size="tiny" class="ml-2" @click="retryLoad">重试</n-button>
    </n-alert>
    <n-alert v-if="problems.length" type="warning" class="mb-3" title="录像不完整">
      {{ problemText }}。缺失部分在回放中直接跳过，画面可能与实际不一致。
    </n-alert>
    <n-alert v-if="gapMarks.length && !problems.length" type="warning" class="mb-3">
      采集时有 {{ gapMarks.length }} 处事件丢失{{
        fullyLoaded ? '' : '（仅统计已加载部分）'
      }}，时间轴上已标出；对应时刻的画面不可信。
    </n-alert>

    <div class="player-stage min-h-0 flex-1 overflow-auto rounded bg-black p-2">
      <div ref="container" class="inline-block" />
    </div>

    <div class="mt-3 flex items-center gap-3">
      <n-space size="small" align="center">
        <n-button size="small" :disabled="controlsDisabled" @click="step(-5000)">−5s</n-button>
        <n-button size="small" type="primary" :disabled="controlsDisabled" @click="toggle">
          {{ playing ? '暂停' : position >= duration ? '重放' : '播放' }}
        </n-button>
        <n-button size="small" :disabled="controlsDisabled" @click="step(5000)">+5s</n-button>
        <n-select v-model:value="speed" :options="speedOptions" size="small" class="w-20" />
      </n-space>
      <span class="w-28 text-right text-xs font-mono">{{ formatMs(position) }} / {{ formatMs(duration) }}</span>
      <n-slider
        :value="position"
        :min="0"
        :max="duration"
        :step="100"
        :marks="sliderMarks"
        :disabled="controlsDisabled"
        :format-tooltip="(v: number) => formatMs(v)"
        class="flex-1"
        @update:value="onSlider"
      />
    </div>
    <p class="mt-2 text-xs opacity-50">
      回放只渲染记录的输出，不会执行其中的脚本或响应终端查询；长录像按块按需加载，拖动到未加载位置时会先补齐之前的内容。空格播放/暂停，←/→
      跳转 5 秒。
    </p>
  </div>
</template>

<style scoped>
  .player-stage :deep(.xterm) {
    padding: 0;
  }
</style>
