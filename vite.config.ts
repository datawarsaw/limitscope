import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

// https://vitejs.dev/config/
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  test: {
    // Only the tracked source roots are canonical; repository-local scratch
    // and worktree copies (e.g. .wip-*, .review-*, .worktrees) must never be
    // collected. Keep include narrow by location instead of excluding
    // scratch directories one by one.
    include: [
      "src/**/*.{test,spec}.{ts,tsx,js,jsx,mjs,cjs,mts,cts}",
      "scripts/**/*.{test,spec}.{ts,tsx,js,jsx,mjs,cjs,mts,cts}",
    ],
  },
  server: {
    port: 1420,
    strictPort: true,
    // cargo writes/locks files under src-tauri/target while `tauri dev` runs;
    // watching them crashes chokidar with EBUSY on Windows.
    watch: {
      ignored: ["**/src-tauri/**"],
    },
  },
  envPrefix: ["VITE_", "TAURI_ENV_"],
  build: {
    target: "chrome105",
    minify: !process.env.TAURI_ENV_DEBUG ? "esbuild" : false,
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
  },
});
