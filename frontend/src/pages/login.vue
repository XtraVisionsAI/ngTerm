<script setup lang="ts">
  import { NButton, NCard, NForm, NFormItem, NInput, useMessage } from 'naive-ui'
  import { ref } from 'vue'
  import { useRouter } from 'vue-router'
  import { useAuthStore } from '@/stores/auth'

  definePage({
    meta: {
      layout: false
    }
  })

  const router = useRouter()
  const auth = useAuthStore()
  const message = useMessage()

  const mode = ref<'user' | 'admin'>('user')
  const userForm = ref({ username: '', password: '' })
  const adminForm = ref({ masterKey: '' })
  const loading = ref(false)

  async function handleUserLogin() {
    loading.value = true
    try {
      await auth.login(userForm.value.username, userForm.value.password)
      router.push('/')
    } catch (e: any) {
      message.error(e.message || '登录失败')
    } finally {
      loading.value = false
    }
  }

  async function handleAdminLogin() {
    loading.value = true
    try {
      await auth.adminLogin(adminForm.value.masterKey)
      router.push('/admin')
    } catch (e: any) {
      message.error(e.message || '登录失败')
    } finally {
      loading.value = false
    }
  }
</script>

<template>
  <div class="h-screen flex items-center justify-center bg-om-panel">
    <n-card class="w-96" title="OneMux">
      <!-- User login -->
      <template v-if="mode === 'user'">
        <n-form @submit.prevent="handleUserLogin">
          <n-form-item label="用户名">
            <n-input v-model:value="userForm.username" placeholder="用户名" />
          </n-form-item>
          <n-form-item label="密码">
            <n-input v-model:value="userForm.password" type="password" placeholder="密码" show-password-on="click" />
          </n-form-item>
          <div class="flex items-center justify-between">
            <a class="cursor-pointer text-xs text-om-dimmed hover:text-om-primary" @click="mode = 'admin'">
              管理员登录 &rarr;
            </a>
            <n-button type="primary" :loading="loading" attr-type="submit">登录</n-button>
          </div>
        </n-form>
      </template>

      <!-- Admin login -->
      <template v-else>
        <n-form @submit.prevent="handleAdminLogin">
          <n-form-item label="Master Key">
            <n-input
              v-model:value="adminForm.masterKey"
              type="password"
              placeholder="管理员密钥"
              show-password-on="click"
            />
          </n-form-item>
          <div class="flex items-center justify-between">
            <a class="cursor-pointer text-xs text-om-dimmed hover:text-om-primary" @click="mode = 'user'">
              &larr; 用户登录
            </a>
            <n-button type="primary" :loading="loading" attr-type="submit">登录</n-button>
          </div>
        </n-form>
      </template>
    </n-card>
  </div>
</template>
