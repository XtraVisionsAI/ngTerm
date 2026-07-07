import type { AppTheme, AppThemeColors } from './terminalThemes'
import { computed, ref, watch } from 'vue'
import { appThemes, getAppTheme, getStoredThemeId, setStoredThemeId } from './terminalThemes'

function camelToKebab(str: string): string {
  return str.replace(/[A-Z]/g, (m) => `-${m.toLowerCase()}`)
}

function applyThemeToDOM(colors: AppThemeColors) {
  const root = document.documentElement
  for (const [key, value] of Object.entries(colors)) {
    root.style.setProperty(`--om-${camelToKebab(key)}`, value)
  }
}

const currentThemeId = ref(getStoredThemeId())

const currentTheme = computed<AppTheme>(() => getAppTheme(currentThemeId.value))

watch(
  currentTheme,
  (theme) => {
    applyThemeToDOM(theme.colors)
  },
  { immediate: true }
)

export function useAppTheme() {
  const themeOptions = computed(() => appThemes.map((t) => ({ label: t.name, key: t.id })))

  function setTheme(id: string) {
    currentThemeId.value = id
    setStoredThemeId(id)
  }

  return {
    currentThemeId,
    currentTheme,
    themeOptions,
    setTheme
  }
}
