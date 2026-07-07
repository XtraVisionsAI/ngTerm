<script setup lang="ts">
  import { NButton, NCard, NSpace, NStatistic } from 'naive-ui'
  import { onActivated, onMounted, ref } from 'vue'
  import { useRouter } from 'vue-router'
  import { useApi } from '@/composables/useApi'

  const api = useApi()
  const router = useRouter()

  const userCount = ref(0)
  const activeSessionCount = ref(0)

  async function loadStats() {
    try {
      const users = await api.get<any[]>('/admin/users')
      userCount.value = users.length
    } catch {}
    try {
      const stats = await api.get<{ activeSessions: number }>('/admin/stats')
      activeSessionCount.value = stats.activeSessions
    } catch {}
  }

  onMounted(loadStats)
  onActivated(loadStats)
</script>

<template>
  <div class="h-full p-4">
    <h2 class="mb-4 text-lg font-bold">管理面板</h2>
    <n-space :size="16">
      <n-card class="w-48 cursor-pointer" hoverable @click="router.push('/admin/users')">
        <n-statistic label="用户数" :value="userCount" />
      </n-card>
      <n-card class="w-48">
        <n-statistic label="活跃会话" :value="activeSessionCount" />
      </n-card>
      <n-card class="w-48 cursor-pointer" hoverable @click="router.push('/audit')">
        <n-statistic label="审计日志">
          <template #default>
            <n-button text type="primary">查看</n-button>
          </template>
        </n-statistic>
      </n-card>
      <n-card class="w-48 cursor-pointer" hoverable @click="router.push('/admin/terminal')">
        <n-statistic label="服务器终端">
          <template #default>
            <n-button text type="primary">打开</n-button>
          </template>
        </n-statistic>
      </n-card>
    </n-space>
  </div>
</template>
