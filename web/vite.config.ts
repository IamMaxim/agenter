import { svelte } from '@sveltejs/vite-plugin-svelte';
import { defineConfig } from 'vite';

export default defineConfig({
  plugins: [svelte()],
  server: {
    proxy: {
      '/api': {
        target: 'http://127.0.0.1:7777',
        ws: true
      },
      '/healthz': 'http://127.0.0.1:7777'
    }
  }
});
