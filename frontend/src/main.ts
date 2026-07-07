import { createPinia } from 'pinia'
import { setupLayouts } from 'virtual:meta-layouts'
import { createApp } from 'vue'
import { createRouter, createWebHashHistory } from 'vue-router'
import { routes } from 'vue-router/auto-routes'

import App from './App.vue'
import { useAuthStore } from './stores/auth'

import 'virtual:uno.css'
import '@xterm/xterm/css/xterm.css'
import './styles/base.css'

const app = createApp(App)
const pinia = createPinia()

const router = createRouter({
  history: createWebHashHistory(),
  routes: setupLayouts(routes)
})

app.use(pinia)

const auth = useAuthStore()
const sessionReady = auth.check()

router.beforeEach(async (to) => {
  await sessionReady
  const authStore = useAuthStore()
  if (!authStore.isAuthenticated && to.path !== '/login') {
    return { path: '/login', query: { redirect: to.fullPath } }
  }
  if (authStore.isAuthenticated && to.path === '/login') {
    return { path: authStore.isAdmin ? '/admin' : '/' }
  }
  if (authStore.isAuthenticated && authStore.isAdmin) {
    const adminAllowed = ['/admin', '/audit']
    if (!adminAllowed.some((p) => to.path.startsWith(p))) {
      return { path: '/admin' }
    }
  }
})

app.use(router)
app.mount('#app')
