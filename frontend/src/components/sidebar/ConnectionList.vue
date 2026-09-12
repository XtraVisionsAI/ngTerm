<script setup lang="ts">
  import type { Server } from '@/stores/server'
  import { NDropdown, NInput } from 'naive-ui'
  import { computed, onMounted, ref } from 'vue'
  import { usePrefsStore } from '@/stores/prefs'
  import { useServerStore } from '@/stores/server'
  import { matchesServer } from '@/utils/servers'
  import ServerRow from './ServerRow.vue'

  const emit = defineEmits<{
    connect: [server: Server]
    create: []
    import: []
    edit: [server: Server]
    delete: [id: string]
  }>()

  const serverStore = useServerStore()
  const prefs = usePrefsStore()
  onMounted(() => prefs.load())

  const query = ref('')
  const visibleServers = computed(() => serverStore.servers.filter((s) => matchesServer(s, query.value)))
  const favoriteServers = computed(() => visibleServers.value.filter((s) => prefs.isFavorite(s.id)))
  const expandedGroups = ref<Set<string>>(new Set())

  function toggleGroup(group: string) {
    if (expandedGroups.value.has(group)) {
      expandedGroups.value.delete(group)
    } else {
      expandedGroups.value.add(group)
    }
  }

  const ungroupedServers = computed(() => visibleServers.value.filter((s) => !s.groupName))
  const groupedServers = computed(() => {
    const map = new Map<string, Server[]>()
    for (const s of visibleServers.value) {
      if (s.groupName) {
        if (!map.has(s.groupName)) map.set(s.groupName, [])
        map.get(s.groupName)!.push(s)
      }
    }
    return map
  })

  const ctxMenu = ref({ show: false, x: 0, y: 0, server: null as Server | null })
  const ctxOptions = computed(() => [
    { label: '连接', key: 'connect' },
    { label: ctxMenu.value.server && prefs.isFavorite(ctxMenu.value.server.id) ? '取消收藏' : '收藏', key: 'favorite' },
    { label: '编辑', key: 'edit' },
    { label: '复制地址', key: 'copyAddr' },
    { type: 'divider', key: 'd1' },
    { label: '删除', key: 'delete' }
  ])

  function handleServerContextMenu(e: MouseEvent, server: Server) {
    e.preventDefault()
    ctxMenu.value = { show: true, x: e.clientX, y: e.clientY, server }
  }

  async function handleCtxSelect(key: string) {
    ctxMenu.value.show = false
    const server = ctxMenu.value.server
    if (!server) return
    switch (key) {
      case 'connect':
        emit('connect', server)
        break
      case 'favorite':
        prefs.toggleFavorite(server.id)
        break
      case 'edit':
        emit('edit', server)
        break
      case 'copyAddr':
        await navigator.clipboard.writeText(`${server.username}@${server.host}:${server.port}`)
        break
      case 'delete':
        emit('delete', server.id)
        break
    }
  }
</script>

<template>
  <div class="sidebar-section">
    <div class="px-2 pb-1 pt-2">
      <n-input v-model:value="query" size="tiny" clearable placeholder="搜索 别名 / 主机 / 标签">
        <template #prefix><i class="i-ri:search-line block size-3" /></template>
      </n-input>
    </div>
    <div class="section-list">
      <!-- Favourites -->
      <template v-if="favoriteServers.length">
        <div class="px-2 pb-0.5 pt-1 text-[10px] text-om-dimmed tracking-wide uppercase">收藏</div>
        <server-row
          v-for="server in favoriteServers"
          :key="`fav-${server.id}`"
          :server="server"
          :favorite="true"
          @connect="emit('connect', server)"
          @edit="emit('edit', server)"
          @delete="emit('delete', server.id)"
          @favorite="prefs.toggleFavorite(server.id)"
          @contextmenu="handleServerContextMenu($event, server)"
        />
        <div class="mx-2 my-1 h-px bg-om-border" />
      </template>
      <!-- Ungrouped servers -->
      <server-row
        v-for="server in ungroupedServers"
        :key="server.id"
        :server="server"
        :favorite="prefs.isFavorite(server.id)"
        @connect="emit('connect', server)"
        @edit="emit('edit', server)"
        @delete="emit('delete', server.id)"
        @favorite="prefs.toggleFavorite(server.id)"
        @contextmenu="handleServerContextMenu($event, server)"
      />
      <!-- Grouped servers -->
      <div v-for="[group, servers] in groupedServers" :key="group">
        <div class="sidebar-item group-folder" @click="toggleGroup(group)">
          <i
            :class="expandedGroups.has(group) || query ? 'i-ri:folder-open-line' : 'i-ri:folder-line'"
            class="item-icon"
          />
          <span class="item-label">{{ group }}</span>
          <span class="text-[10px] text-om-dimmed">{{ servers.length }}</span>
        </div>
        <div v-if="expandedGroups.has(group) || query" class="pl-4">
          <server-row
            v-for="server in servers"
            :key="server.id"
            :server="server"
            :favorite="prefs.isFavorite(server.id)"
            @connect="emit('connect', server)"
            @edit="emit('edit', server)"
            @delete="emit('delete', server.id)"
            @favorite="prefs.toggleFavorite(server.id)"
            @contextmenu="handleServerContextMenu($event, server)"
          />
        </div>
      </div>
      <div v-if="query && visibleServers.length === 0" class="px-3 py-2 text-xs text-om-dimmed">没有匹配的服务器</div>
      <!-- Add / import -->
      <button class="add-placeholder" @click="emit('create')">
        <i class="i-ri:add-line" style="display: inline-block; width: 14px; height: 14px" />
        <span>添加连接</span>
      </button>
      <button class="add-placeholder" @click="emit('import')">
        <i class="i-ri:download-2-line" style="display: inline-block; width: 14px; height: 14px" />
        <span>从 SSH config 导入</span>
      </button>
    </div>

    <n-dropdown
      trigger="manual"
      placement="bottom-start"
      :show="ctxMenu.show"
      :x="ctxMenu.x"
      :y="ctxMenu.y"
      :options="ctxOptions"
      @select="handleCtxSelect"
      @clickoutside="ctxMenu.show = false"
    />
  </div>
</template>
