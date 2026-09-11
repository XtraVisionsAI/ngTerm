import { useRouter } from 'vue-router'
import { useAuthStore } from '@/stores/auth'

export function useApi() {
  const auth = useAuthStore()
  const router = useRouter()

  async function request<T>(method: string, path: string, body?: unknown): Promise<T> {
    const headers: Record<string, string> = {}

    if (auth.token) {
      headers.Authorization = `Bearer ${auth.token}`
    }
    if (body !== undefined) {
      headers['Content-Type'] = 'application/json'
    }

    const res = await fetch(`/api${path}`, {
      method,
      headers,
      body: body !== undefined ? JSON.stringify(body) : undefined
    })

    if (res.status === 401) {
      auth.logout()
      router.push('/login')
      throw new Error('Unauthorized')
    }

    if (!res.ok) {
      const err = await res.json().catch(() => ({ error: res.statusText }))
      throw new Error(err.error || res.statusText)
    }

    if (res.status === 204) return undefined as T
    return res.json() as Promise<T>
  }

  return {
    get: <T>(path: string) => request<T>('GET', path),
    post: <T>(path: string, body?: unknown) => request<T>('POST', path, body),
    put: <T>(path: string, body?: unknown) => request<T>('PUT', path, body),
    del: <T>(path: string, body?: unknown) => request<T>('DELETE', path, body)
  }
}
