import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";

// Tauri 前端配置：固定端口 1420，构建产物给 Tauri 壳打包
export default defineConfig({
  plugins: [vue()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
  },
  build: {
    target: "es2021",
  },
});
