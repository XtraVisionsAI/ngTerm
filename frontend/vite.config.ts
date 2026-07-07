import { fileURLToPath, URL } from 'node:url'

import { createVitePlugins } from '@xv-shared/vite'
import { defineConfig } from 'vite'

export default defineConfig(() => {
  return {
    plugins: createVitePlugins({
      autoRouter: {
        dts: 'types/generated/typed-router.d.ts'
      },
      autoLayout: true,
      html: {
        minify: true,
        entry: '/src/main.ts',
        template: 'index.html'
      }
    }),
    resolve: {
      alias: {
        '@': fileURLToPath(new URL('./src', import.meta.url))
      }
    },
    server: {
      port: 3000,
      proxy: {
        '/api': {
          target: 'http://localhost:8080',
          changeOrigin: true,
          timeout: 300000
        },
        '/ws': {
          target: 'ws://localhost:8080',
          ws: true
        }
      }
    }
  }
})
