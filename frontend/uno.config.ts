import transformerDirectives from '@unocss/transformer-directives'
import transformerVariantGroup from '@unocss/transformer-variant-group'
import { defineConfig, presetIcons, presetUno } from 'unocss'

export default defineConfig({
  presets: [
    presetUno(),
    presetIcons({
      scale: 1.2,
      warn: true
    })
  ],
  transformers: [transformerDirectives(), transformerVariantGroup()],
  theme: {
    colors: {
      om: {
        bg: 'var(--om-bg)',
        panel: 'var(--om-bg-panel)',
        hover: 'var(--om-bg-hover)',
        input: 'var(--om-bg-input)',
        border: 'var(--om-border)',
        text: 'var(--om-text)',
        muted: 'var(--om-text-muted)',
        dimmed: 'var(--om-text-dimmed)',
        primary: 'var(--om-primary)',
        'primary-hover': 'var(--om-primary-hover)',
        danger: 'var(--om-danger)',
        success: 'var(--om-success)',
        warning: 'var(--om-warning)',
        accent: 'var(--om-accent)'
      }
    }
  }
})
