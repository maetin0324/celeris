# P5-03 mobile・a11y の gate（全画面・4 幅）

tasks: [p5-03-mobile-a11y]

日付: 2026-10-01。計画: docs/web/implementation-plan.md §2 S3・S4、§P5-03（X10）。

## やったこと

- `web/scripts/mobile-audit.mjs` は引数なしで `web/e2e/support/screens.ts` の全行（31 行、fixture の重複
  `/org/cos` を 1 回にして 30 path。現行 gui の mobile-audit が見ていない 11 本を含む）× 幅 360 / 390 / 412 / 1440 を監査する。
  gateway は偽 daemon（空き port、`scripts/fixture-gateway.mjs`）に中継し、T1 / R1 / cos / P1 の fixture で中身まで描いて見る
  （以前は daemon なしで描いており、中身の操作要素を見ていなかった）。SSE が開いたままなので networkidle ではなく h1 を待つ。
  `web/scripts/screenshots.mjs` も同じ偽 daemon で撮る。
- `web/e2e/a11y/axe.spec.ts`（S4）は v3 の行だけでなく全行（`/login` を含む 31 本）にした。偽 daemon の fixture で描く。
- `web/e2e/parity/mobile-gate.spec.ts`（題 `parity-x: axe <fixture> 4 幅で …`）を足した。30 path × 4 幅で axe critical/serious 0
  と横溢れ 0 を見る。
- 落ちた画面: `/providers` の tiers の checkbox が 13×13（4 幅とも 6 個）。`size-11` にして直した。同じ形の
  `/knowledge/inbox` の「既存ページを上書き」と変更 tab の「取り返しがつかないことを確認した」の checkbox も `size-11` にした
  （fixture では出ない条件付きの要素）。

## 結果（画面 × 幅）

各セルは「横溢れ px / タップ 44×44・名前・構造（main 1・h1 1）・正の tabindex」の判定。axe は critical+serious の件数。

| path | fixture | 360 | 390 | 412 | 1440 | axe | S4 e2e |
|---|---|---|---|---|---|---|---|
| `/` | `/` | 0 / ok | 0 / ok | 0 / ok | 0 / ok | 0 | ok |
| `/inbox` | `/inbox` | 0 / ok | 0 / ok | 0 / ok | 0 / ok | 0 | ok |
| `/login` | `/login` | 0 / ok | 0 / ok | 0 / ok | 0 / ok | 0 | ok |
| `/org` | `/org` | 0 / ok | 0 / ok | 0 / ok | 0 / ok | 0 | ok |
| `/org/secretary` | `/org/cos` | 0 / ok | 0 / ok | 0 / ok | 0 / ok | 0 | ok |
| `/org/$id` | `/org/cos` | 0 / ok | 0 / ok | 0 / ok | 0 / ok | 0 | ok |
| `/projects` | `/projects` | 0 / ok | 0 / ok | 0 / ok | 0 / ok | 0 | ok |
| `/projects/$id` | `/projects/P1` | 0 / ok | 0 / ok | 0 / ok | 0 / ok | 0 | ok |
| `/projects/$id/docs` | `/projects/P1/docs` | 0 / ok | 0 / ok | 0 / ok | 0 / ok | 0 | ok |
| `/board` | `/board` | 0 / ok | 0 / ok | 0 / ok | 0 / ok | 0 | ok |
| `/knowledge` | `/knowledge` | 0 / ok | 0 / ok | 0 / ok | 0 / ok | 0 | ok |
| `/knowledge/inbox` | `/knowledge/inbox` | 0 / ok | 0 / ok | 0 / ok | 0 / ok | 0 | ok |
| `/knowledge/skills` | `/knowledge/skills` | 0 / ok | 0 / ok | 0 / ok | 0 / ok | 0 | ok |
| `/reports` | `/reports` | 0 / ok | 0 / ok | 0 / ok | 0 / ok | 0 | ok |
| `/approvals` | `/approvals` | 0 / ok | 0 / ok | 0 / ok | 0 / ok | 0 | ok |
| `/artifacts` | `/artifacts` | 0 / ok | 0 / ok | 0 / ok | 0 / ok | 0 | ok |
| `/tasks` | `/tasks` | 0 / ok | 0 / ok | 0 / ok | 0 / ok | 0 | ok |
| `/tasks/new` | `/tasks/new` | 0 / ok | 0 / ok | 0 / ok | 0 / ok | 0 | ok |
| `/tasks/$id` | `/tasks/T1` | 0 / ok | 0 / ok | 0 / ok | 0 / ok | 0 | ok |
| `/tasks/$id/files` | `/tasks/T1/files` | 0 / ok | 0 / ok | 0 / ok | 0 / ok | 0 | ok |
| `/tasks/$id/changes` | `/tasks/T1/changes` | 0 / ok | 0 / ok | 0 / ok | 0 / ok | 0 | ok |
| `/tasks/$id/runs/$runId` | `/tasks/T1/runs/R1` | 0 / ok | 0 / ok | 0 / ok | 0 / ok | 0 | ok |
| `/plans/new` | `/plans/new` | 0 / ok | 0 / ok | 0 / ok | 0 / ok | 0 | ok |
| `/daemon` | `/daemon` | 0 / ok | 0 / ok | 0 / ok | 0 / ok | 0 | ok |
| `/providers` | `/providers` | 0 / ok | 0 / ok | 0 / ok | 0 / ok | 0 | ok |
| `/accounts` | `/accounts` | 0 / ok | 0 / ok | 0 / ok | 0 / ok | 0 | ok |
| `/clusters` | `/clusters` | 0 / ok | 0 / ok | 0 / ok | 0 / ok | 0 | ok |
| `/releases` | `/releases` | 0 / ok | 0 / ok | 0 / ok | 0 / ok | 0 | ok |
| `/graph` | `/graph` | 0 / ok | 0 / ok | 0 / ok | 0 / ok | 0 | ok |
| `/help` | `/help` | 0 / ok | 0 / ok | 0 / ok | 0 / ok | 0 | ok |
| `/projects/$id/docs/maintenance` | `/projects/P1/docs/maintenance` | 0 / ok | 0 / ok | 0 / ok | 0 / ok | 0 | ok |

違反 0（S3・S4 とも）。

## 証拠コマンド

| コマンド | 結果 |
|---|---|
| `corepack pnpm@12.6.0 -C web mobile-audit` | exit 0、`mobile-audit: 30 path(s) x 4 widths ok`（修正前は `/providers` の 24 件で exit 1） |
| `corepack pnpm@12.6.0 -C web e2e a11y/ parity/mobile-gate.spec.ts --workers 4` | exit 0、191 passed（11.5 m） |
| `corepack pnpm@12.6.0 -C web screenshots --out <run artifacts>/shots` | 31 screen(s) x 4 widths、120 file（fixture の重複は同名で上書き） |
| `corepack pnpm@12.6.0 -C web typecheck` / `lint` | exit 0 / exit 0 |

スクリーンショットは run の artifacts（`shots/`）にあり、リポジトリには置かない。
