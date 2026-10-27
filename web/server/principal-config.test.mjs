import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { test } from "node:test";

test("gateway rejects the removed shared-token setting before listening", () => {
  const legacyPath = "/private/operator-token-must-not-be-read";
  const result = spawnSync(process.execPath, [fileURLToPath(new URL("./index.js", import.meta.url))], {
    env: {
      ...process.env,
      CELERIS_API_TOKEN_FILE: legacyPath,
      CELERIS_WEB_API_TOKEN_FILE: "/private/web-token",
    },
    encoding: "utf8",
    timeout: 10_000,
  });
  assert.equal(result.status, 2);
  assert.match(result.stderr, /CELERIS_API_TOKEN_FILE was removed/);
  assert.doesNotMatch(result.stderr, /listening on/);
  assert.equal(result.stderr.includes(legacyPath), false);
});
