import tailwindcss from "@tailwindcss/vite";
import vue from "@vitejs/plugin-vue";
import { defineConfig } from "vite";

// devUrl 与 crates/desktop/tauri.conf.json 对齐（strictPort 避免漂移）。
export default defineConfig({
  plugins: [vue(), tailwindcss()],
  clearScreen: false,
  server: {
    port: 5180,
    strictPort: true,
  },
  build: {
    target: "es2022",
    outDir: "dist",
  },
});
