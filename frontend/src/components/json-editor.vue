<script setup lang="ts">
  import { json } from '@codemirror/lang-json'
  import { oneDark } from '@codemirror/theme-one-dark'
  import { NButton } from 'naive-ui'
  import { computed, ref } from 'vue'
  import { Codemirror } from 'vue-codemirror'

  const props = withDefaults(
    defineProps<{
      modelValue: string
      placeholder?: string
      rows?: number
    }>(),
    { placeholder: '{}', rows: 6 }
  )

  const emit = defineEmits<{
    (e: 'update:modelValue', value: string): void
  }>()

  const extensions = [json(), oneDark]
  const height = computed(() => `${props.rows * 20}px`)
  const formatError = ref('')

  function handleChange(value: string) {
    emit('update:modelValue', value)
    formatError.value = ''
  }

  function formatJson() {
    try {
      const parsed = JSON.parse(props.modelValue)
      emit('update:modelValue', JSON.stringify(parsed, null, 2))
      formatError.value = ''
    } catch (e: any) {
      formatError.value = e.message
    }
  }
</script>

<template>
  <div class="w-full">
    <div class="overflow-hidden border border-om-border rounded">
      <codemirror
        :model-value="modelValue"
        :placeholder="placeholder"
        :extensions="extensions"
        :style="{ height }"
        @update:model-value="handleChange"
      />
    </div>
    <div class="mt-1 flex items-center gap-2">
      <n-button size="tiny" quaternary @click="formatJson">格式化</n-button>
      <span v-if="formatError" class="text-xs text-red-400">{{ formatError }}</span>
    </div>
  </div>
</template>
