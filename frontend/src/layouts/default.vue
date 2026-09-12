<script setup lang="ts">
  import type { Server } from '@/stores/server'
  import { useMediaQuery, useNetwork } from '@vueuse/core'
  import { NButton, NDrawer, NDrawerContent, NLayout, useMessage } from 'naive-ui'
  import { onMounted, ref } from 'vue'
  import { useRoute, useRouter } from 'vue-router'
  import ActivityBar from '@/components/layout/ActivityBar.vue'
  import ConnectionList from '@/components/sidebar/ConnectionList.vue'
  import CredentialList from '@/components/sidebar/CredentialList.vue'
  import KeyFormModal from '@/components/sidebar/KeyFormModal.vue'
  import ServerFormModal from '@/components/sidebar/ServerFormModal.vue'
  import SshConfigImportModal from '@/components/sidebar/SshConfigImportModal.vue'
  import UserFooter from '@/components/sidebar/UserFooter.vue'
  import { useApi } from '@/composables/useApi'
  import { useAuthStore } from '@/stores/auth'
  import { useServerStore } from '@/stores/server'
  import { useSessionStore } from '@/stores/session'

  const router = useRouter()
  const route = useRoute()
  const auth = useAuthStore()
  const api = useApi()
  const message = useMessage()
  const serverStore = useServerStore()
  const sessionStore = useSessionStore()
  const { isOnline } = useNetwork()

  const isDesktop = useMediaQuery('(min-width: 768px)')
  const showMobileDrawer = ref(false)
  const activePanel = ref('')

  // --- Data loading ---
  onMounted(async () => {
    if (!auth.isAdmin) {
      await loadData()
      await restoreSessions()
    }
  })

  async function restoreSessions() {
    try {
      const sessions = await api.get<
        {
          id: string
          serverId: string
          serverAlias: string
          serverHost: string
          aiToolId?: string
          parentSessionId?: string | null
        }[]
      >('/sessions')
      for (const s of sessions) {
        if (s.parentSessionId) continue
        if (!sessionStore.tabs.find((t) => t.id === s.id)) {
          sessionStore.addTab(s)
        }
      }
      if (sessions.length > 0 && route.path !== '/') {
        router.push('/')
      }
    } catch {}
  }

  async function loadData() {
    try {
      const [servers, keys, groups, tools] = await Promise.all([
        api.get<Server[]>('/servers'),
        api.get<{ id: string; name: string; fingerprint: string; keyType: string; createdAt: string }[]>('/keys'),
        api.get<string[]>('/groups'),
        api.get<{ id: string; displayName: string }[]>('/tools')
      ])
      serverStore.setServers(servers)
      serverStore.setKeys(keys)
      serverStore.setGroups(groups)
      serverStore.setAiToolOptions(tools.map((t) => ({ label: t.displayName, value: t.id })))
    } catch {}
  }

  // --- Connect ---
  function handleConnect(server: Server) {
    const tempId = `pending-${Date.now()}`
    sessionStore.addTab(
      {
        id: tempId,
        serverId: server.id,
        serverAlias: server.alias || server.host,
        serverHost: server.host,
        aiToolId: server.aiToolId
      },
      'connecting'
    )
    if (route.path !== '/') router.push('/')
    showMobileDrawer.value = false
  }

  // --- Server CRUD ---
  const showServerModal = ref(false)
  const showImportModal = ref(false)
  const editingServer = ref<Server | null>(null)

  function openCreateServer() {
    editingServer.value = null
    showServerModal.value = true
  }

  function openEditServer(server: Server) {
    editingServer.value = server
    showServerModal.value = true
  }

  async function handleDeleteServer(id: string) {
    try {
      await api.del(`/servers/${id}`)
      message.success('已删除')
      await loadData()
    } catch (e: any) {
      message.error(e.message)
    }
  }

  // --- Key CRUD ---
  const showKeyModal = ref(false)

  async function handleDeleteKey(id: string) {
    try {
      await api.del(`/keys/${id}`)
      message.success('已删除')
      await loadData()
    } catch (e: any) {
      message.error(e.message)
    }
  }

  // --- Navigation ---
  function handleNavigate(path: string) {
    router.push(path)
    showMobileDrawer.value = false
  }
</script>

<template>
  <div class="h-screen flex overflow-hidden">
    <!-- Desktop: ActivityBar + Side Panel -->
    <template v-if="isDesktop">
      <activity-bar v-model:active-panel="activePanel" :is-admin="auth.isAdmin" @navigate="handleNavigate" />

      <!-- Collapsible side panel (user only) -->
      <div v-if="activePanel && !auth.isAdmin" class="side-panel">
        <div class="side-panel-header">
          <span class="text-xs font-semibold tracking-wide uppercase">
            {{ activePanel === 'connections' ? '连接管理' : '凭证管理' }}
          </span>
        </div>
        <div class="side-panel-content">
          <connection-list
            v-if="activePanel === 'connections'"
            @connect="handleConnect"
            @create="openCreateServer"
            @import="showImportModal = true"
            @edit="openEditServer"
            @delete="handleDeleteServer"
          />
          <credential-list
            v-else-if="activePanel === 'credentials'"
            @create="showKeyModal = true"
            @delete="handleDeleteKey"
          />
        </div>
        <user-footer />
      </div>
    </template>

    <!-- Main content -->
    <n-layout class="min-w-0 flex-1" content-class="h-full flex flex-col overflow-hidden">
      <!-- Mobile header -->
      <div
        v-if="!isDesktop"
        class="flex shrink-0 items-center gap-2 px-3 py-2"
        style="border-bottom: 1px solid var(--om-border); background: var(--om-bg)"
      >
        <n-button quaternary size="small" @click="showMobileDrawer = true">
          <template #icon>
            <i class="i-ri:menu-line" style="display: inline-block; width: 18px; height: 18px" />
          </template>
        </n-button>
        <span class="text-sm font-bold">OneMux</span>
      </div>
      <!-- Offline banner -->
      <div
        v-if="!isOnline"
        class="flex shrink-0 items-center justify-center gap-2 bg-amber-500 px-3 py-1.5 text-sm text-white"
      >
        <i class="i-ri:wifi-off-line" style="display: inline-block; width: 16px; height: 16px" />
        网络连接已断开，终端会话可能受影响
      </div>
      <div class="min-h-0 flex-1">
        <router-view v-slot="{ Component }">
          <keep-alive>
            <component :is="Component" />
          </keep-alive>
        </router-view>
      </div>
    </n-layout>
  </div>

  <!-- Mobile drawer -->
  <n-drawer v-model:show="showMobileDrawer" placement="left" :width="260">
    <n-drawer-content body-content-style="padding: 0">
      <div class="h-full flex flex-col" style="background: var(--om-bg)">
        <div class="p-4 text-center text-lg font-bold">OneMux</div>
        <div class="flex-1 overflow-y-auto">
          <connection-list
            @connect="handleConnect"
            @create="openCreateServer"
            @import="showImportModal = true"
            @edit="openEditServer"
            @delete="handleDeleteServer"
          />
        </div>
        <user-footer />
      </div>
    </n-drawer-content>
  </n-drawer>

  <!-- Modals -->
  <server-form-modal v-model:show="showServerModal" :editing-server="editingServer" @saved="loadData" />
  <ssh-config-import-modal v-model:show="showImportModal" @saved="loadData" />
  <key-form-modal v-model:show="showKeyModal" @saved="loadData" />
</template>

<style scoped>
  .side-panel {
    width: 200px;
    height: 100%;
    display: flex;
    flex-direction: column;
    border-right: 1px solid var(--om-border);
    overflow: hidden;
  }

  .side-panel-header {
    padding: 12px 12px 8px;
    color: var(--om-text-muted);
    border-bottom: 1px solid var(--om-border);
  }

  .side-panel-content {
    flex: 1;
    overflow-y: auto;
  }
</style>
