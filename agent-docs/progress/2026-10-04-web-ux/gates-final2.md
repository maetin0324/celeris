---
title: visual QA 最終検査（gates-final2）
tasks: [01M45MEFZ09VXFJ00B30FQ4WT7]
status: done
updated: 2026-10-05
---

# visual QA 最終検査（gates-final2）

fix-e2e-r4 の後の HEAD `18df72f2` で web/ の全検査を実行した。コードは変えていない。
前回 gates-final（HEAD 9da059a7）は functional 6 件の失敗（task-detail・rich-data・console の spec 追従 5 件と S1 /providers の一過性 1 件）で止まっていたが、今回は全検査が通った。

## 実行環境

- worktree の web/ に offline install（`corepack pnpm@12.6.0 -C web install --offline --frozen-lockfile`）: exit 0。ERR_PNPM_NO_OFFLINE_TARBALL は起きず、store の写しは不要だった。
- 実行開始時の loadavg 11.7、S1 開始時 3.7（16 未満を待つ条件は即時に満たした）。

## 結果

| 検査 | コマンド | exit | 要点 |
| --- | --- | --- | --- |
| install | `corepack pnpm@12.6.0 -C web install --frozen-lockfile` | 0 | lockfile 変更なし |
| typecheck | `... typecheck` | 0 | |
| lint | `... lint` | 0 | |
| test | `... test` | 0 | 58 files / 352 passed |
| build | `... build` | 0 | 2276 modules |
| check:boundaries | `... check:boundaries` | 0 | |
| check:parity | `... check:parity` | 0 | |
| check:secrets | `... check:secrets` | 0 | |
| gen:types --check | `... gen:types --check` | 0 | 差分なし |
| mobile-audit | `... mobile-audit` | 0 | 31 path × 4 幅 ok |
| functional e2e | `corepack pnpm@12.6.0 -C web e2e --grep-invert 'S1 /'` | 0 | 181 passed / 8 skipped / 0 failed（1.1 分） |
| e2e:nfr | `corepack pnpm@12.6.0 -C web e2e:nfr --retries=0` | 0 | 95 passed / 0 failed（21 秒） |
| S1 latency | `WEB_E2E_LATENCY_WORKERS=1 corepack pnpm@12.6.0 -C web e2e latency/transition.spec.ts --retries=0` | 0 | 31 passed / 0 failed（1.6 分、1 回のみ・再実行なし） |

- 範囲: `git log --format= --name-only 7eab1be6a4e3..HEAD --not main | grep -E '^(crates|gui|docs/api)/'` は空。
- ログは run の artifacts（static.log・functional.log・nfr.log・s1.log）に残した。

## 未解決事項

- なし。S1 /providers は前回一過性で落ちたが、今回は 1 worker・retries 0 で通った（計測は負荷に依存するため、高負荷時の再発余地は残る）。

## 提案

- なし。次は record-final で UI/UX 判定の記録に進める。
