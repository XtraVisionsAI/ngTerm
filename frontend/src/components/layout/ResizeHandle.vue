<script setup lang="ts">
  import { ref } from 'vue'

  const props = withDefaults(
    defineProps<{
      direction?: 'horizontal' | 'vertical'
    }>(),
    { direction: 'horizontal' }
  )

  const emit = defineEmits<{
    resize: [delta: number]
  }>()

  const dragging = ref(false)

  function onMouseDown(e: MouseEvent) {
    e.preventDefault()
    dragging.value = true
    const startPos = props.direction === 'horizontal' ? e.clientX : e.clientY

    function onMouseMove(ev: MouseEvent) {
      const currentPos = props.direction === 'horizontal' ? ev.clientX : ev.clientY
      emit('resize', currentPos - startPos)
    }

    function onMouseUp() {
      dragging.value = false
      document.removeEventListener('mousemove', onMouseMove)
      document.removeEventListener('mouseup', onMouseUp)
      document.body.style.cursor = ''
      document.body.style.userSelect = ''
    }

    document.body.style.cursor = props.direction === 'horizontal' ? 'col-resize' : 'row-resize'
    document.body.style.userSelect = 'none'
    document.addEventListener('mousemove', onMouseMove)
    document.addEventListener('mouseup', onMouseUp)
  }
</script>

<template>
  <div class="resize-handle" :class="[direction, { active: dragging }]" @mousedown="onMouseDown" />
</template>

<style scoped>
  .resize-handle {
    flex-shrink: 0;
    background: transparent;
    transition: background 0.15s;
  }

  .resize-handle.horizontal {
    width: 4px;
    cursor: col-resize;
  }

  .resize-handle.vertical {
    height: 4px;
    cursor: row-resize;
  }

  .resize-handle:hover,
  .resize-handle.active {
    background: var(--om-primary);
    opacity: 0.5;
  }
</style>
