<script setup lang="ts">
  import type { FileEntry } from '@/composables/useFileExplorer'
  import { NButton, NDropdown, NEmpty, NInput, NModal, NSpin, NVirtualList, useMessage } from 'naive-ui'
  import { computed, inject, nextTick, ref, watch } from 'vue'
  import { useApi } from '@/composables/useApi'
  import { useFileExplorer } from '@/composables/useFileExplorer'
  import { useFileTransfer } from '@/composables/useFileTransfer'

  const props = defineProps<{
    sessionId: string
    initialPath?: string
  }>()

  const emit = defineEmits<{
    'path-change': [path: string]
  }>()

  const api = useApi()
  const message = useMessage()
  const explorer = useFileExplorer(
    () => props.sessionId,
    () => props.initialPath
  )
  const transfer = useFileTransfer()

  watch(
    () => explorer.currentPath.value,
    (path) => {
      if (path) emit('path-change', path)
    }
  )

  const fileInputRef = ref<HTMLInputElement | null>(null)
  const showNewFolder = ref(false)
  const newFolderName = ref('')
  const showNewFile = ref(false)
  const newFileName = ref('')
  const showRename = ref(false)
  const renameTarget = ref<FileEntry | null>(null)
  const renameName = ref('')
  const contextMenu = ref({ show: false, x: 0, y: 0, entry: null as FileEntry | null })
  const pathEditing = ref(false)
  const pathInput = ref('')
  const pathInputRef = ref<InstanceType<typeof NInput> | null>(null)
  const dragOver = ref(false)
  const showHidden = ref(true)
  const transferExpanded = ref(true)

  /** Provided by the terminal page: queue a file's content as AI context. */
  const addAiContext = inject<
    ((paneId: string, text: string, opts?: { analyze?: boolean; kind?: 'file'; title?: string }) => void) | undefined
  >('addAiContext', undefined)
  const AI_CONTEXT_MAX_BYTES = 512 * 1024

  async function addFileToAiContext(entry: FileEntry) {
    if (!addAiContext) return
    if (entry.size > AI_CONTEXT_MAX_BYTES) {
      message.warning(`文件超过 ${AI_CONTEXT_MAX_BYTES / 1024} KiB，请下载后裁剪再添加`)
      return
    }
    const base = explorer.currentPath.value
    const path = base.endsWith('/') ? `${base}${entry.name}` : `${base}/${entry.name}`
    try {
      const r = await api.get<{ content: string }>(
        `/sessions/${props.sessionId}/files/content?path=${encodeURIComponent(path)}`
      )
      addAiContext(props.sessionId, r.content, { kind: 'file', title: path })
    } catch (e) {
      message.error(`读取文件失败: ${(e as Error).message}`)
    }
  }

  const contextMenuOptions = computed(() => {
    const entry = contextMenu.value.entry
    if (!entry) {
      return [
        { label: '新建文件', key: 'newFile' },
        { label: '新建文件夹', key: 'newFolder' },
        { type: 'divider', key: 'd1' },
        { label: '刷新', key: 'refresh' }
      ]
    }
    const items: any[] = []
    if (entry.fileType === 'directory') {
      items.push({ label: '打开', key: 'open' })
    } else {
      items.push({ label: '下载', key: 'download' })
      if (addAiContext) items.push({ label: '添加到 AI 上下文', key: 'aiContext' })
    }
    items.push({ label: '重命名', key: 'rename' })
    items.push({ label: '删除', key: 'delete' })
    return items
  })

  function handleEntryClick(entry: FileEntry) {
    if (entry.fileType === 'directory') {
      const base = explorer.currentPath.value
      const path = base.endsWith('/') ? `${base}${entry.name}` : `${base}/${entry.name}`
      explorer.navigate(path)
    }
  }

  function handleEntryDblClick(entry: FileEntry) {
    if (entry.fileType !== 'directory') {
      const base = explorer.currentPath.value
      const path = base.endsWith('/') ? `${base}${entry.name}` : `${base}/${entry.name}`
      transfer.enqueueDownload(props.sessionId, path)
    }
  }

  function handleContextMenu(e: MouseEvent, entry: FileEntry) {
    e.preventDefault()
    contextMenu.value = { show: true, x: e.clientX, y: e.clientY, entry }
  }

  function handleContextSelect(key: string) {
    contextMenu.value.show = false
    const entry = contextMenu.value.entry

    if (!entry) {
      switch (key) {
        case 'newFile':
          showNewFile.value = true
          newFileName.value = ''
          break
        case 'newFolder':
          showNewFolder.value = true
          newFolderName.value = ''
          break
        case 'refresh':
          explorer.refresh()
          break
      }
      return
    }

    const base = explorer.currentPath.value
    const path = base.endsWith('/') ? `${base}${entry.name}` : `${base}/${entry.name}`

    switch (key) {
      case 'open':
        explorer.navigate(path)
        break
      case 'download':
        transfer.enqueueDownload(props.sessionId, path)
        break
      case 'aiContext':
        addFileToAiContext(entry)
        break
      case 'rename':
        renameTarget.value = entry
        renameName.value = entry.name
        showRename.value = true
        break
      case 'delete':
        doDelete(path)
        break
    }
  }

  async function doDelete(path: string) {
    try {
      await api.del(`/sessions/${props.sessionId}/files?path=${encodeURIComponent(path)}`)
      explorer.refresh()
    } catch (e: any) {
      message.error(`删除失败: ${e.message || '未知错误'}`)
    }
  }

  async function doRename() {
    if (!renameTarget.value || !renameName.value.trim()) return
    const base = explorer.currentPath.value
    const fromPath = base.endsWith('/') ? `${base}${renameTarget.value.name}` : `${base}/${renameTarget.value.name}`
    const toPath = base.endsWith('/') ? `${base}${renameName.value.trim()}` : `${base}/${renameName.value.trim()}`

    try {
      await api.post(`/sessions/${props.sessionId}/files/rename`, { from: fromPath, to: toPath })
      showRename.value = false
      explorer.refresh()
    } catch (e: any) {
      message.error(`重命名失败: ${e.message || '未知错误'}`)
    }
  }

  async function doNewFolder() {
    if (!newFolderName.value.trim()) return
    const base = explorer.currentPath.value
    const path = base.endsWith('/') ? `${base}${newFolderName.value.trim()}` : `${base}/${newFolderName.value.trim()}`
    try {
      await api.post(`/sessions/${props.sessionId}/files/mkdir`, { path })
      showNewFolder.value = false
      newFolderName.value = ''
      explorer.refresh()
    } catch (e: any) {
      message.error(`创建文件夹失败: ${e.message || '未知错误'}`)
    }
  }

  async function doNewFile() {
    if (!newFileName.value.trim()) return
    const base = explorer.currentPath.value
    const path = base.endsWith('/') ? `${base}${newFileName.value.trim()}` : `${base}/${newFileName.value.trim()}`
    try {
      await api.put(`/sessions/${props.sessionId}/files/content`, { path, content: '' })
      showNewFile.value = false
      newFileName.value = ''
      explorer.refresh()
    } catch (e: any) {
      message.error(`创建文件失败: ${e.message || '未知错误'}`)
    }
  }

  function copyPath() {
    navigator.clipboard.writeText(explorer.currentPath.value)
  }

  function triggerUpload() {
    fileInputRef.value?.click()
  }

  async function handleFileSelect(e: Event) {
    const input = e.target as HTMLInputElement
    const fileList = input.files
    if (!fileList) return
    let failed = 0
    for (let i = 0; i < fileList.length; i++) {
      const result = await transfer.enqueueUpload(props.sessionId, explorer.currentPath.value, fileList[i])
      if (!result) failed++
    }
    input.value = ''
    if (failed > 0) message.error(`${failed} 个文件上传失败`)
    explorer.refresh()
  }

  function startPathEdit() {
    pathInput.value = explorer.displayPath.value
    pathEditing.value = true
    nextTick(() => pathInputRef.value?.focus())
  }

  function confirmPathEdit() {
    pathEditing.value = false
    const newPath = pathInput.value.trim()
    if (newPath && newPath !== explorer.displayPath.value) {
      explorer.navigate(newPath)
    }
  }

  async function handleDrop(e: DragEvent) {
    e.preventDefault()
    dragOver.value = false
    const fileList = e.dataTransfer?.files
    if (!fileList) return
    let failed = 0
    for (let i = 0; i < fileList.length; i++) {
      const result = await transfer.enqueueUpload(props.sessionId, explorer.currentPath.value, fileList[i])
      if (!result) failed++
    }
    if (failed > 0) message.error(`${failed} 个文件上传失败`)
    explorer.refresh()
  }

  function handleDragOver(e: DragEvent) {
    e.preventDefault()
    dragOver.value = true
  }

  function handleDragLeave() {
    dragOver.value = false
  }

  function formatSize(bytes: number): string {
    if (bytes < 1024) return `${bytes} B`
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} K`
    if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} M`
    return `${(bytes / (1024 * 1024 * 1024)).toFixed(1)} G`
  }

  function getFileIcon(entry: FileEntry): string {
    if (entry.fileType === 'directory') return 'i-ri:folder-fill'
    if (entry.fileType === 'symlink') return 'i-ri:links-line'
    const name = entry.name.toLowerCase()
    const ext = name.split('.').pop() || ''
    if (['ts', 'tsx'].includes(ext)) return 'i-ri:braces-line'
    if (['js', 'jsx', 'mjs', 'cjs'].includes(ext)) return 'i-ri:javascript-line'
    if (ext === 'vue') return 'i-ri:vuejs-line'
    if (ext === 'rs') return 'i-ri:terminal-box-line'
    if (['py', 'pyi'].includes(ext)) return 'i-ri:code-s-slash-line'
    if (ext === 'go') return 'i-ri:code-s-slash-line'
    if (['java', 'kt', 'scala'].includes(ext)) return 'i-ri:cup-line'
    if (['rb', 'php', 'lua', 'pl', 'sh', 'bash', 'zsh', 'fish'].includes(ext)) return 'i-ri:terminal-line'
    if (['c', 'cpp', 'h', 'hpp', 'cc'].includes(ext)) return 'i-ri:code-box-line'
    if (['html', 'htm'].includes(ext)) return 'i-ri:html5-line'
    if (['css', 'scss', 'sass', 'less'].includes(ext)) return 'i-ri:css3-line'
    if (['md', 'mdx'].includes(ext)) return 'i-ri:markdown-line'
    if (['txt', 'log', 'csv', 'tsv'].includes(ext)) return 'i-ri:file-text-line'
    if (['json', 'yaml', 'yml', 'toml', 'xml', 'ini', 'env'].includes(ext)) return 'i-ri:settings-3-line'
    if (['png', 'jpg', 'jpeg', 'gif', 'svg', 'webp', 'ico', 'bmp', 'tiff'].includes(ext)) return 'i-ri:image-line'
    if (['mp4', 'avi', 'mov', 'mkv', 'webm', 'flv'].includes(ext)) return 'i-ri:film-line'
    if (['mp3', 'wav', 'ogg', 'flac', 'aac', 'm4a'].includes(ext)) return 'i-ri:music-line'
    if (['zip', 'tar', 'gz', 'bz2', 'xz', '7z', 'rar', 'tgz'].includes(ext)) return 'i-ri:file-zip-line'
    if (['pdf'].includes(ext)) return 'i-ri:file-pdf-line'
    if (['doc', 'docx', 'odt', 'rtf'].includes(ext)) return 'i-ri:file-word-line'
    if (['xls', 'xlsx', 'ods'].includes(ext)) return 'i-ri:file-excel-line'
    if (['ppt', 'pptx', 'odp'].includes(ext)) return 'i-ri:file-ppt-line'
    if (['db', 'sqlite', 'sql'].includes(ext)) return 'i-ri:database-2-line'
    if (['ttf', 'otf', 'woff', 'woff2', 'eot'].includes(ext)) return 'i-ri:font-size-2'
    if (['lock'].includes(ext) || name === 'package-lock.json' || name === 'pnpm-lock.yaml') return 'i-ri:lock-line'
    if (['key', 'pem', 'crt', 'cer', 'p12'].includes(ext)) return 'i-ri:shield-keyhole-line'
    if (name === 'dockerfile' || name.startsWith('dockerfile.')) return 'i-ri:ship-line'
    if (name === 'makefile' || name === 'justfile') return 'i-ri:hammer-line'
    if (name === '.gitignore' || name === '.gitattributes') return 'i-ri:git-branch-line'
    return 'i-ri:file-line'
  }

  const filteredFiles = computed(() => {
    if (showHidden.value) return explorer.files.value
    return explorer.files.value.filter((f) => !f.name.startsWith('.'))
  })

  const useVirtualList = computed(() => filteredFiles.value.length > 200)

  const virtualListItems = computed(() => {
    return filteredFiles.value.map((entry) => ({ key: entry.name, entry }))
  })

  const stats = computed(() => {
    const files = filteredFiles.value
    const count = files.length
    const totalSize = files.reduce((sum, f) => sum + (f.fileType !== 'directory' ? f.size : 0), 0)
    return { count, totalSize }
  })

  const hasCompletedTransfers = computed(() =>
    transfer.transfers.value.some((t) => t.status === 'completed' || t.status === 'error' || t.status === 'cancelled')
  )

  function getTransferStatus(status: string): string {
    switch (status) {
      case 'queued':
        return '等待'
      case 'transferring':
        return ''
      case 'completed':
        return '✓'
      case 'error':
        return '✗'
      case 'cancelled':
        return '取消'
      default:
        return status
    }
  }
</script>

<template>
  <div
    class="h-full flex flex-col border-l border-l-om-border border-l-solid bg-om-bg text-om-text"
    @drop="handleDrop"
    @dragover="handleDragOver"
    @dragleave="handleDragLeave"
  >
    <!-- Toolbar -->
    <div class="flex items-center gap-1 border-b border-om-border border-b-solid px-2 py-1">
      <n-button quaternary size="tiny" title="上级目录" @click="explorer.goUp">
        <template #icon><i class="i-ri:arrow-up-line block size-3.5" /></template>
      </n-button>
      <n-button quaternary size="tiny" title="主目录" @click="explorer.navigateHome">
        <template #icon><i class="i-ri:home-4-line block size-3.5" /></template>
      </n-button>
      <n-button quaternary size="tiny" title="刷新" @click="explorer.refresh">
        <template #icon><i class="i-ri:refresh-line block size-3.5" /></template>
      </n-button>
      <n-button quaternary size="tiny" title="复制路径" @click="copyPath">
        <template #icon><i class="i-ri:clipboard-line block size-3.5" /></template>
      </n-button>
      <div class="mx-1 h-4 w-px bg-om-border" />
      <n-button quaternary size="tiny" title="上传" @click="triggerUpload">
        <template #icon><i class="i-ri:upload-2-line block size-3.5" /></template>
      </n-button>
      <n-button quaternary size="tiny" title="新建文件" @click="((showNewFile = true), (newFileName = ''))">
        <template #icon><i class="i-ri:file-add-line block size-3.5" /></template>
      </n-button>
      <n-button quaternary size="tiny" title="新建文件夹" @click="((showNewFolder = true), (newFolderName = ''))">
        <template #icon><i class="i-ri:folder-add-line block size-3.5" /></template>
      </n-button>
      <div class="mx-1 h-4 w-px bg-om-border" />
      <n-button
        quaternary
        size="tiny"
        :title="showHidden ? '隐藏点文件' : '显示点文件'"
        :type="showHidden ? 'primary' : 'default'"
        @click="showHidden = !showHidden"
      >
        <template #icon
          ><i :class="showHidden ? 'i-ri:eye-line' : 'i-ri:eye-off-line'" class="block size-3.5"
        /></template>
      </n-button>
      <input ref="fileInputRef" type="file" multiple class="hidden" @change="handleFileSelect" />
    </div>

    <!-- Path bar -->
    <div
      class="min-h-7 flex items-center border-b border-om-border border-b-solid px-2 py-1 text-xs"
      @dblclick="startPathEdit"
    >
      <template v-if="pathEditing">
        <n-input
          ref="pathInputRef"
          v-model:value="pathInput"
          size="tiny"
          class="flex-1"
          @keydown.enter="confirmPathEdit"
          @keydown.escape="pathEditing = false"
          @blur="confirmPathEdit"
        />
      </template>
      <template v-else>
        <div class="flex cursor-pointer items-center gap-1 overflow-hidden">
          <template v-for="(crumb, i) in explorer.breadcrumbs.value" :key="i">
            <span v-if="i > 0" class="text-om-dimmed">›</span>
            <n-button size="tiny" quaternary @click="explorer.navigate(crumb.path)">
              {{ crumb.name }}
            </n-button>
          </template>
        </div>
      </template>
    </div>

    <!-- Column headers -->
    <div
      class="flex select-none items-center border-b border-om-border border-b-solid px-2 py-1 text-[10px] text-om-dimmed tracking-wide uppercase"
    >
      <div class="flex-1 cursor-pointer" @click="explorer.toggleSort('name')">
        名称
        <span v-if="explorer.sortBy.value === 'name'">{{ explorer.sortDir.value === 'asc' ? '↑' : '↓' }}</span>
      </div>
      <div class="w-16 cursor-pointer text-right" @click="explorer.toggleSort('size')">
        大小
        <span v-if="explorer.sortBy.value === 'size'">{{ explorer.sortDir.value === 'asc' ? '↑' : '↓' }}</span>
      </div>
    </div>

    <!-- File list -->
    <div
      class="relative min-h-0 flex-1"
      :class="[dragOver ? 'ring-2 ring-om-primary ring-inset' : '', useVirtualList ? '' : 'overflow-y-auto']"
      @contextmenu.self.prevent="contextMenu = { show: true, x: $event.clientX, y: $event.clientY, entry: null }"
    >
      <div v-if="explorer.loading.value" class="flex flex-col items-center justify-center gap-2 p-8">
        <n-spin size="small" />
        <span class="text-xs text-om-dimmed">加载中…</span>
      </div>
      <template v-else-if="filteredFiles.length > 0">
        <!-- Virtual list for large directories -->
        <n-virtual-list v-if="useVirtualList" :items="virtualListItems" :item-size="28" key-field="key" class="h-full">
          <template #default="{ item }">
            <div
              class="h-7 flex cursor-pointer items-center px-2 text-xs hover:bg-om-hover"
              @click="handleEntryClick(item.entry)"
              @dblclick="handleEntryDblClick(item.entry)"
              @contextmenu="handleContextMenu($event, item.entry)"
            >
              <div class="min-w-0 flex flex-1 items-center gap-1.5">
                <i
                  :class="[
                    getFileIcon(item.entry),
                    item.entry.fileType === 'directory' ? 'text-om-primary' : 'text-om-dimmed'
                  ]"
                  class="block size-3.5 shrink-0"
                />
                <span class="truncate" :title="item.entry.name">{{ item.entry.name }}</span>
              </div>
              <div class="w-16 text-right text-om-dimmed">{{
                item.entry.fileType !== 'directory' ? formatSize(item.entry.size) : ''
              }}</div>
            </div>
          </template>
        </n-virtual-list>
        <!-- Simple list for small directories -->
        <template v-else>
          <!-- Parent dir -->
          <div
            v-if="explorer.displayPath.value !== '/'"
            class="flex cursor-pointer items-center px-2 py-1 text-xs hover:bg-om-hover"
            @click="explorer.goUp"
          >
            <div class="flex flex-1 items-center gap-1.5">
              <i class="i-ri:corner-left-up-line block size-3.5 text-om-dimmed" />
              <span>..</span>
            </div>
          </div>
          <!-- Files -->
          <div
            v-for="entry in filteredFiles"
            :key="entry.name"
            class="flex cursor-pointer items-center px-2 py-1 text-xs hover:bg-om-hover"
            @click="handleEntryClick(entry)"
            @dblclick="handleEntryDblClick(entry)"
            @contextmenu="handleContextMenu($event, entry)"
          >
            <div class="min-w-0 flex flex-1 items-center gap-1.5">
              <i
                :class="[getFileIcon(entry), entry.fileType === 'directory' ? 'text-om-primary' : 'text-om-dimmed']"
                class="block size-3.5 shrink-0"
              />
              <span class="truncate" :title="entry.name">{{ entry.name }}</span>
            </div>
            <div class="w-16 text-right text-om-dimmed">{{
              entry.fileType !== 'directory' ? formatSize(entry.size) : ''
            }}</div>
          </div>
        </template>
      </template>
      <div v-else-if="explorer.error.value" class="flex flex-col items-center justify-center gap-2 p-8 text-center">
        <i class="i-ri:error-warning-line block size-6 text-red-400" />
        <span class="text-xs text-red-400">{{ explorer.error.value }}</span>
        <n-button size="tiny" quaternary @click="explorer.refresh">重试</n-button>
      </div>
      <div v-else-if="!explorer.loading.value" class="flex items-center justify-center p-8">
        <n-empty description="空目录" size="small" />
      </div>

      <!-- Drag overlay -->
      <div v-if="dragOver" class="pointer-events-none absolute inset-0 flex items-center justify-center bg-om-bg/80">
        <div class="flex flex-col items-center gap-2 text-om-primary">
          <i class="i-ri:upload-cloud-2-line block size-8" />
          <span class="text-sm">拖放文件上传</span>
        </div>
      </div>
    </div>

    <!-- Transfer panel -->
    <div v-if="transfer.transfers.value.length > 0" class="shrink-0 border-t border-om-border border-t-solid">
      <div
        class="flex cursor-pointer items-center justify-between px-2 py-0.5"
        @click="transferExpanded = !transferExpanded"
      >
        <div class="flex items-center gap-1">
          <i
            :class="transferExpanded ? 'i-ri:arrow-down-s-line' : 'i-ri:arrow-right-s-line'"
            class="block size-3 text-om-dimmed"
          />
          <span class="text-[10px] text-om-dimmed font-medium">传输队列 ({{ transfer.transfers.value.length }})</span>
        </div>
        <n-button
          v-if="hasCompletedTransfers"
          quaternary
          size="tiny"
          title="清除已完成"
          @click.stop="transfer.clearCompleted()"
        >
          <template #icon><i class="i-ri:delete-bin-line block size-3" /></template>
        </n-button>
      </div>
      <div v-if="transferExpanded" class="max-h-60 overflow-y-auto">
        <div v-for="item in transfer.transfers.value" :key="item.id" class="min-h-[60px] px-2 py-1.5">
          <div class="flex items-center gap-1.5">
            <i
              class="block size-3 shrink-0"
              :class="[
                item.direction === 'upload'
                  ? 'i-ri:upload-2-line text-om-primary'
                  : 'i-ri:download-2-line text-green-400'
              ]"
            />
            <span class="flex-1 truncate text-[11px]">{{ item.fileName }}</span>
            <span class="text-[10px] text-om-dimmed">{{ getTransferStatus(item.status) }}</span>
            <n-button
              v-if="item.status === 'transferring'"
              quaternary
              size="tiny"
              title="取消"
              @click="transfer.cancelTransfer(item.id)"
            >
              <template #icon><i class="i-ri:close-circle-line block size-3" /></template>
            </n-button>
          </div>
          <div v-if="item.status === 'transferring'" class="mt-1">
            <div class="h-1.5 w-full overflow-hidden rounded bg-om-border">
              <div
                class="h-full bg-om-primary transition-all"
                :style="{
                  width: item.totalSize > 0 ? `${Math.round((item.bytesTransferred / item.totalSize) * 100)}%` : '100%',
                  opacity: item.totalSize > 0 ? 1 : 0.3
                }"
              />
            </div>
            <div class="mt-0.5 text-[11px] text-om-dimmed">
              {{
                item.totalSize > 0 ? `${formatSize(item.bytesTransferred)}/${formatSize(item.totalSize)}` : '传输中…'
              }}
            </div>
          </div>
        </div>
      </div>
    </div>

    <!-- Status bar -->
    <div
      class="flex items-center justify-between border-t border-om-border border-t-solid px-2 py-0.5 text-[10px] text-om-dimmed"
    >
      <span>{{ stats.count }} 项</span>
      <span>{{ formatSize(stats.totalSize) }}</span>
    </div>

    <!-- Context menu -->
    <n-dropdown
      :show="contextMenu.show"
      :x="contextMenu.x"
      :y="contextMenu.y"
      :options="contextMenuOptions"
      placement="bottom-start"
      trigger="manual"
      @select="handleContextSelect"
      @clickoutside="contextMenu.show = false"
    />

    <!-- New folder modal -->
    <n-modal
      v-model:show="showNewFolder"
      preset="dialog"
      title="新建文件夹"
      positive-text="创建"
      @positive-click="doNewFolder"
    >
      <n-input v-model:value="newFolderName" placeholder="文件夹名称" @keydown.enter="doNewFolder" />
    </n-modal>

    <!-- New file modal -->
    <n-modal
      v-model:show="showNewFile"
      preset="dialog"
      title="新建文件"
      positive-text="创建"
      @positive-click="doNewFile"
    >
      <n-input v-model:value="newFileName" placeholder="文件名称" @keydown.enter="doNewFile" />
    </n-modal>

    <!-- Rename modal -->
    <n-modal v-model:show="showRename" preset="dialog" title="重命名" positive-text="确定" @positive-click="doRename">
      <n-input v-model:value="renameName" placeholder="新名称" @keydown.enter="doRename" />
    </n-modal>
  </div>
</template>
