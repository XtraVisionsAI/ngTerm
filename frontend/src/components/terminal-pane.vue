<script setup lang="ts">
  import type { Ref } from 'vue'
  import type { PaneNode } from '@/stores/session'
  import { computed, inject, ref } from 'vue'
  import ResizeHandle from '@/components/layout/ResizeHandle.vue'
  import TerminalView from '@/components/terminal-view.vue'

  const props = defineProps<{
    node: PaneNode
    activePaneId?: string
    themeKey?: string
  }>()

  const emit = defineEmits<{
    split: [paneId: string, direction: 'horizontal' | 'vertical']
    close: [paneId: string]
    focus: [paneId: string]
    'ratio-change': [splitId: string, ratio: number]
  }>()

  const terminalRefs = inject<Ref<Record<string, InstanceType<typeof TerminalView>>>>('terminalRefs')
  const createPaneSession =
    inject<(serverId: string, pendingId: string, cols: number, rows: number) => Promise<{ id: string }>>(
      'createPaneSession'
    )
  const termViewRef = ref<InstanceType<typeof TerminalView>>()

  function registerRef(el: any) {
    if (el && props.node.type === 'terminal' && terminalRefs) {
      terminalRefs.value[props.node.id] = el
      termViewRef.value = el
    }
  }

  function getCreateSessionFn() {
    if (props.node.type !== 'terminal') return undefined
    if (!props.node.sessionId.startsWith('pending-')) return undefined
    if (!createPaneSession) return undefined
    const serverId = props.node.serverId
    const pendingId = props.node.sessionId
    return (cols: number, rows: number) => createPaneSession(serverId, pendingId, cols, rows)
  }

  const firstFlex = computed(() => {
    if (props.node.type !== 'split') return '1 1 0'
    return `${props.node.ratio} 1 0`
  })

  const secondFlex = computed(() => {
    if (props.node.type !== 'split') return '0 1 0'
    return `${1 - props.node.ratio} 1 0`
  })

  function handleResize(delta: number) {
    if (props.node.type !== 'split') return
    const container = document.getElementById(`split-${props.node.id}`)
    if (!container) return
    const rect = container.getBoundingClientRect()
    const totalSize = props.node.direction === 'horizontal' ? rect.width : rect.height
    const newRatio = Math.max(0.1, Math.min(0.9, props.node.ratio + delta / totalSize))
    emit('ratio-change', props.node.id, newRatio)
  }
</script>

<template>
  <div v-if="node.type === 'terminal'" class="h-full w-full" @mousedown="emit('focus', node.id)">
    <terminal-view
      :ref="registerRef"
      :session-id="node.sessionId"
      :server-id="node.serverId"
      :active="true"
      :theme-key="themeKey"
      :create-session-fn="getCreateSessionFn()"
      @split="(dir) => emit('split', node.id, dir)"
      @close="emit('close', node.id)"
    />
  </div>
  <div
    v-else
    :id="`split-${node.id}`"
    class="h-full w-full flex"
    :class="node.direction === 'vertical' ? 'flex-col' : ''"
  >
    <div :style="{ flex: firstFlex, minWidth: 0, minHeight: 0 }" class="h-full overflow-hidden">
      <terminal-pane
        :node="node.children[0]"
        :active-pane-id="activePaneId"
        :theme-key="themeKey"
        @split="(id, dir) => emit('split', id, dir)"
        @close="(id) => emit('close', id)"
        @focus="(id) => emit('focus', id)"
        @ratio-change="(id, r) => emit('ratio-change', id, r)"
      />
    </div>
    <resize-handle :direction="node.direction" @resize="handleResize" />
    <div :style="{ flex: secondFlex, minWidth: 0, minHeight: 0 }" class="h-full overflow-hidden">
      <terminal-pane
        :node="node.children[1]"
        :active-pane-id="activePaneId"
        :theme-key="themeKey"
        @split="(id, dir) => emit('split', id, dir)"
        @close="(id) => emit('close', id)"
        @focus="(id) => emit('focus', id)"
        @ratio-change="(id, r) => emit('ratio-change', id, r)"
      />
    </div>
  </div>
</template>
