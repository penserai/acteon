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
    // Bumped from the default 500 KB to 600 KB so the warning fires
    // only on truly egregious chunks. After manualChunks splitting
    // the largest vendor chunk (recharts) is well under the new cap.
    chunkSizeWarningLimit: 600,
    rollupOptions: {
      output: {
        // Vendor chunking: pull the heaviest libraries into their own
        // chunks so they cache independently and don't bloat the
        // initial page load. Anything not matched here falls into
        // route-level chunks created by React.lazy() in App.tsx.
        manualChunks: (id) => {
          if (!id.includes('node_modules')) return undefined

          // React core + router. Eager (every page needs them).
          if (
            /[\\/]node_modules[\\/](react|react-dom|react-router|react-router-dom|scheduler)[\\/]/.test(
              id,
            )
          ) {
            return 'react-vendor'
          }

          // Recharts is used by Dashboard (eager) and Analytics (lazy).
          // Keeping it in its own chunk means Analytics rides on the
          // already-cached chunk instead of re-downloading it.
          if (id.includes('node_modules/recharts') || id.includes('node_modules/d3-')) {
            return 'recharts'
          }

          // @xyflow/react is only used by the chain DAG visualizer.
          // Lazy via Chains/ChainDetail/ChainDefinitions, but split
          // out so the three pages share one cached chunk.
          if (id.includes('node_modules/@xyflow')) return 'xyflow'

          // CodeMirror — large editor stack pinned in package.json
          // even though src doesn't import it today; gate it so any
          // future use (rule editor, template editor) lands in its
          // own chunk instead of the eager bundle.
          if (id.includes('node_modules/@codemirror') || id.includes('node_modules/codemirror')) {
            return 'codemirror'
          }

          // framer-motion drives the page-transition animation in
          // AppShell. Eager but cacheable.
          if (id.includes('node_modules/framer-motion') || id.includes('node_modules/motion-')) {
            return 'framer-motion'
          }

          // TanStack query + table — used across many pages, so a
          // shared chunk avoids duplicating the runtime per route.
          if (id.includes('node_modules/@tanstack')) return 'tanstack'

          // cmdk powers the command palette (always mounted).
          if (id.includes('node_modules/cmdk')) return 'cmdk'

          // lucide-react ships an enormous icon catalog; tree
          // shaking already trims it but the rest still benefits
          // from a dedicated chunk so it caches separately.
          if (id.includes('node_modules/lucide-react')) return 'icons'

          // Everything else in node_modules → a single shared vendor
          // chunk so we don't fragment small utilities across every
          // page chunk.
          return 'vendor'
        },
      },
    },
  },
})
