import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()], base: "/", publicDir: "remote-public", logLevel: "warn",
  build: {
    assetsInlineLimit: 1024 * 1024,
    outDir: "dist-remote", chunkSizeWarningLimit: 1200,
    rollupOptions: {
      input: "remote.html",
      output: { entryFileNames: "remote.js", codeSplitting: false, assetFileNames: "remote[extname]" },
    },
  },
});
