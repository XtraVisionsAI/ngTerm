<script setup lang="ts">
  import { NButton, NSpace, NTag } from 'naive-ui'
  import { useRouter } from 'vue-router'
  import { useAuthStore } from '@/stores/auth'

  const auth = useAuthStore()
  const router = useRouter()

  function handleLogout() {
    auth.logout()
    router.push('/login')
  }
</script>

<template>
  <div class="p-3 text-center" style="border-top: 1px solid var(--om-border)">
    <n-space vertical :size="8" align="center">
      <div class="flex items-center gap-2">
        <span class="truncate text-sm">{{ auth.username }}</span>
        <n-tag :type="auth.isAdmin ? 'warning' : 'info'" size="small">
          {{ auth.isAdmin ? '管理员' : '用户' }}
        </n-tag>
      </div>
      <n-button size="small" quaternary block type="error" @click="handleLogout">
        <template #icon>
          <i class="i-ri:logout-box-r-line" style="display: inline-block; width: 16px; height: 16px" />
        </template>
        退出登录
      </n-button>
    </n-space>
  </div>
</template>
