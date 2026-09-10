import { useAuthStore } from '@/stores/auth'

/**
 * Fetch an authenticated API resource and hand it to the browser as a file.
 * Used for audit exports, which need the bearer token and so cannot be plain
 * links.
 */
export async function downloadWithAuth(path: string, fallbackName: string): Promise<void> {
  const auth = useAuthStore()
  const res = await fetch(`/api${path}`, {
    headers: auth.token ? { Authorization: `Bearer ${auth.token}` } : {}
  })
  if (!res.ok) {
    const err = await res.json().catch(() => ({ error: res.statusText }))
    throw new Error(err.error || res.statusText)
  }
  const disposition = res.headers.get('content-disposition') || ''
  const match = /filename="([^"]+)"/.exec(disposition)
  const filename = match ? match[1] : fallbackName
  const blob = await res.blob()
  const url = URL.createObjectURL(blob)
  const a = document.createElement('a')
  a.href = url
  a.download = filename
  document.body.appendChild(a)
  a.click()
  a.remove()
  URL.revokeObjectURL(url)
  if (res.headers.get('x-audit-truncated') === 'true') {
    throw new Error('导出超过上限，仅包含前 10000 行')
  }
}
