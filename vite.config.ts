import { defineConfig } from "vite";

export default defineConfig({
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  test: { include: ["src/**/*.test.ts"] },
} as never);
