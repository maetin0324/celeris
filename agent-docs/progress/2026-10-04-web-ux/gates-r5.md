---
title: visual QA 最終検査（gates-r5）
tasks: [01M45SDF15D8S11KS0XGVFGM0A]
status: failed
updated: 2026-10-05
---

# visual QA 最終検査（gates-r5）

fix-r5 統合後の HEAD `a9d757b1`（`integrate wu/fix-r5 (phase place)`）で web/ の全検査を 1 回ずつ実行した。コードは変えていない。13 本のうち 12 本が exit 0、S1 latency が 1 本 exit 1（2 件失敗）。指示どおり直し・再実行はしていない。

## 実行環境

- worktree の web/ に offline install（`corepack pnpm@12.6.0 -C web install --offline --frozen-lockfile`）: exit 0。ERR_PNPM_NO_OFFLINE_TARBALL は起きず、store の写しは不要だった。
- 実行時の loadavg: install 4.77 / functional 12.80 / nfr 5.62 / S1 7.34 / mobile-audit 10.43（16 未満を待つ条件は即時に満たしていた）。S1 は preview 起動完了を Playwright の webServer（url 確認、timeout 120s）で待ってから 1 worker・retries 0 で 1 回だけ流した。

## 結果

| 検査 | コマンド | exit | 件数 | 時間 |
| --- | --- | --- | --- | --- |
| install | `corepack pnpm@12.6.0 -C web install --offline --frozen-lockfile` | 0 | lockfile 変更なし | ~30s |
| typecheck | `corepack pnpm@12.6.0 -C web typecheck` | 0 | tsc -b 差分なし | 4.7s |
| lint | `corepack pnpm@12.6.0 -C web lint` | 0 | 316 files / 5 warnings | 0.8s |
| test | `corepack pnpm@12.6.0 -C web test` | 0 | 58 files / 352 passed | 8.4s |
| build | `corepack pnpm@12.6.0 -C web build` | 0 | built in 398ms | 1.5s |
| check:boundaries | `corepack pnpm@12.6.0 -C web check:boundaries` | 0 | | |
| check:parity | `corepack pnpm@12.6.0 -C web check:parity` | 0 | | |
| check:secrets | `corepack pnpm@12.6.0 -C web check:secrets` | 0 | token absent | |
| gen:types --check | `corepack pnpm@12.6.0 -C web gen:types --check` | 0 | 差分なし | |
| mobile-audit | `corepack pnpm@12.6.0 -C web mobile-audit` | 0 | 31 path × 4 幅 ok | 25.8s |
| functional e2e | `corepack pnpm@12.6.0 -C web e2e --grep-invert 'S1 /'` | 0 | 186 passed / 8 skipped / 0 failed | 1.2m |
| e2e:nfr | `corepack pnpm@12.6.0 -C web e2e:nfr --retries=0` | 0 | 95 passed / 0 failed | 21.7s |
| S1 latency | `WEB_E2E_LATENCY_WORKERS=1 corepack pnpm@12.6.0 -C web e2e latency/transition.spec.ts --retries=0` | **1** | 29 passed / 2 failed / 0 skipped | 1.7m |

- 範囲: `git log --format= --name-only 7eab1be6a4e3..HEAD --not main` に `crates/`・`gui/`・`docs/api/` は無い（web/ と agent-docs/progress/ のみ）。
- 試験の skip・削除はしていない（functional の 8 skipped は元からの条件付き skip）。

## S1 の失敗と原因の見立て

| 試験 | 落ちたアサーション | 値 | 閾値 |
| --- | --- | --- | --- |
| `S1 /reports: JSON 0/5/10s でも shell 遷移は独立` | heading 10s-0s | 116.56ms | <= 100ms |
| `S1 /approvals: JSON 0/5/10s でも shell 遷移は独立` | URL 10s-0s | 140.94ms | <= 100ms |

見立て: 負荷依存の一過性の計測値で、コード由来の回退ではない。

- 落ちたのはいずれも delay 10s 側の計測（`measures[2] - measures[0]`）。同画面の 0s/5s 側は閾値内（/reports: url 37.7→69.9→135.1、heading 47.6→86.1→164.1。/approvals: url 39.4→30.3→180.4、heading 44.9→35.4→192.5）。delay 5s 側が 0s 側より速い点から、10s 側単発の計測が host 負荷で遅れたパターン。
- 実行時の loadavg 7.34（12.80→5.62→7.34 と変動中）で、S1 は計測値が負荷に依存する gate。gates-final（HEAD 9da059a7）でも S1 /providers が同様に一過性で落ちた後、gates-final2（HEAD 18df72f2）で 1 worker・retries 0・1 回のみで通っている。
- fix-r5 統合（7914a525..a9d757b1）で変わった web/ の file は artifact-table・console-view・project-detail-view・screenshots.mjs 等 8 本のみで、reports・approvals 画面と latency spec・support には差分が無い。reports/approvals 画面の直近変更 `bb8ce583` は旧 visual-qa 分岐（7914a525）以前のもので、この統合で入ったものではない。
- 指示どおり再実行していない（負荷が下がるのを待つ・再実行で通すことはしない）。

## 未解決事項

- S1 latency が 2 件失敗したため、本 WU の受け入れ条件 0（全検査が同一 HEAD で exit 0）は満たせない。replan で低負荷時間帯の再実行か、S1 の warm-up/閾値の見直しを別途行う必要がある。

## 提案

- なし。
