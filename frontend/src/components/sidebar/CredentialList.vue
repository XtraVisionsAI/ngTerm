<script setup lang="ts">
  import { NPopconfirm } from 'naive-ui'
  import { useServerStore } from '@/stores/server'

  defineEmits<{
    create: []
    delete: [id: string]
  }>()

  const serverStore = useServerStore()
</script>

<template>
  <div class="sidebar-section">
    <div class="section-list">
      <div v-for="key in serverStore.keys" :key="key.id" class="sidebar-item group">
        <i class="i-ri:key-2-line item-icon" />
        <span class="item-label">{{ key.name }}</span>
        <span class="item-actions opacity-0 group-hover:opacity-100">
          <n-popconfirm @positive-click="$emit('delete', key.id)">
            <template #trigger>
              <button @click.stop>
                <i class="i-ri:delete-bin-line action-icon" />
              </button>
            </template>
            确认删除此密钥？
          </n-popconfirm>
        </span>
      </div>
      <!-- Add button -->
      <button class="add-placeholder" @click="$emit('create')">
        <i class="i-ri:add-line" style="display: inline-block; width: 14px; height: 14px" />
        <span>添加凭证</span>
      </button>
    </div>
  </div>
</template>
