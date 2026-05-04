import { svelte } from '@sveltejs/vite-plugin-svelte';
import { defineConfig } from 'vite';

export default defineConfig({
  plugins: [svelte()],
  server: {
    host: '0.0.0.0',
    hmr: {
      host: '0.0.0.0'
    },
    proxy: {
      '/api': {
        target: 'http://127.0.0.1:7777',
        ws: true
      },
      '/healthz': 'http://127.0.0.1:7777'
    },
    allowedHosts: true
  }
});
