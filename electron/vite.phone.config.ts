import { defineConfig, mergeConfig } from 'vite';
import remote from './vite.remote.config';

// The native custom scheme loads one classic IIFE without a network module loader.
export default mergeConfig(remote, defineConfig({
  build: { outDir: 'dist-phone', rollupOptions: { output: { format: 'iife' } } },
}));
