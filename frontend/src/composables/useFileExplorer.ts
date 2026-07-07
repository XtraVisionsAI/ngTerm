import { computed, ref, watch } from 'vue'
import { useApi } from './useApi'

export interface FileEntry {
  name: string
  fileType: 'file' | 'directory' | 'symlink' | 'unknown'
  size: number
  permissions: string
  owner: string
  group: string
  modified: string
}

export type SortKey = 'name' | 'modified' | 'size' | 'permissions'
export type SortDir = 'asc' | 'desc'

export function useFileExplorer(sessionId: () => string, initialPath?: () => string | undefined) {
  const api = useApi()

  const files = ref<FileEntry[]>([])
  const currentPath = ref('~')
  const homePath = ref('')
  const loading = ref(true)
  const error = ref<string | null>(null)
  const sortBy = ref<SortKey>('name')
  const sortDir = ref<SortDir>('asc')

  const sortedFiles = computed(() => {
    const dirs = files.value.filter((f) => f.fileType === 'directory')
    const rest = files.value.filter((f) => f.fileType !== 'directory')

    const compareFn = (a: FileEntry, b: FileEntry) => {
      let cmp = 0
      switch (sortBy.value) {
        case 'name':
          cmp = a.name.localeCompare(b.name)
          break
        case 'size':
          cmp = a.size - b.size
          break
        case 'modified':
          cmp = a.modified.localeCompare(b.modified)
          break
        case 'permissions':
          cmp = a.permissions.localeCompare(b.permissions)
          break
      }
      return sortDir.value === 'asc' ? cmp : -cmp
    }

    return [...dirs.sort(compareFn), ...rest.sort(compareFn)]
  })

  const displayPath = computed(() => {
    const p = currentPath.value
    if (!homePath.value || !p) return p
    if (p === homePath.value) return '~'
    if (p.startsWith(`${homePath.value}/`)) return `~${p.slice(homePath.value.length)}`
    return p
  })

  const breadcrumbs = computed(() => {
    const dp = displayPath.value
    if (!dp || dp === '/') {
      return [{ name: '/', path: '/' }]
    }
    if (dp === '~') {
      return [{ name: '~', path: '~' }]
    }
    if (dp.startsWith('~')) {
      const parts = dp.slice(2).split('/').filter(Boolean)
      const crumbs = [{ name: '~', path: '~' }]
      for (let i = 0; i < parts.length; i++) {
        crumbs.push({
          name: parts[i],
          path: `~/${parts.slice(0, i + 1).join('/')}`
        })
      }
      return crumbs
    }
    const parts = dp.split('/').filter(Boolean)
    const crumbs = [{ name: '/', path: '/' }]
    for (let i = 0; i < parts.length; i++) {
      crumbs.push({
        name: parts[i],
        path: `/${parts.slice(0, i + 1).join('/')}`
      })
    }
    return crumbs
  })

  function resolveApiPath(path: string): string {
    if (!path || path === '~') return homePath.value || '~'
    if (path.startsWith('~/')) return (homePath.value || '~') + path.slice(1)
    return path
  }

  async function navigate(path: string) {
    const sid = sessionId()
    if (!sid || sid.startsWith('pending-')) return

    const apiPath = resolveApiPath(path)
    currentPath.value = apiPath
    loading.value = true
    error.value = null
    try {
      const result = await api.get<FileEntry[]>(`/sessions/${sid}/files?path=${encodeURIComponent(apiPath)}`)
      files.value = result
    } catch (e: any) {
      error.value = e.message || 'Failed to list directory'
      files.value = []
    } finally {
      loading.value = false
    }
  }

  async function navigateHome() {
    const sid = sessionId()
    if (!sid || sid.startsWith('pending-')) return

    try {
      const result = await api.get<{ path: string }>(`/sessions/${sid}/files/home`)
      homePath.value = result.path
      const savedPath = initialPath?.()
      if (savedPath) {
        await navigate(savedPath)
      } else {
        await navigate(result.path)
      }
    } catch {
      await navigate('/')
    }
  }

  function goUp() {
    const dp = displayPath.value
    if (!dp || dp === '/') return
    const parts = currentPath.value.split('/').filter(Boolean)
    parts.pop()
    const parent = parts.length > 0 ? `/${parts.join('/')}` : '/'
    navigate(parent)
  }

  function refresh() {
    navigate(currentPath.value || homePath.value || '/')
  }

  function toggleSort(key: SortKey) {
    if (sortBy.value === key) {
      sortDir.value = sortDir.value === 'asc' ? 'desc' : 'asc'
    } else {
      sortBy.value = key
      sortDir.value = 'asc'
    }
  }

  watch(
    () => sessionId(),
    (sid) => {
      if (sid && !sid.startsWith('pending-')) {
        navigateHome()
      }
    },
    { immediate: true }
  )

  return {
    files: sortedFiles,
    rawFiles: files,
    currentPath,
    displayPath,
    homePath,
    loading,
    error,
    sortBy,
    sortDir,
    breadcrumbs,
    navigate,
    navigateHome,
    goUp,
    refresh,
    toggleSort
  }
}
