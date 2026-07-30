import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";

// Tauri dev expects a fixed port and clearscreen disabled
export default defineConfig({
  plugins: [vue()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: { ignored: ["**/src-tauri/**"] },
  },
});
