import './styles.css';
import { mount } from 'svelte';
import App from './App.svelte';

if (import.meta.env.PROD && 'serviceWorker' in navigator) {
  window.addEventListener('load', () => {
    void navigator.serviceWorker.register('/sw.js').catch((error) => {
      console.error('Service worker registration failed', error);
    });
  });
}

const app = mount(App, {
  target: document.getElementById('app') as HTMLElement
});

export default app;
