<script setup lang="ts">
  import type { Server } from '@/stores/server'
  import { NPopconfirm, NTag } from 'naive-ui'
  import { computed } from 'vue'
  import { envBadge, plainTags } from '@/utils/servers'

  const props = defineProps<{ server: Server; favorite: boolean }>()
  const emit = defineEmits<{
    connect: []
    edit: []
    delete: []
    favorite: []
    contextmenu: [e: MouseEvent]
  }>()

  const env = computed(() => envBadge(props.server.tags))
  const tags = computed(() => plainTags(props.server.tags))
  const title = computed(() => {
    const t = [`${props.server.username}@${props.server.host}:${props.server.port}`]
    if (props.server.tags?.length) t.push(props.server.tags.join(', '))
    return t.join('\n')
  })
</script>

<template>
  <div
    :title="title"
    class="sidebar-item group select-none"
    @dblclick="emit('connect')"
    @contextmenu="emit('contextmenu', $event)"
  >
    <i class="i-ri:terminal-line item-icon" />
    <span class="item-label">{{ server.alias || server.host }}</span>
    <n-tag v-if="env" size="tiny" :type="env.type" :bordered="false" class="ml-1 shrink-0 text-[9px]">{{
      env.label
    }}</n-tag>
    <span v-if="tags.length" class="ml-1 shrink-0 text-[10px] text-om-dimmed" :title="tags.join(', ')"
      >#{{ tags.length }}</span
    >
    <span class="item-actions" :class="favorite ? '' : 'opacity-0 group-hover:opacity-100'">
      <button :title="favorite ? '取消收藏' : '收藏'" @click.stop="emit('favorite')">
        <i :class="favorite ? 'i-ri:star-fill text-om-warning' : 'i-ri:star-line'" class="action-icon" />
      </button>
      <button class="opacity-0 group-hover:opacity-100" @click.stop="emit('edit')">
        <i class="i-ri:edit-line action-icon" />
      </button>
      <n-popconfirm @positive-click="emit('delete')">
        <template #trigger>
          <button class="opacity-0 group-hover:opacity-100" @click.stop>
            <i class="i-ri:delete-bin-line action-icon" />
          </button>
        </template>
        确认删除此服务器？
      </n-popconfirm>
    </span>
  </div>
</template>
