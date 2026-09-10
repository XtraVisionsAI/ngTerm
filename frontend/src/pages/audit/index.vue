<script setup lang="ts">
  import { NTabPane, NTabs } from 'naive-ui'
  import { ref } from 'vue'
  import { useAuthStore } from '@/stores/auth'
  import AuditConnections from './components/audit-connections.vue'
  import AuditOperations from './components/audit-operations.vue'
  import AuditSessions from './components/audit-sessions.vue'
  import AuditSystem from './components/audit-system.vue'

  const auth = useAuthStore()
  const tab = ref<'sessions' | 'operations' | 'connections' | 'system'>('sessions')
</script>

<template>
  <div class="h-full flex flex-col p-4">
    <div class="mb-2 flex items-center justify-between">
      <h2 class="text-lg font-bold">审计</h2>
    </div>
    <n-tabs v-model:value="tab" type="line" animated class="min-h-0 flex flex-1 flex-col" pane-class="min-h-0 flex-1">
      <n-tab-pane name="sessions" tab="会话与录像" display-directive="show:lazy">
        <audit-sessions />
      </n-tab-pane>
      <n-tab-pane name="operations" tab="操作检索" display-directive="show:lazy">
        <audit-operations />
      </n-tab-pane>
      <n-tab-pane name="connections" tab="连接日志" display-directive="show:lazy">
        <audit-connections />
      </n-tab-pane>
      <n-tab-pane v-if="auth.isAdmin" name="system" tab="系统事件" display-directive="show:lazy">
        <audit-system />
      </n-tab-pane>
    </n-tabs>
  </div>
</template>

<style scoped>
  :deep(.n-tabs-pane-wrapper) {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  :deep(.n-tab-pane) {
    height: 100%;
  }
</style>
