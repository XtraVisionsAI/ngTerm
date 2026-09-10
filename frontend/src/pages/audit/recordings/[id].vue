<script setup lang="ts">
  import type { ChunkProblem, RecordedEvent, RecordingMeta } from '@/utils/audit'
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

  const meta = ref<RecordingMeta | null>(null)
  const events = ref<RecordedEvent[]>([])
  const problems = ref<ChunkProblem[]>([])
  const loading = ref(true)
  const loadError = ref<string | null>(null)

  const playing = ref(false)
  const speed = ref(1)
  const position = ref(0) // ms into the recording
  const duration = computed(() => {
    const last = events.value.length ? events.value[events.value.length - 1].t : 0
    return Math.max(meta.value?.durationMs ?? 0, last, 1)
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

  /** Render everything up to `target` instantly, then continue from there. */
  function seek(target: number) {
    target = Math.max(0, Math.min(target, duration.value))
    if (target < position.value) {
      // Terminal state is not reversible: rebuild from the start.
      createTerminal(meta.value?.cols || 80, meta.value?.rows || 24)
      nextIndex = 0
    }
    while (nextIndex < events.value.length && events.value[nextIndex].t <= target) {
      apply(events.value[nextIndex])
      nextIndex++
    }
    position.value = target
  }

  function tick() {
    if (!playing.value) return
    const now = performance.now()
    const advance = (now - lastTick) * speed.value
    lastTick = now
    const target = position.value + advance
    if (target >= duration.value) {
      seek(duration.value)
      playing.value = false
      return
    }
    seek(target)
    timer = setTimeout(tick, 33)
  }

  function play() {
    if (position.value >= duration.value) seek(0)
    playing.value = true
    lastTick = performance.now()
    tick()
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
    else play()
  }

  function step(ms: number) {
    pause()
    seek(position.value + ms)
  }

  function onSlider(v: number) {
    const wasPlaying = playing.value
    pause()
    seek(v)
    if (wasPlaying) play()
  }

  // Timeline markers: where input happened, and where the stream has holes.
  const gapMarks = computed(() => events.value.filter((e) => e.k === 'g'))
  const inputCount = computed(() => events.value.filter((e) => e.k === 'i').length)
  const sliderMarks = computed(() => {
    const marks: Record<number, string> = {}
    for (const g of gapMarks.value) marks[g.t] = `丢失 ${g.n}`
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

  onMounted(async () => {
    try {
      const data = await api.get<{ recording: RecordingMeta; events: RecordedEvent[]; problems: ChunkProblem[] }>(
        `/audit/recordings/${recordingId}/events`
      )
      meta.value = data.recording
      events.value = data.events
      problems.value = data.problems
      createTerminal(data.recording.cols || 80, data.recording.rows || 24)
      const t = Number(route.query.t)
      if (Number.isFinite(t) && t > 0) {
        seek(t)
      }
    } catch (e) {
      loadError.value = (e as Error).message
      message.error(loadError.value)
    } finally {
      loading.value = false
    }
  })

  onBeforeUnmount(() => {
    pause()
    term?.dispose()
    term = null
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
        </template>
      </div>
      <div v-if="meta" class="text-xs opacity-70">
        开始于 {{ formatTime(meta.startedAt) }} · {{ meta.cols }}×{{ meta.rows }} · {{ meta.chunkCount }} 块 · 输入{{
          meta.inputPolicy === 'content' ? '含内容' : meta.inputPolicy === 'none' ? '未记录' : '仅记录时间与长度'
        }}
        <span v-if="inputCount"> · {{ inputCount }} 次输入</span>
      </div>
    </div>

    <n-alert v-if="loadError" type="error" class="mb-3">{{ loadError }}</n-alert>
    <n-alert v-if="problems.length" type="warning" class="mb-3" title="录像不完整">
      {{ problemText }}。缺失部分在回放中直接跳过，画面可能与实际不一致。
    </n-alert>
    <n-alert v-if="gapMarks.length && !problems.length" type="warning" class="mb-3">
      采集时有 {{ gapMarks.length }} 处事件丢失，时间轴上已标出；对应时刻的画面不可信。
    </n-alert>

    <div class="player-stage min-h-0 flex-1 overflow-auto rounded bg-black p-2">
      <div ref="container" class="inline-block" />
    </div>

    <div class="mt-3 flex items-center gap-3">
      <n-space size="small" align="center">
        <n-button size="small" :disabled="loading || !!loadError" @click="step(-5000)">−5s</n-button>
        <n-button size="small" type="primary" :disabled="loading || !!loadError" @click="toggle">
          {{ playing ? '暂停' : position >= duration ? '重放' : '播放' }}
        </n-button>
        <n-button size="small" :disabled="loading || !!loadError" @click="step(5000)">+5s</n-button>
        <n-select v-model:value="speed" :options="speedOptions" size="small" class="w-20" />
      </n-space>
      <span class="w-28 text-right text-xs font-mono">{{ formatMs(position) }} / {{ formatMs(duration) }}</span>
      <n-slider
        :value="position"
        :min="0"
        :max="duration"
        :step="100"
        :marks="sliderMarks"
        :format-tooltip="(v: number) => formatMs(v)"
        class="flex-1"
        @update:value="onSlider"
      />
    </div>
    <p class="mt-2 text-xs opacity-50">
      回放只渲染记录的输出，不会执行其中的脚本或响应终端查询；空格播放/暂停，←/→ 跳转 5 秒。
    </p>
  </div>
</template>

<style scoped>
  .player-stage :deep(.xterm) {
    padding: 0;
  }
</style>
