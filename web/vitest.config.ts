import { defineConfig } from "vitest/config";

// 単体テスト。外部ネットワークに出ない。
export default defineConfig({
  test: {
    environment: "node",
    include: [
      "test/**/*.test.ts",
      "test/**/*.test.tsx",
      "lib/**/*.test.ts",
      "api/**/*.test.ts",
      "components/**/*.test.ts",
      "components/**/*.test.tsx",
      "server/**/*.test.ts",
      "e2e/support/**/*.test.ts",
    ],
    exclude: ["node_modules/**", "dist/**"],
  },
});
