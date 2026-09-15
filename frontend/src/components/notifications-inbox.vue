<script setup lang="ts">
  /**
   * Notification inbox (UX-08): lists this person's notifications, marks one or
   * all read, and links back to the flows page for scheduled-run results.
   * Rendered inside a drawer opened from the activity bar bell.
   */
  import type { Notification } from '@/stores/notifications'
  import { NButton, NEmpty, NSpin, NTag } from 'naive-ui'
  import { onMounted } from 'vue'
  import { useRouter } from 'vue-router'
  import { useNotificationsStore } from '@/stores/notifications'
  import { formatTime } from '@/utils/format'

  const emit = defineEmits<{ close: [] }>()
  const store = useNotificationsStore()
  const router = useRouter()

  onMounted(() => store.load())

  const levelTag: Record<Notification['level'], 'info' | 'warning' | 'error'> = {
    info: 'info',
    warning: 'warning',
    error: 'error'
  }

  async function open(n: Notification) {
    if (!n.read) await store.markRead(n.notifId)
    if (n.link) {
      emit('close')
      router.push(n.link)
    }
  }
</script>

<template>
  <div class="flex flex-col gap-2">
    <div class="flex items-center justify-between">
      <span class="text-xs text-om-dimmed">共 {{ store.total }} 条 · 未读 {{ store.unread }}</span>
      <n-button size="tiny" quaternary :disabled="!store.unread" @click="store.markAllRead()">全部标记已读</n-button>
    </div>

    <n-spin :show="store.loading">
      <n-empty v-if="!store.items.length && !store.loading" description="暂无通知" size="small" class="py-8" />
      <div
        v-for="n in store.items"
        :key="n.notifId"
        class="mb-2 cursor-pointer border border-om-border rounded p-2 text-xs hover:bg-om-hover"
        :class="n.read ? 'opacity-60' : ''"
        @click="open(n)"
      >
        <div class="flex items-center gap-2">
          <span v-if="!n.read" class="h-1.5 w-1.5 shrink-0 rounded-full bg-om-primary" />
          <n-tag size="tiny" :type="levelTag[n.level]" :bordered="false">{{ n.kind }}</n-tag>
          <span class="font-semibold">{{ n.title }}</span>
          <span class="ml-auto text-om-dimmed">{{ formatTime(n.createdAt) }}</span>
        </div>
        <div class="mt-1 whitespace-pre-wrap text-om-dimmed">{{ n.body }}</div>
      </div>
    </n-spin>
  </div>
</template>
