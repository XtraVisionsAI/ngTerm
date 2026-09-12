import { defineStore } from 'pinia'
import { computed, ref } from 'vue'

/**
 * Feature names the running distribution advertises through `GET /api/features`.
 * Screens that only make sense with a matching backend gate on `has(name)`.
 */
export const useFeaturesStore = defineStore('features', () => {
  const features = ref<string[]>([])
  const loaded = ref(false)
  let inflight: Promise<void> | null = null

  async function load(force = false): Promise<void> {
    if (loaded.value && !force) return
    if (inflight) return inflight
    inflight = (async () => {
      try {
        const res = await fetch('/api/features')
        if (res.ok) {
          const data = (await res.json()) as { features?: string[] }
          features.value = Array.isArray(data.features) ? data.features : []
        }
      } catch {
        // offline or old backend: no optional features
      } finally {
        loaded.value = true
        inflight = null
      }
    })()
    return inflight
  }

  function has(name: string): boolean {
    return features.value.includes(name)
  }

  const approvals = computed(() => has('approvals'))
  const tasks = computed(() => has('tasks'))

  return { features, loaded, load, has, approvals, tasks }
})
