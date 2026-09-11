<script setup lang="ts">
  import type { ChangePreview } from '@/utils/changes'
  import { computed } from 'vue'
  import { diffLineClass } from '@/utils/changes'

  const props = defineProps<{ preview: ChangePreview; compact?: boolean }>()

  const headline = computed(() => {
    const p = props.preview
    if (p.unavailable) return '无法在服务端解析目标文件，变更未绑定基线'
    if (p.exists === false) return '新建文件'
    return `修改现有文件（${p.size ?? '?'} 字节）`
  })

  const baselineShort = computed(() => {
    const b = props.preview.baseline
    if (!b) return ''
    return b.length > 12 ? `${b.slice(0, 12)}…` : b
  })
</script>

<template>
  <div class="change-preview text-xs">
    <div class="mb-1 flex flex-wrap items-center gap-2">
      <i class="i-ri:file-edit-line" style="display: inline-block; width: 14px; height: 14px" />
      <span class="break-all font-mono">{{ preview.path }}</span>
      <span class="text-om-dimmed">{{ headline }}</span>
      <template v-if="preview.diff">
        <span class="text-om-success">+{{ preview.diff.added }}</span>
        <span class="text-om-error">-{{ preview.diff.removed }}</span>
      </template>
      <span v-if="preview.newLines !== undefined" class="text-om-dimmed">→ {{ preview.newLines }} 行</span>
    </div>
    <div v-if="baselineShort" class="mb-1 text-om-dimmed">
      基线 <span class="font-mono">{{ baselineShort }}</span>
      <span v-if="preview.backup" class="ml-2">· 写入前将保留备份，可恢复</span>
      <span v-else-if="preview.exists === false" class="ml-2">· 新文件，无需备份</span>
    </div>
    <div v-if="preview.reason" class="mb-1 text-om-warning">{{ preview.reason }}</div>
    <pre
      v-if="preview.diff && preview.diff.lines.length > 0"
      class="diff-body overflow-auto whitespace-pre rounded bg-om-bg p-2 font-mono"
      :class="compact ? 'max-h-48' : 'max-h-80'"
    ><span v-for="(line, i) in preview.diff.lines" :key="i" class="diff-line" :class="diffLineClass(line)">{{ line }}
</span></pre>
    <div v-if="preview.diff?.truncated" class="mt-1 text-om-warning">diff 已截断，仅显示前面部分；统计为估算值。</div>
  </div>
</template>

<style scoped>
  .diff-line {
    display: block;
  }
  .diff-add {
    color: var(--om-success, #3fb950);
    background: rgba(63, 185, 80, 0.12);
  }
  .diff-del {
    color: var(--om-error, #f85149);
    background: rgba(248, 81, 73, 0.12);
  }
  .diff-hunk {
    color: var(--om-text-dimmed);
    font-style: italic;
  }
  .diff-ctx {
    color: var(--om-text);
    opacity: 0.85;
  }
</style>
