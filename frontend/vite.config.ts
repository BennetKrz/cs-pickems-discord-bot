import react from '@vitejs/plugin-react'
import { defineConfig } from 'vitest/config'

const backend = 'http://localhost:8080'

// `pnpm dev` serves on localhost:5173 and proxies the backend.
// `pnpm dev:tunnel` (mode "tunnel") additionally makes HMR work when the page
// is loaded through a cloudflared quick tunnel inside the Discord client,
// which reaches the dev server over HTTPS on port 443.
export default defineConfig(({ mode }) => ({
  plugins: [react()],
  server: {
    port: 5173,
    strictPort: true,
    // Vite rejects unknown Host headers; allow cloudflared quick tunnels.
    allowedHosts: ['.trycloudflare.com'],
    proxy: {
      '/api': backend,
      '/healthz': backend,
    },
    ws: mode === 'tunnel' ? { clientPort: 443 } : undefined,
  },
  build: {
    outDir: 'dist',
    emptyOutDir: true,
  },
  test: {
    environment: 'jsdom',
    setupFiles: ['./src/test-setup.ts'],
    restoreMocks: true,
  },
}))
