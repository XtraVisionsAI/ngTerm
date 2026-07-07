<script setup lang="ts">
  import { NButton, NInput, NSpace } from 'naive-ui'

  export interface McpServerItem {
    name: string
    command: string
    args: string
    env: string
  }

  const model = defineModel<McpServerItem[]>({ default: () => [] })

  function addServer() {
    model.value.push({ name: '', command: '', args: '', env: '' })
  }

  function removeServer(idx: number) {
    model.value.splice(idx, 1)
  }
</script>

<template>
  <div class="w-full">
    <div class="mb-2 flex items-center justify-between">
      <span class="text-sm text-om-dimmed">MCP Servers</span>
      <n-button size="tiny" dashed @click="addServer">+ 添加</n-button>
    </div>

    <div v-if="model.length === 0" class="py-2 text-center text-xs text-om-dimmed op-60"> 暂无 MCP Server 配置 </div>

    <div v-for="(server, idx) in model" :key="idx" class="mb-3 border border-om-border rounded p-3">
      <div class="mb-2 flex items-center justify-between">
        <span class="text-xs font-medium">Server #{{ idx + 1 }}</span>
        <n-button size="tiny" quaternary type="error" @click="removeServer(idx)">
          <template #icon>
            <i class="i-ri:delete-bin-line" style="display: inline-block; width: 14px; height: 14px" />
          </template>
        </n-button>
      </div>

      <n-space vertical :size="8">
        <div class="flex gap-2">
          <n-input v-model:value="server.name" placeholder="名称 (如 kubernetes)" size="small" class="w-40" />
          <n-input v-model:value="server.command" placeholder="命令 (如 npx)" size="small" class="flex-1" />
        </div>
        <n-input v-model:value="server.args" placeholder="参数 (空格分隔, 如 -y @kubernetes/mcp-server)" size="small" />
        <n-input v-model:value="server.env" placeholder="环境变量 (KEY=VALUE, 逗号分隔)" size="small" />
      </n-space>
    </div>
  </div>
</template>
