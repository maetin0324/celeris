---
task: 01M417CC9HME43M4WJJZZB25RN
work_unit: gui-web-verify
status: done
completed: 2026-10-03
base: 7b33c4185ed3
---

# gui・web の型検査と試験、clippy（gui-web-verify）

WorkUnit `gui-web-verify`（工程 record、task「gui・web の型検査と試験、clippy を流し、
review-sync-main.md に PROGRESS v2 の節を書く」）の実行記録。main 取り込み後の HEAD
（`7b33c4185ed3`）で gui・web の install/typecheck/test と `cargo clippy --workspace`
を流した。全て exit 0 で、gui/・web/・docs/ の修正は不要だった（`crates/` を含め
working tree に差分なし）。

## 実行結果

| コマンド | exit | 要点 |
|---|---|---|
| `corepack pnpm@11.27.0 -C gui install --frozen-lockfile` | 0 | 340 packages、lockfile は supply-chain policies 検証済み |
| `corepack pnpm@11.27.0 -C gui typecheck` | 0 | `react-router typegen && tsc -b`、エラーなし |
| `corepack pnpm@11.27.0 -C gui test` | 0 | vitest: 90 test files / 1287 tests すべて passed |
| `corepack pnpm@12.6.0 -C web install --frozen-lockfile` | 0 | 200 packages、lockfile は supply-chain policies 検証済み |
| `corepack pnpm@12.6.0 -C web typecheck` | 0 | `tsc -b`、エラーなし |
| `corepack pnpm@12.6.0 -C web test` | 0 | `vitest run && node --test server/*.test.mjs`: vitest 26 files / 188 tests + node:test 42 tests、すべて passed |
| `cargo clippy --workspace -- -D warnings` | 0 | warning 0 件、`Finished dev profile` |

install は初回 run で `gui/node_modules`・`web/node_modules` が無かったため、非 TTY 環境向けに
`CI=true ... --prefer-offline` を付けて実行した（人の決定「leaf のまま 1 run で試す」の指示どおり）。
TTY 確認で止まることはなく、問題なく完了した。

## crates/ への影響

`git diff --stat crates/` は空、`git status --short` も空（working tree は clean のまま）。
main 取り込み由来の型ずれ・`gui/app/celeris/types.ts` や `web/api/generated/schema.json` と
`docs/api` の schema のずれは見つからなかった。gui/・web/・docs/ のいずれにも修正は不要だった。

## 未解決事項

なし。4 条件（gui install/typecheck/test、web install/typecheck/test、clippy、crates 差分ゼロ）
すべて exit 0 で確認済み。

## 提案

なし。
