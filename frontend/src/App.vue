<script setup lang="ts">
  import type { GlobalThemeOverrides } from 'naive-ui'
  import { darkTheme, dateZhCN, NConfigProvider, NMessageProvider, zhCN } from 'naive-ui'
  import { computed } from 'vue'
  import { useAppTheme } from '@/composables/useAppTheme'

  const { currentTheme } = useAppTheme()

  const themeOverrides = computed<GlobalThemeOverrides>(() => {
    const c = currentTheme.value.colors
    return {
      common: {
        primaryColor: c.primary,
        primaryColorHover: c.primaryHover,
        primaryColorPressed: c.primaryHover,
        primaryColorSuppl: c.accent,
        bodyColor: c.bgPanel,
        cardColor: c.bg,
        modalColor: c.bg,
        popoverColor: c.bg,
        tableColor: c.bg,
        inputColor: c.bgInput,
        actionColor: c.bgInput,
        borderColor: c.border,
        dividerColor: c.border,
        hoverColor: c.bgHover,
        tableColorHover: c.bgHover,
        textColorBase: c.text,
        textColor1: c.text,
        textColor2: c.text,
        textColor3: c.textMuted,
        placeholderColor: c.textDimmed,
        borderRadius: '6px',
        borderRadiusSmall: '4px'
      },
      Button: {
        colorPrimary: c.primary,
        colorHoverPrimary: c.primaryHover,
        colorPressedPrimary: c.primaryHover,
        textColorPrimary: c.onPrimary
      },
      Menu: {
        itemTextColorActive: c.primary,
        itemIconColorActive: c.primary,
        itemColorActive: c.bgHover,
        itemTextColorHover: c.primary,
        itemIconColorHover: c.primary
      },
      Tag: {
        colorInfo: `${c.primary}1a`,
        textColorInfo: c.primary,
        borderInfo: `1px solid ${c.primary}4d`,
        colorWarning: `${c.warning}1a`,
        textColorWarning: c.warning,
        borderWarning: `1px solid ${c.warning}4d`,
        colorError: `${c.danger}1a`,
        textColorError: c.danger,
        borderError: `1px solid ${c.danger}4d`,
        colorSuccess: `${c.success}1a`,
        textColorSuccess: c.success,
        borderSuccess: `1px solid ${c.success}4d`
      },
      DataTable: {
        thColor: c.bg,
        tdColor: c.bg,
        thColorHover: c.bgHover,
        tdColorHover: c.bgHover,
        borderColor: c.border
      },
      Card: {
        borderColor: c.border
      },
      Input: {
        border: `1px solid ${c.border}`,
        borderHover: `1px solid ${c.primary}`,
        borderFocus: `1px solid ${c.primary}`
      },
      Tabs: {
        tabBorderColor: c.border,
        tabColor: c.bgPanel,
        tabColorSegment: c.bgHover
      }
    }
  })
</script>

<template>
  <n-config-provider :theme="darkTheme" :theme-overrides="themeOverrides" :locale="zhCN" :date-locale="dateZhCN">
    <n-message-provider>
      <router-view />
    </n-message-provider>
  </n-config-provider>
</template>
