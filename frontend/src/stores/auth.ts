import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { useSessionStore } from './session'

export const useAuthStore = defineStore('auth', () => {
  const token = ref<string | null>(localStorage.getItem('onemux_token'))
  const userId = ref<string | null>(localStorage.getItem('onemux_user_id'))
  const username = ref<string | null>(localStorage.getItem('onemux_username'))
  const role = ref<string | null>(localStorage.getItem('onemux_role'))

  const isAuthenticated = computed(() => !!token.value)
  const isAdmin = computed(() => role.value === 'admin')

  async function login(user: string, password: string) {
    const res = await fetch('/api/auth/login', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ username: user, password })
    })

    if (!res.ok) {
      const err = await res.json().catch(() => ({ error: 'Login failed' }))
      throw new Error(err.error || 'Login failed')
    }

    const data = await res.json()
    setSession(data)
  }

  async function adminLogin(masterKey: string) {
    const res = await fetch('/api/auth/admin-login', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ masterKey })
    })

    if (!res.ok) {
      const err = await res.json().catch(() => ({ error: 'Login failed' }))
      throw new Error(err.error || 'Login failed')
    }

    const data = await res.json()
    setSession(data)
  }

  function setSession(data: { token: string; userId: string; username: string; role: string }) {
    token.value = data.token
    userId.value = data.userId
    username.value = data.username
    role.value = data.role

    localStorage.setItem('onemux_token', data.token)
    localStorage.setItem('onemux_user_id', data.userId)
    localStorage.setItem('onemux_username', data.username)
    localStorage.setItem('onemux_role', data.role)
  }

  async function check(): Promise<boolean> {
    if (!token.value) return false
    try {
      const res = await fetch('/api/auth/check', {
        headers: { Authorization: `Bearer ${token.value}` }
      })
      if (!res.ok) {
        logout()
        return false
      }
      return true
    } catch {
      return false
    }
  }

  function logout() {
    token.value = null
    userId.value = null
    username.value = null
    role.value = null
    localStorage.removeItem('onemux_token')
    localStorage.removeItem('onemux_user_id')
    localStorage.removeItem('onemux_username')
    localStorage.removeItem('onemux_role')
    useSessionStore().clearAll()
  }

  return { token, userId, username, role, isAuthenticated, isAdmin, login, adminLogin, check, logout }
})
