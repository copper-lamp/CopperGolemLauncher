import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";
// @ts-expect-error type error without @types/node package
import process from "node:process";
const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(() => ({
  plugins: [
    vue(),
    {
      name: "copper-addon-shared-dev-runtime",
      apply: "serve",
      transformIndexHtml: {
        order: "pre",
        handler(html) {
          // A public/vendor copy is a second Vue instance in Vite development.
          // These entry re-exports use the host's exact optimized module graph,
          // including vue-router injection symbols. Production vendor assets
          // and its import map are deliberately not rewritten by this plugin.
          return html
            .replace('"vue": "/vendor/vue.esm-browser.prod.js"', '"vue": "/src/modules/sharedVue.ts"')
            .replace('"vue-router": "/vendor/vue-router.esm-browser.js"', '"vue-router": "/src/modules/sharedRouter.ts"');
        },
      },
    },
    {
      name: "copper-addon-single-production-runtime",
      apply: "build",
      generateBundle(_options, bundle) {
        for (const output of Object.values(bundle)) {
          if (output.type !== "chunk") continue;
          for (const id of Object.keys(output.modules)) {
            if (/[/\\]node_modules[/\\](?:vue|vue-router|@vue[/\\](?:runtime-core|runtime-dom|reactivity|shared))[/\\]/.test(id)) {
              this.error(`Host must not inline addon-shared Vue/router runtime: ${id}`);
            }
          }
        }
      },
    },
  ],

  // Release host and addon ESM must resolve these exact bare specifiers through
  // the same index.html import map; bundling either creates a second runtime.
  build: {
    rolldownOptions: {
      external: ["vue", "vue-router"],
    },
  },

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. tell Vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
  },
}));
