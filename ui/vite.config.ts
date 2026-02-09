import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'

export default defineConfig({
  plugins: [react(), tailwindcss()],
  server: {
    proxy: {
      '/health': 'http://localhost:8080',
      '/metrics': 'http://localhost:8080',
      '/v1': 'http://localhost:8080',
      '/admin': 'http://localhost:8080',
    },
  },
})
