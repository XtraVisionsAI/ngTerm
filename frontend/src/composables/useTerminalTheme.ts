import { computed } from 'vue'
import { useAppTheme } from './useAppTheme'

export function useTerminalTheme() {
  const { currentThemeId, currentTheme, themeOptions, setTheme } = useAppTheme()

  const bgColor = computed(() => currentTheme.value.terminal.background || '#000')

  function handleThemeSelect(key: string) {
    setTheme(key)
  }

  return {
    currentThemeKey: currentThemeId,
    bgColor,
    themeOptions,
    handleThemeSelect
  }
}
