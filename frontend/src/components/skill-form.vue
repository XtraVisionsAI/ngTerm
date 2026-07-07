<script setup lang="ts">
  import { NButton, NInput, NSpace } from 'naive-ui'

  export interface SkillItem {
    name: string
    description: string
    prompt: string
  }

  const model = defineModel<SkillItem[]>({ default: () => [] })

  function addSkill() {
    model.value.push({ name: '', description: '', prompt: '' })
  }

  function removeSkill(idx: number) {
    model.value.splice(idx, 1)
  }
</script>

<template>
  <div class="w-full">
    <div class="mb-2 flex items-center justify-between">
      <span class="text-sm text-om-dimmed">Skills</span>
      <n-button size="tiny" dashed @click="addSkill">+ 添加</n-button>
    </div>

    <div v-if="model.length === 0" class="py-2 text-center text-xs text-om-dimmed op-60"> 暂无 Skill 配置 </div>

    <div v-for="(skill, idx) in model" :key="idx" class="mb-3 border border-om-border rounded p-3">
      <div class="mb-2 flex items-center justify-between">
        <span class="text-xs font-medium">Skill #{{ idx + 1 }}</span>
        <n-button size="tiny" quaternary type="error" @click="removeSkill(idx)">
          <template #icon>
            <i class="i-ri:delete-bin-line" style="display: inline-block; width: 14px; height: 14px" />
          </template>
        </n-button>
      </div>

      <n-space vertical :size="8">
        <div class="flex gap-2">
          <n-input v-model:value="skill.name" placeholder="名称 (如 health-check)" size="small" class="w-40" />
          <n-input
            v-model:value="skill.description"
            placeholder="描述 (如 服务器健康巡检)"
            size="small"
            class="flex-1"
          />
        </div>
        <n-input
          v-model:value="skill.prompt"
          type="textarea"
          placeholder="Skill 指令内容 (LLM 激活时注入的 prompt)"
          size="small"
          :rows="4"
        />
      </n-space>
    </div>
  </div>
</template>
