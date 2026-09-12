<script setup lang="ts">
  import type { AiContextItem } from '@/stores/aiContext'
  /**
   * Context items queued for the next AI message: chips above the chat input
   * with review (preview / trim / redact / remove). Items bound to a server
   * other than the agent's are marked and cannot be sent.
   */
  import { NButton, NInput, NModal, NTag, NTooltip } from 'naive-ui'
  import { computed, ref } from 'vue'
  import { kindLabel, useAiContextStore } from '@/stores/aiContext'
  import { looksSensitive, redactText } from '@/utils/redact'

  const props = defineProps<{
    tabId: string
    /** Server the agent of this chat runs against. */
    serverId: string
  }>()

  const store = useAiContextStore()
  const items = computed(() => store.forTab(props.tabId))

  function mismatched(it: AiContextItem): boolean {
    return it.serverId !== props.serverId
  }

  const editing = ref<AiContextItem | null>(null)
  const draft = ref('')
  const draftRedacted = ref(0)

  function openEdit(it: AiContextItem) {
    editing.value = it
    draft.value = it.text
    draftRedacted.value = 0
  }

  function applyRedact() {
    const r = redactText(draft.value)
    draft.value = r.text
    draftRedacted.value += r.count
  }

  function saveEdit() {
    const it = editing.value
    if (!it) return
    store.update(it.id, {
      text: draft.value,
      // Trimmed by hand when the text changed for a reason other than redaction.
      edited: it.edited || (draft.value !== it.text && draftRedacted.value === 0),
      redactedCount: (it.redactedCount || 0) + draftRedacted.value || undefined
    })
    editing.value = null
  }

  function sizeLabel(text: string): string {
    const n = text.length
    return n >= 10_000 ? `${(n / 1000).toFixed(0)}k 字符` : `${n} 字符`
  }

  const draftSensitive = computed(() => looksSensitive(draft.value))
</script>

<template>
  <div v-if="items.length" class="mx-auto mb-2 max-w-3xl flex flex-wrap items-center gap-1.5">
    <n-tooltip v-for="it in items" :key="it.id" placement="top">
      <template #trigger>
        <n-tag
          size="small"
          :type="mismatched(it) ? 'error' : looksSensitive(it.text) ? 'warning' : 'default'"
          closable
          class="cursor-pointer"
          @close="store.remove(it.id)"
          @click="openEdit(it)"
        >
          <i
            :class="
              it.kind === 'terminal'
                ? 'i-ri:terminal-line'
                : it.kind === 'file'
                  ? 'i-ri:file-text-line'
                  : 'i-ri:git-branch-line'
            "
            class="mr-1 inline-block size-3"
          />
          <span class="max-w-40 truncate align-middle">{{ it.title }}</span>
          <span class="ml-1 opacity-60">{{ sizeLabel(it.text) }}</span>
          <span v-if="it.redactedCount" class="ml-1 text-om-success">已脱敏 {{ it.redactedCount }}</span>
        </n-tag>
      </template>
      <div class="text-xs">
        <div>{{ kindLabel(it.kind) }} · {{ it.serverAlias || it.serverId }} · 会话 {{ it.sessionId.slice(0, 8) }}</div>
        <div v-if="mismatched(it)" class="text-om-danger">来自另一台服务器的分屏，不会随本对话发送</div>
        <div v-else-if="looksSensitive(it.text)" class="text-om-warning">疑似包含凭证，发送前请脱敏</div>
        <div class="opacity-70">点击查看／裁剪／脱敏</div>
      </div>
    </n-tooltip>
    <n-button v-if="items.length > 1" size="tiny" quaternary @click="store.clearTab(tabId)">清空</n-button>

    <n-modal
      :show="!!editing"
      preset="card"
      :title="editing ? `${kindLabel(editing.kind)} · ${editing.title}` : ''"
      style="width: min(860px, 92vw)"
      @update:show="(v: boolean) => !v && (editing = null)"
    >
      <template v-if="editing">
        <div class="mb-2 flex items-center gap-2 text-xs text-om-dimmed">
          <span>{{ editing.serverAlias || editing.serverId }} · 会话 {{ editing.sessionId.slice(0, 8) }}</span>
          <span>· {{ sizeLabel(draft) }}</span>
          <span v-if="draftSensitive" class="text-om-warning">疑似包含凭证</span>
          <span v-if="draftRedacted" class="text-om-success">本次脱敏 {{ draftRedacted }} 处</span>
          <span v-if="mismatched(editing)" class="text-om-danger">此条目绑定的服务器与当前对话不同，不会发送</span>
        </div>
        <n-input
          v-model:value="draft"
          type="textarea"
          :autosize="{ minRows: 12, maxRows: 28 }"
          class="text-xs font-mono"
          placeholder="删掉不需要发送的部分"
        />
        <div class="mt-2 text-xs text-om-dimmed">
          脱敏按常见形态匹配（bearer、password=、sk-、AKIA、URL
          凭证、私钥块）并与审计脱敏规则一致，不保证覆盖全部秘密；请自行检查。
        </div>
      </template>
      <template #footer>
        <div class="flex justify-end gap-2">
          <n-button size="small" @click="applyRedact">
            <template #icon><i class="i-ri:eye-off-line inline-block size-3.5" /></template>
            脱敏
          </n-button>
          <n-button
            size="small"
            type="error"
            quaternary
            @click="editing && (store.remove(editing.id), (editing = null))"
            >移除</n-button
          >
          <n-button size="small" @click="editing = null">取消</n-button>
          <n-button size="small" type="primary" @click="saveEdit">保存</n-button>
        </div>
      </template>
    </n-modal>
  </div>
</template>
