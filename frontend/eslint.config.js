import defineConfig from '@xv-shared/eslint-config'

export default defineConfig().then((configs) => [
  ...configs,
  {
    rules: {
      'sort-imports': 'off',
      // v-model emits (update:modelValue etc.) cannot be kebab-cased
      'vue/custom-event-name-casing': ['error', 'kebab-case', { ignores: ['/^update:/'] }]
    }
  }
])
