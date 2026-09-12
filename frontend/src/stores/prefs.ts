import { defineStore } from 'pinia'
import { ref } from 'vue'
import { useUiState } from '@/composables/useUiState'

/**
 * Reactive view of the user's connection preferences (favourites, directory
 * bookmarks). Backed by the same persisted UI state as the layout, so they
 * follow the user across browsers.
 */
export const usePrefsStore = defineStore('prefs', () => {
  const uiState = useUiState()
  const favorites = ref<string[]>([])
  const bookmarks = ref<Record<string, string[]>>({})
  const loaded = ref(false)
  let inflight: Promise<void> | null = null

  async function load(): Promise<void> {
    if (loaded.value) return
    if (inflight) return inflight
    inflight = (async () => {
      const s = await uiState.loadState()
      favorites.value = s.prefs?.favorites ?? []
      bookmarks.value = s.prefs?.bookmarks ?? {}
      loaded.value = true
      inflight = null
    })()
    return inflight
  }

  function persist() {
    uiState.updatePrefs({ favorites: favorites.value, bookmarks: bookmarks.value })
  }

  function isFavorite(serverId: string): boolean {
    return favorites.value.includes(serverId)
  }

  function toggleFavorite(serverId: string) {
    favorites.value = isFavorite(serverId)
      ? favorites.value.filter((id) => id !== serverId)
      : [...favorites.value, serverId]
    persist()
  }

  function bookmarksFor(serverId: string): string[] {
    return bookmarks.value[serverId] ?? []
  }

  function hasBookmark(serverId: string, path: string): boolean {
    return bookmarksFor(serverId).includes(path)
  }

  function toggleBookmark(serverId: string, path: string) {
    const list = bookmarksFor(serverId)
    const next = list.includes(path) ? list.filter((p) => p !== path) : [...list, path].sort()
    bookmarks.value = { ...bookmarks.value, [serverId]: next }
    persist()
  }

  return { favorites, bookmarks, loaded, load, isFavorite, toggleFavorite, bookmarksFor, hasBookmark, toggleBookmark }
})
