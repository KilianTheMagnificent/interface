import { readFileSync } from "node:fs";
import path from "node:path";
import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";
import type { Plugin } from "vite";

// A build without the log would ship an empty Settings -> Updates, so only the
// dev server and the tests may go without it.
function readUpdateLogMarkdown(required: boolean): string {
  try {
    return readFileSync(path.resolve(import.meta.dirname, "..", "docs", "UPDATES.md"), "utf8");
  } catch (error) {
    if (required) throw new Error("docs/UPDATES.md is missing: the build embeds the update log", { cause: error });
    return "";
  }
}

// `scripts/dev.mjs` gives each Vite process a unique mode and accepts the
// server only when this endpoint echoes that exact value. A stale process on
// the fixed port can serve the same index.html, so an ordinary HTTP 200 is not
// enough to prove Electron is about to connect to the process just spawned.
function readinessPlugin(runId: string): Plugin {
  return {
    name: "interface-dev-readiness",
    configureServer(server) {
      server.middlewares.use("/__interface_dev_ready__", (request, response) => {
        if (request.method !== "GET" && request.method !== "HEAD") {
          response.statusCode = 405;
          response.end();
          return;
        }
        response.statusCode = 200;
        response.setHeader("Cache-Control", "no-store");
        response.setHeader("Content-Type", "text/plain; charset=utf-8");
        response.end(request.method === "HEAD" ? undefined : runId);
      });
    },
  };
}

// `file:` loads in a packaged app never pass through `webRequest.onHeadersReceived`,
// so the built `dist/index.html` must carry the production CSP in its own meta tag
// (matching `contentSecurityPolicy(undefined)` in `src/main/window-security.ts`).
export function productionCspHtml(html: string): string {
  let tightened = "";
  const next = html.replace(
    /(<meta\s+http-equiv="Content-Security-Policy"\s+content=")([^"]+)(")/u,
    (_match, prefix: string, content: string, suffix: string) => {
      tightened = content
        .replace(/\bscript-src 'self' 'unsafe-inline'/u, "script-src 'self'")
        .replace(/\bconnect-src 'self' ws:\/\/localhost:\* ws:\/\/127\.0\.0\.1:\*/u, "connect-src 'self'");
      return `${prefix}${tightened}${suffix}`;
    },
  );
  if (tightened === "" || /script-src[^;]*'unsafe-inline'/u.test(tightened)
      || /connect-src[^;]*\bws:\/\//u.test(tightened)) {
    throw new Error("vite build could not tighten the Content-Security-Policy meta tag in index.html");
  }
  return next;
}

function productionCspPlugin(): Plugin {
  return {
    name: "interface-production-csp",
    transformIndexHtml: {
      order: "post",
      handler(html) {
        return productionCspHtml(html);
      },
    },
  };
}

export default defineConfig(({ command, mode }) => ({
  plugins: [
    react(),
    readinessPlugin(mode),
    ...(command === "build" ? [productionCspPlugin()] : []),
  ],
  base: "./",
  define: {
    __INTERFACE_UPDATE_LOG_MD__: JSON.stringify(readUpdateLogMarkdown(command === "build")),
  },
  // The gate's log is read when something failed, so the build contributes its
  // warnings and errors and nothing else. The renderer's main chunk is one
  // deliberate bundle for an Electron window that loads it off local disk, so
  // the 500 kB default warning fires on every single build and means nothing;
  // the limit sits just above today's main chunk (2,122 kB, with the update log
  // and the PDF viewer in their own lazy chunks) so real growth still speaks up.
  logLevel: "warn",
  build: { outDir: "dist", chunkSizeWarningLimit: 2_150 },
  test: {
    include: ["src/**/*.test.{ts,tsx}"],
    environment: "node",
    pool: "threads",
    reporters: ["dot"],
  },
}));
