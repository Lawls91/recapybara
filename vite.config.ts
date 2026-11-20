import { defineConfig } from "vite";

export default defineConfig({
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  test: { include: ["src/**/*.test.ts", "scripts/**/*.test.mjs", "e2e/*.test.mjs"] },
} as never);
