---
title: 葉 web-e2e-tmp — web e2e の一時 dir を共通 helper で作り終了時に消す
tasks: [01M4B4J92KBR73EQA5S7FWB21G]
status: done
updated: 2026-10-07
completed: 2026-10-07
---
# 葉 web-e2e-tmp

- 追加: `web/e2e/support/tmp-dir.ts`（`makeTmpDir(prefix)`・`removeTmpDir`・`cleanupTmpDirs`。作った dir は process の `exit` でも消す）。
  単体試験 `web/e2e/support/tmp-dir.test.ts`（vitest include の `e2e/support/**`）。
- 変更: spec・support 計 34 file の `mkdtempSync(path.join(tmpdir(), "celeris-…"))` を `makeTmpDir("celeris-…")` へ。既存の afterAll の rmSync は残した。
- `web/tsconfig.app.json` に tmp-dir.ts を追加（app project が import 元を含むため）。
- 証拠: `grep -rn mkdtemp web/e2e`（tmp-dir.ts 以外）→ 0 件。`pnpm exec vitest run e2e/support/tmp-dir.test.ts` → 2 passed。
  `pnpm exec tsc -b` → error 0。`pnpm exec biome check .` → error 0（warning 4、既存）。
- 未解決: playwright e2e 自体は実行していない（import 置換のみ）。
