import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  // Tauri prints Rust errors in the same terminal; don't let Vite wipe them.
  clearScreen: false,
  server: {
    // Must match devUrl in src-tauri/tauri.conf.json, so fail instead of
    // silently moving to another port.
    port: 1420,
    strictPort: true,
    host: "127.0.0.1",
  },
});
