import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'

export default defineConfig({
  plugins: [react(), tailwindcss()],
  server: {
    proxy: {
      '/health': 'http://127.0.0.1:8080',
      '/metrics': 'http://127.0.0.1:8080',
      '/v1': 'http://127.0.0.1:8080',
      '/admin': 'http://127.0.0.1:8080',
    },
  },
  build: {
    rollupOptions: {
      output: {
        // Only the chunks that pay their keep are named explicitly.
        // Everything else falls into Vite's default splitting, which
        // puts eager-shared deps in the main entry and lazy-only
        // deps in their route chunks. That keeps this config short
        // and stops new dependencies from silently bloating the
        // initial load through a catch-all `vendor` chunk.
        manualChunks: (id) => {
          if (!id.includes('node_modules')) return undefined

          // React core + router. Every route needs it; making it a
          // named chunk gives it a stable cache key across deploys.
          if (
            /[\\/]node_modules[\\/](react|react-dom|react-router|react-router-dom|scheduler)[\\/]/.test(
              id,
            )
          ) {
            return 'react-vendor'
          }

          // Recharts is the single largest dependency and is used by
          // Dashboard (eager) AND Analytics (lazy). Splitting it into
          // its own chunk means visiting Analytics after Dashboard
          // reuses the cached recharts bundle instead of re-downloading
          // it inside the Analytics route chunk. d3-* is included
          // because recharts depends on several d3 sub-packages.
          if (id.includes('node_modules/recharts') || id.includes('node_modules/d3-')) {
            return 'recharts'
          }

          // @xyflow/react — used only by the chain DAG visualizer.
          // Lazy via Chains/ChainDetail/ChainDefinitions, but a named
          // chunk so the three pages that need it share a single
          // cacheable file (and so `npm run build` surfaces it
          // readably instead of as `chunk-Dxxx.js`).
          if (id.includes('node_modules/@xyflow')) return 'xyflow'

          // framer-motion drives the page-transition animation in
          // AppShell (eager). Splitting it out keeps the ~40 KB gz
          // library cached across app-code deploys so repeat
          // visitors don't re-download it just because the main
          // entry hash changed.
          if (id.includes('node_modules/framer-motion') || id.includes('node_modules/motion-')) {
            return 'framer-motion'
          }

          // Everything else → Vite's default. No catch-all vendor
          // chunk: adding a new dep to a lazy page won't suddenly
          // bloat the eager load.
          return undefined
        },
      },
    },
  },
})
