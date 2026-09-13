<script setup lang="ts">
  /** Progress and evidence of one flow run: pre-checks, steps, verification, error. */
  import type { CommandResult, Run, StepResult } from '@/utils/flows'
  import { NTag } from 'naive-ui'
  import { runStatusInfo } from '@/utils/flows'
  import { formatTime } from '@/utils/format'

  defineProps<{ run: Run }>()

  function stepTag(s: StepResult): { label: string; type: 'success' | 'error' } {
    if (!s.result.passed) return { label: '失败', type: 'error' }
    if (s.verify && !s.verify.passed) return { label: '验证未通过', type: 'error' }
    return { label: s.verify ? '完成并已验证' : '完成', type: 'success' }
  }

  function exitLabel(r: CommandResult): string {
    if (r.exitCode === null) return r.message || '无结果'
    return `exit ${r.exitCode} · ${r.durationMs} ms${r.truncated ? ' · 输出已截断' : ''}`
  }
</script>

<template>
  <div class="border border-om-border rounded p-3 text-xs">
    <div class="mb-2 flex flex-wrap items-center gap-2">
      <n-tag size="small" :type="runStatusInfo[run.status].type">{{ runStatusInfo[run.status].label }}</n-tag>
      <span class="text-om-dimmed"
        >{{ run.serverAlias || run.serverId || '本地' }} · 会话 {{ run.sessionId.slice(0, 8) }} ·
        {{ formatTime(run.startedAt) }}</span
      >
      <span v-if="Object.keys(run.params).length" class="text-om-dimmed font-mono">
        {{
          Object.entries(run.params)
            .map(([k, v]) => `${k}=${v}`)
            .join(' ')
        }}
      </span>
    </div>
    <div v-if="run.error" class="mb-2 text-om-danger">{{ run.error }}</div>

    <div v-if="run.preChecks.length" class="mb-2">
      <div class="mb-1 text-om-dimmed">前置检查</div>
      <div
        v-for="(c, i) in run.preChecks"
        :key="i"
        class="mb-1 border-l-2 pl-2"
        :class="c.passed ? 'border-om-success' : 'border-om-danger'"
      >
        <div class="flex items-center gap-2">
          <n-tag size="tiny" :type="c.passed ? 'success' : 'error'" :bordered="false">{{
            c.passed ? '通过' : '未通过'
          }}</n-tag>
          <span>{{ c.name }}</span>
          <code class="text-om-dimmed">{{ c.command }}</code>
          <span class="text-om-dimmed">{{ exitLabel(c) }}</span>
        </div>
        <div v-if="c.message" class="text-om-danger">{{ c.message }}</div>
        <pre
          v-if="c.output"
          class="mt-0.5 max-h-32 overflow-auto whitespace-pre-wrap break-all rounded bg-om-bg p-1 font-mono"
          >{{ c.output }}</pre>
      </div>
    </div>

    <div class="mb-1 text-om-dimmed">步骤 {{ run.steps.length }}/{{ run.definition.steps.length }}</div>
    <div
      v-for="s in run.steps"
      :key="s.index"
      class="mb-2 border-l-2 pl-2"
      :class="stepTag(s).type === 'success' ? 'border-om-success' : 'border-om-danger'"
    >
      <div class="flex flex-wrap items-center gap-2">
        <span class="font-semibold">{{ s.index + 1 }}. {{ s.name }}</span>
        <n-tag size="tiny" :type="stepTag(s).type" :bordered="false">{{ stepTag(s).label }}</n-tag>
        <n-tag v-if="s.approval === 'required'" size="tiny" type="warning" :bordered="false">已经第二人审批</n-tag>
        <span class="text-om-dimmed">{{ exitLabel(s.result) }}</span>
        <router-link
          v-if="s.result.operationId"
          :to="{ path: '/audit', query: { operationId: s.result.operationId } }"
          class="text-om-primary hover:underline"
        >
          审计
        </router-link>
      </div>
      <code class="text-om-dimmed">{{ s.result.command }}</code>
      <div v-if="s.result.message && !s.result.passed" class="text-om-danger">{{ s.result.message }}</div>
      <pre
        v-if="s.result.output"
        class="mt-0.5 max-h-48 overflow-auto whitespace-pre-wrap break-all rounded bg-om-bg p-1 font-mono"
        >{{ s.result.output }}</pre>
      <div
        v-if="s.verify"
        class="mt-1 border-l-2 pl-2"
        :class="s.verify.passed ? 'border-om-success' : 'border-om-danger'"
      >
        <div class="flex items-center gap-2">
          <n-tag size="tiny" :type="s.verify.passed ? 'success' : 'error'" :bordered="false"
            >验证{{ s.verify.passed ? '通过' : '未通过' }}</n-tag
          >
          <span>{{ s.verify.name }}</span>
          <code class="text-om-dimmed">{{ s.verify.command }}</code>
          <span class="text-om-dimmed">{{ exitLabel(s.verify) }}</span>
        </div>
        <div v-if="s.verify.message && !s.verify.passed" class="text-om-danger">{{ s.verify.message }}</div>
        <pre
          v-if="s.verify.output"
          class="mt-0.5 max-h-32 overflow-auto whitespace-pre-wrap break-all rounded bg-om-bg p-1 font-mono"
          >{{ s.verify.output }}</pre>
      </div>
    </div>
    <div v-if="run.status === 'ready' && run.definition.steps[run.currentStep]" class="text-om-dimmed">
      下一步：{{ run.definition.steps[run.currentStep].name }}
      <code>{{ run.definition.steps[run.currentStep].command }}</code>
    </div>
    <div class="mt-2">
      <slot name="actions" />
    </div>
  </div>
</template>
