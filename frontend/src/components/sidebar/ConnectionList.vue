<script setup lang="ts">
  import type { Server } from '@/stores/server'
  import { NDropdown, NPopconfirm } from 'naive-ui'
  import { computed, ref } from 'vue'
  import { useServerStore } from '@/stores/server'

  const emit = defineEmits<{
    connect: [server: Server]
    create: []
    edit: [server: Server]
    delete: [id: string]
  }>()

  const serverStore = useServerStore()
  const expandedGroups = ref<Set<string>>(new Set())

  function toggleGroup(group: string) {
    if (expandedGroups.value.has(group)) {
      expandedGroups.value.delete(group)
    } else {
      expandedGroups.value.add(group)
    }
  }

  const ungroupedServers = computed(() => serverStore.servers.filter((s) => !s.groupName))
  const groupedServers = computed(() => {
    const map = new Map<string, Server[]>()
    for (const s of serverStore.servers) {
      if (s.groupName) {
        if (!map.has(s.groupName)) map.set(s.groupName, [])
        map.get(s.groupName)!.push(s)
      }
    }
    return map
  })

  const ctxMenu = ref({ show: false, x: 0, y: 0, server: null as Server | null })
  const ctxOptions = [
    { label: '连接', key: 'connect' },
    { label: '编辑', key: 'edit' },
    { label: '复制地址', key: 'copyAddr' },
    { type: 'divider', key: 'd1' },
    { label: '删除', key: 'delete' }
  ]

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
    <div class="section-list">
      <!-- Ungrouped servers -->
      <div
        v-for="server in ungroupedServers"
        :key="server.id"
        :title="`${server.alias} (${server.host})`"
        class="sidebar-item group select-none"
        @dblclick="emit('connect', server)"
        @contextmenu="handleServerContextMenu($event, server)"
      >
        <i class="i-ri:terminal-line item-icon" />
        <span class="item-label">{{ server.alias || server.host }}</span>
        <span class="item-actions opacity-0 group-hover:opacity-100">
          <button @click.stop="emit('edit', server)">
            <i class="i-ri:edit-line action-icon" />
          </button>
          <n-popconfirm @positive-click="emit('delete', server.id)">
            <template #trigger>
              <button @click.stop>
                <i class="i-ri:delete-bin-line action-icon" />
              </button>
            </template>
            确认删除此服务器？
          </n-popconfirm>
        </span>
      </div>
      <!-- Grouped servers -->
      <div v-for="[group, servers] in groupedServers" :key="group">
        <div class="sidebar-item group-folder" @click="toggleGroup(group)">
          <i :class="expandedGroups.has(group) ? 'i-ri:folder-open-line' : 'i-ri:folder-line'" class="item-icon" />
          <span class="item-label">{{ group }}</span>
        </div>
        <div v-if="expandedGroups.has(group)" class="pl-4">
          <div
            v-for="server in servers"
            :key="server.id"
            :title="`${server.alias} (${server.host})`"
            class="sidebar-item group select-none"
            @dblclick="emit('connect', server)"
            @contextmenu="handleServerContextMenu($event, server)"
          >
            <i class="i-ri:terminal-line item-icon" />
            <span class="item-label">{{ server.alias || server.host }}</span>
            <span class="item-actions opacity-0 group-hover:opacity-100">
              <button @click.stop="emit('edit', server)">
                <i class="i-ri:edit-line action-icon" />
              </button>
              <n-popconfirm @positive-click="emit('delete', server.id)">
                <template #trigger>
                  <button @click.stop>
                    <i class="i-ri:delete-bin-line action-icon" />
                  </button>
                </template>
                确认删除此服务器？
              </n-popconfirm>
            </span>
          </div>
        </div>
      </div>
      <!-- Add button -->
      <button class="add-placeholder" @click="emit('create')">
        <i class="i-ri:add-line" style="display: inline-block; width: 14px; height: 14px" />
        <span>添加连接</span>
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
