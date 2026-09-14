<script setup lang="ts">
  /**
   * Shared list state: a failed load is shown as a failure with a retry, never
   * as an empty list ("no records", "working tree clean"). While the first
   * load runs the spinner shows; reloads with data present keep the content.
   */
  import { NButton, NEmpty, NSpin } from 'naive-ui'

  withDefaults(
    defineProps<{
      loading: boolean
      error: string | null
      empty: boolean
      emptyText?: string
      size?: 'small' | 'medium'
    }>(),
    { emptyText: '暂无数据', size: 'medium' }
  )
  const emit = defineEmits<{ retry: [] }>()
</script>

<template>
  <div v-if="error" class="flex flex-1 flex-col items-center justify-center gap-2 p-4 text-center" role="alert">
    <i class="i-ri:error-warning-line inline-block size-6 text-om-danger" />
    <div class="text-sm">加载失败</div>
    <div class="max-w-md break-all text-xs text-om-dimmed">{{ error }}</div>
    <n-button size="small" @click="emit('retry')">重试</n-button>
  </div>
  <div v-else-if="loading && empty" class="flex flex-1 items-center justify-center p-4" aria-busy="true">
    <n-spin :size="size" />
  </div>
  <div v-else-if="empty" class="flex flex-1 items-center justify-center p-4">
    <n-empty :description="emptyText" :size="size">
      <template v-if="$slots['empty-extra']" #extra><slot name="empty-extra" /></template>
    </n-empty>
  </div>
  <slot v-else />
</template>
