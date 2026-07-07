<script setup lang="ts">
  import { computed } from 'vue'
  import { useRoute, useRouter } from 'vue-router'
  import { useAuthStore } from '@/stores/auth'

  const activeKey = defineModel<string>('activePanel')

  const props = defineProps<{
    isAdmin: boolean
  }>()

  const emit = defineEmits<{
    navigate: [path: string]
  }>()

  const auth = useAuthStore()
  const router = useRouter()

  interface ActivityItem {
    icon: string
    key: string
    tooltip: string
  }

  const userItems: ActivityItem[] = [
    { icon: 'i-ri:terminal-box-line', key: '/', tooltip: '终端' },
    { icon: 'i-ri:links-line', key: 'connections', tooltip: '连接' },
    { icon: 'i-ri:shield-keyhole-line', key: 'credentials', tooltip: '凭证' },
    { icon: 'i-ri:tools-line', key: '/tools', tooltip: '工具配置' },
    { icon: 'i-ri:file-list-3-line', key: '/audit', tooltip: '审计日志' }
  ]

  const adminItems: ActivityItem[] = [
    { icon: 'i-ri:dashboard-line', key: '/admin', tooltip: '面板' },
    { icon: 'i-ri:user-settings-line', key: '/admin/users', tooltip: '用户管理' },
    { icon: 'i-ri:robot-2-line', key: '/admin/tools', tooltip: 'AI 工具' },
    { icon: 'i-ri:file-list-3-line', key: '/audit', tooltip: '审计日志' }
  ]

  const items = computed(() => (props.isAdmin ? adminItems : userItems))

  const route = useRoute()

  function handleClick(item: ActivityItem) {
    if (item.key.startsWith('/')) {
      activeKey.value = ''
      emit('navigate', item.key)
    } else {
      activeKey.value = activeKey.value === item.key ? '' : item.key
    }
  }

  function isActive(item: ActivityItem): boolean {
    if (item.key.startsWith('/')) {
      if (activeKey.value) return false
      const matchingItems = items.value.filter(
        (i) => i.key.startsWith('/') && (route.path === i.key || route.path.startsWith(`${i.key}/`))
      )
      const bestMatch = matchingItems.sort((a, b) => b.key.length - a.key.length)[0]
      return bestMatch?.key === item.key
    }
    return activeKey.value === item.key
  }

  function handleLogout() {
    auth.logout()
    router.push('/login')
  }
</script>

<template>
  <div class="activity-bar">
    <div class="activity-items">
      <button
        v-for="item in items"
        :key="item.key"
        :title="item.tooltip"
        class="activity-item"
        :class="{ active: isActive(item) }"
        @click="handleClick(item)"
      >
        <i :class="item.icon" class="activity-icon" />
      </button>
    </div>
    <div class="activity-bottom">
      <button title="退出登录" class="activity-item" @click="handleLogout">
        <i class="i-ri:logout-box-r-line activity-icon" />
      </button>
    </div>
  </div>
</template>

<style scoped>
  .activity-bar {
    width: 48px;
    height: 100%;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: space-between;
    border-right: 1px solid var(--om-border);
    padding: 8px 0;
    flex-shrink: 0;
  }

  .activity-items {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .activity-item {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 36px;
    height: 36px;
    border-radius: var(--om-radius-sm);
    border: none;
    background: transparent;
    color: var(--om-text-dimmed);
    cursor: pointer;
    transition: color 0.15s;
    position: relative;
  }

  .activity-item:hover {
    color: var(--om-text);
  }

  .activity-item.active {
    color: var(--om-primary);
  }

  .activity-item.active::before {
    content: '';
    position: absolute;
    left: -6px;
    top: 8px;
    bottom: 8px;
    width: 2px;
    background: var(--om-primary);
    border-radius: 1px;
  }

  .activity-icon {
    display: inline-block;
    width: 20px;
    height: 20px;
  }

  .activity-bottom {
    display: flex;
    flex-direction: column;
    align-items: center;
    padding-bottom: 8px;
  }
</style>
