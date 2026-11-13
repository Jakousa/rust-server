import { defineConfig } from 'vite';

export default defineConfig({
  server: {
    port: 3000,
  },
  build: {
    target: 'esnext',
  },
  test: {
    include: ['src/**/*.test.js'],
  },
});
