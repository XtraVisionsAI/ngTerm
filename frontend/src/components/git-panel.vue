<script setup lang="ts">
  import { NButton, NEmpty, NSpin, NTag } from 'naive-ui'
  import { onMounted, ref, watch } from 'vue'
  import { useApi } from '@/composables/useApi'

  const props = defineProps<{
    sessionId: string
  }>()

  const api = useApi()

  interface GitFile {
    status: string
    file: string
  }

  interface GitCommit {
    hash: string
    shortHash: string
    message: string
    author: string
    relativeDate: string
    refs: string
  }

  interface GitBranch {
    name: string
    current: boolean
    upstream: string
  }

  const currentBranch = ref('')
  const files = ref<GitFile[]>([])
  const commits = ref<GitCommit[]>([])
  const branches = ref<GitBranch[]>([])
  const diff = ref('')
  const loading = ref(false)
  const activeView = ref<'status' | 'log' | 'branches'>('status')
  const showDiff = ref(false)

  onMounted(loadStatus)

  watch(
    () => props.sessionId,
    () => {
      if (!props.sessionId.startsWith('pending-')) loadStatus()
    }
  )

  async function loadStatus() {
    if (props.sessionId.startsWith('pending-')) return
    loading.value = true
    try {
      const result = await api.get<{ branch: string; files: GitFile[] }>(`/sessions/${props.sessionId}/git/status`)
      currentBranch.value = result.branch
      files.value = result.files
    } catch {
      currentBranch.value = ''
      files.value = []
    } finally {
      loading.value = false
    }
  }

  async function loadLog() {
    loading.value = true
    try {
      commits.value = await api.get<GitCommit[]>(`/sessions/${props.sessionId}/git/log?limit=30`)
    } catch {
      commits.value = []
    } finally {
      loading.value = false
    }
  }

  async function loadBranches() {
    loading.value = true
    try {
      branches.value = await api.get<GitBranch[]>(`/sessions/${props.sessionId}/git/branches`)
    } catch {
      branches.value = []
    } finally {
      loading.value = false
    }
  }

  async function loadDiff() {
    try {
      const result = await api.get<{ diff: string }>(`/sessions/${props.sessionId}/git/diff`)
      diff.value = result.diff
      showDiff.value = true
    } catch {
      diff.value = ''
    }
  }

  function switchView(view: 'status' | 'log' | 'branches') {
    activeView.value = view
    showDiff.value = false
    if (view === 'status') loadStatus()
    else if (view === 'log') loadLog()
    else loadBranches()
  }

  function statusColor(status: string): 'success' | 'warning' | 'error' | 'info' {
    if (status === 'M' || status === 'MM') return 'warning'
    if (status === 'A' || status === '??') return 'success'
    if (status === 'D') return 'error'
    return 'info'
  }

  function statusLabel(status: string): string {
    if (status === 'M' || status === 'MM') return 'M'
    if (status === 'A') return 'A'
    if (status === 'D') return 'D'
    if (status === '??') return 'U'
    if (status === 'R') return 'R'
    return status
  }
</script>

<template>
  <div class="h-full flex flex-col bg-om-bg text-om-text">
    <!-- Header -->
    <div class="flex items-center gap-1 border-b border-om-border px-3 py-2">
      <n-button
        v-for="view in ['status', 'log', 'branches'] as const"
        :key="view"
        size="tiny"
        :type="activeView === view ? 'primary' : 'default'"
        quaternary
        @click="switchView(view)"
      >
        {{ view === 'status' ? '状态' : view === 'log' ? '日志' : '分支' }}
      </n-button>
      <div class="flex-1" />
      <n-tag v-if="currentBranch" size="small" type="info">{{ currentBranch }}</n-tag>
    </div>

    <!-- Content -->
    <div class="min-h-0 flex-1 overflow-y-auto">
      <div v-if="loading" class="flex items-center justify-center p-4">
        <n-spin size="small" />
      </div>

      <!-- Status view -->
      <template v-else-if="activeView === 'status' && !showDiff">
        <div v-if="files.length === 0" class="p-4">
          <n-empty description="工作区干净" size="small" />
        </div>
        <template v-else>
          <div class="flex items-center justify-between border-b border-om-border px-3 py-1.5">
            <span class="text-xs text-om-dimmed">{{ files.length }} 个文件变更</span>
            <n-button size="tiny" quaternary @click="loadDiff">查看 Diff</n-button>
          </div>
          <div
            v-for="file in files"
            :key="file.file"
            class="flex items-center gap-2 px-3 py-1 text-xs hover:bg-om-hover"
          >
            <n-tag :type="statusColor(file.status)" size="small" class="w-5 text-center">
              {{ statusLabel(file.status) }}
            </n-tag>
            <span class="flex-1 truncate font-mono">{{ file.file }}</span>
          </div>
        </template>
      </template>

      <!-- Diff view -->
      <template v-else-if="showDiff">
        <div class="flex items-center border-b border-om-border px-3 py-1.5">
          <n-button size="tiny" quaternary @click="showDiff = false">← 返回</n-button>
        </div>
        <pre class="overflow-auto p-3 text-xs leading-5 font-mono">
<template v-for="(line, i) in diff.split('\n')" :key="i"><span :class="line.startsWith('+') ? 'text-om-success' : line.startsWith('-') ? 'text-om-danger' : line.startsWith('@@') ? 'text-om-primary' : 'text-om-text'">{{ line }}
</span></template></pre>
      </template>

      <!-- Log view -->
      <template v-else-if="activeView === 'log'">
        <div v-if="commits.length === 0" class="p-4">
          <n-empty description="无提交记录" size="small" />
        </div>
        <div v-for="commit in commits" :key="commit.hash" class="border-b border-om-border px-3 py-2 hover:bg-om-hover">
          <div class="flex items-center gap-2">
            <span class="text-xs text-om-primary font-mono">{{ commit.shortHash }}</span>
            <span class="flex-1 truncate text-xs">{{ commit.message }}</span>
          </div>
          <div class="mt-0.5 flex items-center gap-2 text-xs text-om-dimmed">
            <span>{{ commit.author }}</span>
            <span>{{ commit.relativeDate }}</span>
            <n-tag v-if="commit.refs" size="small" type="warning">{{ commit.refs }}</n-tag>
          </div>
        </div>
      </template>

      <!-- Branches view -->
      <template v-else-if="activeView === 'branches'">
        <div v-if="branches.length === 0" class="p-4">
          <n-empty description="无分支" size="small" />
        </div>
        <div
          v-for="branch in branches"
          :key="branch.name"
          class="flex items-center gap-2 px-3 py-1.5 text-xs hover:bg-om-hover"
        >
          <i
            :class="branch.current ? 'i-ri:checkbox-circle-fill text-om-success' : 'i-ri:git-branch-line'"
            style="display: inline-block; width: 14px; height: 14px"
          />
          <span :class="{ 'font-bold text-om-primary': branch.current }">{{ branch.name }}</span>
          <span v-if="branch.upstream" class="text-om-dimmed">→ {{ branch.upstream }}</span>
        </div>
      </template>
    </div>
  </div>
</template>
