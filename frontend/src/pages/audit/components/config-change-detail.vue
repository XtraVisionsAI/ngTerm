<script setup lang="ts">
  import type { ConfigChangePayload } from '@/utils/audit'
  import { NTag } from 'naive-ui'
  import { computed } from 'vue'
  import { actionLabel, objectKindLabel } from '@/utils/audit'

  const props = defineProps<{ payload: ConfigChangePayload }>()

  function show(v: unknown): string {
    if (v === null || v === undefined) return '—'
    if (typeof v === 'string') return v
    return JSON.stringify(v)
  }

  /** Every top-level key of both snapshots, changed ones first. */
  const rows = computed(() => {
    const before = props.payload.before || {}
    const after = props.payload.after || {}
    const changed = new Set(props.payload.changedFields || [])
    const keys = Array.from(new Set([...Object.keys(before), ...Object.keys(after)]))
    keys.sort((a, b) => Number(changed.has(b)) - Number(changed.has(a)) || a.localeCompare(b))
    return keys.map((k) => ({ key: k, before: before[k], after: after[k], changed: changed.has(k) }))
  })
</script>

<template>
  <div class="text-sm">
    <div class="mb-2 flex flex-wrap items-center gap-2">
      <n-tag size="small">{{ objectKindLabel[payload.object] || payload.object }}</n-tag>
      <n-tag size="small" type="info">{{ actionLabel[payload.action] || payload.action }}</n-tag>
      <span v-if="payload.objectId" class="text-xs font-mono opacity-70">{{ payload.objectId }}</span>
      <n-tag v-if="payload.outcome === 'failed'" size="small" type="error">未生效</n-tag>
    </div>
    <p v-if="payload.error" class="mb-2 text-xs text-red-500">{{ payload.error }}</p>
    <p v-if="rows.length === 0" class="text-xs opacity-60">该变更没有可记录的字段快照（例如密码本身不会被记录）。</p>
    <div v-else class="overflow-x-auto">
      <table class="w-full border-collapse text-xs">
        <thead>
          <tr class="opacity-60">
            <th class="py-1 pr-3 text-left font-normal">字段</th>
            <th class="py-1 pr-3 text-left font-normal">变更前</th>
            <th class="py-1 text-left font-normal">变更后</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="r in rows" :key="r.key" :class="r.changed ? 'font-medium' : 'opacity-60'">
            <td class="py-1 pr-3 align-top font-mono">
              {{ r.key }}
              <span v-if="r.changed" class="ml-1 text-amber-500">●</span>
            </td>
            <td class="max-w-64 break-all py-1 pr-3 align-top font-mono">{{ show(r.before) }}</td>
            <td class="max-w-64 break-all py-1 align-top font-mono">{{ show(r.after) }}</td>
          </tr>
        </tbody>
      </table>
    </div>
    <p class="mt-2 text-xs opacity-50">
      密码、私钥和环境变量值不会写入审计；密钥只记录指纹与名称，环境变量只记录键名。
    </p>
  </div>
</template>
