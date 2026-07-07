import { ref, triggerRef, watch } from 'vue'
import { useAuthStore } from '@/stores/auth'

export interface TransferItem {
  id: string
  sessionId: string
  fileName: string
  remotePath: string
  direction: 'upload' | 'download'
  status: 'queued' | 'transferring' | 'completed' | 'error' | 'cancelled'
  bytesTransferred: number
  totalSize: number
  error?: string
}

const STORAGE_KEY = 'onemux_transfers'

function loadTransfers(): TransferItem[] {
  try {
    const raw = sessionStorage.getItem(STORAGE_KEY)
    if (!raw) return []
    const items: TransferItem[] = JSON.parse(raw)
    return items.map((item) => {
      if (item.status === 'queued' || item.status === 'transferring') {
        return { ...item, status: 'cancelled' as const }
      }
      return item
    })
  } catch {
    return []
  }
}

function saveTransfers(items: TransferItem[]) {
  sessionStorage.setItem(STORAGE_KEY, JSON.stringify(items))
}

const transfers = ref<TransferItem[]>(loadTransfers())
const abortControllers = new Map<string, AbortController>()
const uploadXhrs = new Map<string, XMLHttpRequest>()
let idCounter = transfers.value.length

watch(transfers, (val) => saveTransfers(val), { deep: true })

export function useFileTransfer() {
  const auth = useAuthStore()

  async function enqueueDownload(sessionId: string, remotePath: string) {
    const fileName = remotePath.split('/').pop() || 'download'

    transfers.value.push({
      id: `dl-${++idCounter}`,
      sessionId,
      fileName,
      remotePath,
      direction: 'download',
      status: 'transferring',
      bytesTransferred: 0,
      totalSize: 0
    })
    const item = transfers.value[transfers.value.length - 1]

    const controller = new AbortController()
    abortControllers.set(item.id, controller)

    try {
      const url = `/api/sessions/${sessionId}/files/download?path=${encodeURIComponent(remotePath)}`
      const headers: Record<string, string> = {}
      if (auth.token) {
        headers.Authorization = `Bearer ${auth.token}`
      }

      const response = await fetch(url, { headers, signal: controller.signal })
      if (!response.ok) throw new Error(response.statusText || 'Download failed')

      const contentLength = response.headers.get('Content-Length')
      item.totalSize = contentLength ? Number.parseInt(contentLength, 10) : 0

      const reader = response.body!.getReader()
      const chunks: Uint8Array[] = []

      while (true) {
        const { done, value } = await reader.read()
        if (done) break
        chunks.push(value)
        item.bytesTransferred += value.byteLength
        triggerRef(transfers)
      }

      const blob = new Blob(chunks)
      const blobUrl = URL.createObjectURL(blob)
      const link = document.createElement('a')
      link.href = blobUrl
      link.download = fileName
      document.body.appendChild(link)
      link.click()
      document.body.removeChild(link)
      URL.revokeObjectURL(blobUrl)

      item.status = 'completed'
    } catch (e: any) {
      if (e.name === 'AbortError') {
        item.status = 'cancelled'
      } else {
        item.status = 'error'
        item.error = e.message
      }
    } finally {
      abortControllers.delete(item.id)
    }
  }

  async function enqueueUpload(sessionId: string, uploadDir: string, file: File): Promise<boolean> {
    transfers.value.push({
      id: `ul-${++idCounter}`,
      sessionId,
      fileName: file.name,
      remotePath: `${uploadDir}/${file.name}`,
      direction: 'upload',
      status: 'queued',
      bytesTransferred: 0,
      totalSize: file.size
    })
    const item = transfers.value[transfers.value.length - 1]

    item.status = 'transferring'

    try {
      const formData = new FormData()
      formData.append('file', file)

      const xhr = new XMLHttpRequest()
      uploadXhrs.set(item.id, xhr)
      const promise = new Promise<void>((resolve, reject) => {
        xhr.upload.onprogress = (e) => {
          if (e.lengthComputable) {
            item.bytesTransferred = e.loaded
            item.totalSize = e.total
            triggerRef(transfers)
          }
        }
        xhr.onload = () => {
          if (xhr.status >= 200 && xhr.status < 300) {
            resolve()
          } else {
            reject(new Error(xhr.statusText || 'Upload failed'))
          }
        }
        xhr.onerror = () => reject(new Error('Network error'))
        xhr.onabort = () => reject(new Error('Cancelled'))
      })

      xhr.open('POST', `/api/sessions/${sessionId}/files/upload?path=${encodeURIComponent(uploadDir)}`)
      if (auth.token) {
        xhr.setRequestHeader('Authorization', `Bearer ${auth.token}`)
      }
      xhr.send(formData)

      await promise
      item.status = 'completed'
      item.bytesTransferred = item.totalSize
      return true
    } catch (e: any) {
      if (e.message === 'Cancelled') {
        item.status = 'cancelled'
      } else {
        item.status = 'error'
        item.error = e.message
      }
      return false
    } finally {
      uploadXhrs.delete(item.id)
    }
  }

  function cancelTransfer(transferId: string) {
    const item = transfers.value.find((t) => t.id === transferId)
    if (item && (item.status === 'queued' || item.status === 'transferring')) {
      item.status = 'cancelled'
      abortControllers.get(transferId)?.abort()
      abortControllers.delete(transferId)
      uploadXhrs.get(transferId)?.abort()
      uploadXhrs.delete(transferId)
    }
  }

  function clearCompleted() {
    transfers.value = transfers.value.filter(
      (t) => t.status !== 'completed' && t.status !== 'error' && t.status !== 'cancelled'
    )
  }

  function hasActive() {
    return transfers.value.some((t) => t.status === 'queued' || t.status === 'transferring')
  }

  return {
    transfers,
    enqueueDownload,
    enqueueUpload,
    cancelTransfer,
    clearCompleted,
    hasActive
  }
}
