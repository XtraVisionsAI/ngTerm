import defineConfig from '@xv-shared/eslint-config'

export default defineConfig().then((configs) => [
  ...configs,
  {
    rules: {
      'sort-imports': 'off'
    }
  }
])
