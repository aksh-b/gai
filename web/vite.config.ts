import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

const flame = fileURLToPath(new URL('../../flame/src', import.meta.url));

export default defineConfig({
  plugins: [react()],
  resolve: {
    // flame is used from source; make its imports share this app's React.
    alias: { '@flame': flame },
    dedupe: ['react', 'react-dom'],
  },
  server: {
    port: 5190,
    fs: { allow: ['..', '../../flame'] },
    proxy: { '/v1': 'http://127.0.0.1:8484' },
  },
  build: { outDir: 'dist', emptyOutDir: true },
});
