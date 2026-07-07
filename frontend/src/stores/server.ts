import { defineStore } from 'pinia'
import { ref } from 'vue'

export interface Server {
  id: string
  groupName: string
  alias: string
  host: string
  port: number
  username: string
  keyId: string | null
  tags: string[]
  aiToolId: string | null
  createdAt: string
}

export interface KeyInfo {
  id: string
  name: string
  fingerprint: string
  keyType: string
  createdAt: string
}

export const useServerStore = defineStore('server', () => {
  const servers = ref<Server[]>([])
  const keys = ref<KeyInfo[]>([])
  const groups = ref<string[]>([])
  const aiToolOptions = ref<{ label: string; value: string }[]>([])

  function setServers(list: Server[]) {
    servers.value = list
  }

  function setKeys(list: KeyInfo[]) {
    keys.value = list
  }

  function setGroups(list: string[]) {
    groups.value = list
  }

  function setAiToolOptions(list: { label: string; value: string }[]) {
    aiToolOptions.value = list
  }

  return { servers, keys, groups, aiToolOptions, setServers, setKeys, setGroups, setAiToolOptions }
})
