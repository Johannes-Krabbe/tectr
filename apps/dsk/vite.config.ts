import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

export default defineConfig({
  plugins: [react()],
  // Keep Rust build errors visible in the terminal Tauri shares with Vite.
  clearScreen: false,
  server: {
    // Tauri's devUrl expects exactly this port.
    port: 1420,
    strictPort: true,
    watch: { ignored: ["**/src-tauri/**"] },
  },
});
