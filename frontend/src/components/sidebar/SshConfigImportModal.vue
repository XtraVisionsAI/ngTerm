<script setup lang="ts">
  /**
   * Import servers from a pasted OpenSSH client config. The server parses a
   * declared subset and returns a preview; blocks it cannot represent (jump
   * hosts, forwards, wildcards, Match/Include) are listed with the reason and
   * never approximated. The user confirms before anything is created.
   */
  import { NButton, NCheckbox, NInput, NModal, NSelect, NSpace, NTag, useMessage } from 'naive-ui'
  import { computed, ref, watch } from 'vue'
  import { useApi } from '@/composables/useApi'
  import { useServerStore } from '@/stores/server'

  const props = defineProps<{ show: boolean }>()
  const emit = defineEmits<{ 'update:show': [v: boolean]; saved: [] }>()

  const api = useApi()
  const message = useMessage()
  const serverStore = useServerStore()

  interface Candidate {
    alias: string
    host: string
    port: number
    username: string | null
    identityFile: string | null
    ignored: string[]
    keyId: string | null
    keyMatchedByName: boolean
    exists: boolean
  }
  interface Preview {
    candidates: Candidate[]
    skipped: { pattern: string; reason: string }[]
    created: unknown[]
    errors: { alias: string; error: string }[]
  }

  const text = ref('')
  const groupName = ref<string | null>(null)
  const defaultKeyId = ref<string | null>(null)
  const preview = ref<Preview | null>(null)
  const busy = ref(false)
  const showSkipped = ref(true)

  const groupOptions = computed(() => serverStore.groups.map((g) => ({ label: g, value: g })))
  const keyOptions = computed(() => serverStore.keys.map((k) => ({ label: k.name, value: k.id })))
  const importable = computed(() => preview.value?.candidates.filter((c) => !c.exists && c.username) ?? [])
  const missingUser = computed(() => preview.value?.candidates.filter((c) => !c.exists && !c.username) ?? [])

  watch(
    () => props.show,
    (v) => {
      if (v) {
        text.value = ''
        preview.value = null
      }
    }
  )
  watch([groupName, defaultKeyId], () => {
    if (preview.value) doPreview()
  })

  async function doPreview() {
    if (!text.value.trim()) return
    busy.value = true
    try {
      preview.value = await api.post<Preview>('/servers/import-ssh-config', {
        text: text.value,
        dryRun: true,
        groupName: groupName.value || undefined,
        defaultKeyId: defaultKeyId.value || undefined
      })
    } catch (e) {
      message.error(`解析失败: ${(e as Error).message}`)
    } finally {
      busy.value = false
    }
  }

  async function doImport() {
    busy.value = true
    try {
      const r = await api.post<Preview>('/servers/import-ssh-config', {
        text: text.value,
        dryRun: false,
        groupName: groupName.value || undefined,
        defaultKeyId: defaultKeyId.value || undefined
      })
      const n = r.created.length
      if (r.errors.length) {
        message.warning(`已导入 ${n} 个，${r.errors.length} 个失败：${r.errors.map((e) => e.alias).join(', ')}`)
      } else {
        message.success(`已导入 ${n} 个服务器（标签 import:ssh-config）`)
      }
      emit('saved')
      emit('update:show', false)
    } catch (e) {
      message.error(`导入失败: ${(e as Error).message}`)
    } finally {
      busy.value = false
    }
  }
</script>

<template>
  <n-modal
    :show="show"
    title="从 SSH config 导入"
    preset="card"
    style="width: min(860px, 94vw)"
    @update:show="emit('update:show', $event)"
  >
    <div class="mb-2 text-xs text-om-dimmed">
      粘贴 ~/.ssh/config
      内容。支持的子集：Host（单个具体别名）、HostName、User、Port、IdentityFile（按文件名匹配已保存的密钥）。 ProxyJump
      / 端口转发 / 证书 / 通配符 / Match / Include 会被列为跳过并说明原因，不会被近似导入。
    </div>
    <n-input
      v-model:value="text"
      type="textarea"
      :autosize="{ minRows: 6, maxRows: 14 }"
      class="text-xs font-mono"
      placeholder="Host web-1&#10;  HostName 10.0.0.1&#10;  User deploy&#10;  Port 22&#10;  IdentityFile ~/.ssh/id_deploy"
    />
    <div class="mt-2 flex flex-wrap items-center gap-2">
      <n-select
        v-model:value="groupName"
        :options="groupOptions"
        placeholder="导入到分组（可选）"
        filterable
        tag
        clearable
        size="small"
        style="width: 200px"
      />
      <n-select
        v-model:value="defaultKeyId"
        :options="keyOptions"
        placeholder="未匹配到密钥时使用"
        clearable
        size="small"
        style="width: 220px"
      />
      <n-button size="small" :loading="busy" :disabled="!text.trim()" @click="doPreview">预览</n-button>
    </div>

    <template v-if="preview">
      <div class="mb-1 mt-3 text-xs text-om-dimmed">
        可导入 {{ importable.length }} 个 · 已存在 {{ preview.candidates.filter((c) => c.exists).length }} 个 · 跳过
        {{ preview.skipped.length }} 个
        <span v-if="missingUser.length" class="text-om-warning">· {{ missingUser.length }} 个缺少 User，不会导入</span>
      </div>
      <div class="max-h-60 overflow-auto border border-om-border rounded">
        <table class="w-full text-xs">
          <thead class="text-om-dimmed">
            <tr>
              <th class="px-2 py-1 text-left">别名</th>
              <th class="px-2 py-1 text-left">地址</th>
              <th class="px-2 py-1 text-left">密钥</th>
              <th class="px-2 py-1 text-left">状态</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="c in preview.candidates" :key="c.alias" class="border-t border-om-border">
              <td class="px-2 py-1 font-mono">{{ c.alias }}</td>
              <td class="px-2 py-1 font-mono">{{ c.username || '?' }}@{{ c.host }}:{{ c.port }}</td>
              <td class="px-2 py-1">
                <template v-if="c.keyId">
                  {{ serverStore.keys.find((k) => k.id === c.keyId)?.name || c.keyId }}
                  <span class="text-om-dimmed">{{ c.keyMatchedByName ? '（按文件名匹配）' : '（默认）' }}</span>
                </template>
                <span v-else class="text-om-dimmed">{{ c.identityFile ? `未匹配 ${c.identityFile}` : '无' }}</span>
              </td>
              <td class="px-2 py-1">
                <n-tag v-if="c.exists" size="tiny">已存在</n-tag>
                <n-tag v-else-if="!c.username" size="tiny" type="warning">缺少 User</n-tag>
                <n-tag v-else size="tiny" type="success">将导入</n-tag>
              </td>
            </tr>
          </tbody>
        </table>
      </div>
      <div v-if="preview.skipped.length" class="mt-2">
        <n-checkbox v-model:checked="showSkipped" size="small">显示跳过的块（{{ preview.skipped.length }}）</n-checkbox>
        <div v-if="showSkipped" class="mt-1 max-h-32 overflow-auto text-xs">
          <div v-for="s in preview.skipped" :key="s.pattern" class="flex gap-2">
            <span class="shrink-0 font-mono">{{ s.pattern }}</span>
            <span class="text-om-dimmed">{{ s.reason }}</span>
          </div>
        </div>
      </div>
    </template>

    <template #footer>
      <n-space justify="end">
        <n-button size="small" @click="emit('update:show', false)">取消</n-button>
        <n-button size="small" type="primary" :disabled="importable.length === 0" :loading="busy" @click="doImport">
          导入 {{ importable.length }} 个
        </n-button>
      </n-space>
    </template>
  </n-modal>
</template>
