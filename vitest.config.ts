import react from "@vitejs/plugin-react";
import {defineConfig} from "vitest/config";

export default defineConfig({
  plugins: [react()],
  test: {
    environment: "node",
    globals: true,
    clearMocks: true,
    pool: "forks",
    maxWorkers: 1,
    fileParallelism: false,
    exclude: ["**/node_modules/**", "**/.tools/**", "**/.runtime/**", "**/dist/**"],
    coverage: {
      provider: "istanbul",
      include: [
        "packages/replay-contract/src/indexed-library.ts",
        "packages/replay-contract/src/jetbrains-status.ts",
        "packages/replay-engine/src/**/*.ts",
      ],
      reporter: ["text", "json-summary"],
      thresholds: {
        branches: 90,
        perFile: true,
      },
    },
  },
});
