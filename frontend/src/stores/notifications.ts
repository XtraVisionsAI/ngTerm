import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { useAuthStore } from '@/stores/auth'
import { useFeaturesStore } from '@/stores/features'

/**
 * In-app notifications (UX-08): scheduled runs and other enterprise events
 * drop a row per user. The activity bar shows the unread count; the inbox
 * lists them and marks them read. Polled on a light interval, like approvals.
 */
export interface Notification {
  notifId: string
  userId: string
  kind: string
  level: 'info' | 'warning' | 'error'
  title: string
  body: string
  link: string | null
  read: boolean
  createdAt: string
}

export const useNotificationsStore = defineStore('notifications', () => {
  const auth = useAuthStore()
  const features = useFeaturesStore()

  const unread = ref(0)
  const items = ref<Notification[]>([])
  const total = ref(0)
  const loading = ref(false)
  let timer: ReturnType<typeof setInterval> | null = null

  async function req<T>(method: string, path: string): Promise<T | null> {
    if (!auth.token) return null
    const res = await fetch(`/api${path}`, {
      method,
      headers: { Authorization: `Bearer ${auth.token}` }
    })
    if (!res.ok) return null
    return (await res.json()) as T
  }

  async function refreshUnread(): Promise<void> {
    if (!features.notifications || !auth.isAuthenticated) return
    const data = await req<{ count: number }>('GET', '/notifications/unread-count')
    if (data) unread.value = data.count
  }

  async function load(unreadOnly = false): Promise<void> {
    if (!features.notifications || !auth.isAuthenticated) return
    loading.value = true
    try {
      const data = await req<{ items: Notification[]; total: number }>(
        'GET',
        `/notifications?unread=${unreadOnly}&limit=100`
      )
      if (data) {
        items.value = data.items
        total.value = data.total
      }
    } finally {
      loading.value = false
    }
  }

  async function markRead(id: string): Promise<void> {
    await req('POST', `/notifications/${id}/read`)
    const n = items.value.find((x) => x.notifId === id)
    if (n && !n.read) {
      n.read = true
      unread.value = Math.max(0, unread.value - 1)
    }
  }

  async function markAllRead(): Promise<void> {
    await req('POST', '/notifications/read-all')
    items.value.forEach((n) => (n.read = true))
    unread.value = 0
  }

  function startPolling(): void {
    if (timer) return
    refreshUnread()
    timer = setInterval(refreshUnread, 30000)
  }

  function stopPolling(): void {
    if (timer) clearInterval(timer)
    timer = null
  }

  const hasUnread = computed(() => unread.value > 0)

  return {
    unread,
    items,
    total,
    loading,
    hasUnread,
    refreshUnread,
    load,
    markRead,
    markAllRead,
    startPolling,
    stopPolling
  }
})
