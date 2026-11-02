import { fileURLToPath } from "node:url";
import { defineConfig } from "vitest/config";

export default defineConfig({
  resolve: {
    // celeris:host の実体は daemon の V8 の中だけにある。試験では差し替える。
    alias: { "celeris:host": fileURLToPath(new URL("src/testing/host-stub.ts", import.meta.url)) },
  },
  test: { include: ["src/**/*.test.ts"] },
});
