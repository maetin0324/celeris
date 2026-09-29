import { defineConfig } from "vitest/config";

// 単体テスト。外部ネットワークに出ない。
export default defineConfig({
  test: {
    environment: "node",
    include: ["test/**/*.test.ts", "test/**/*.test.tsx", "lib/**/*.test.ts", "server/**/*.test.ts"],
    exclude: ["e2e/**", "node_modules/**", "dist/**"],
  },
});
