---
title: fix-r7 統合後の web 全検査（gates-r7）
tasks: [01M464X80ZYX31613J0FM76DTS]
status: failed
updated: 2026-10-05
---

# fix-r7 統合後の web 全検査（gates-r7）

検査した HEAD: `d27fc55baadb71f15af8d1cbcd2183575067b478`（fix-r7 統合後、`integrate wu/fix-r7 (phase fix)`）

fix-r7 の 2 葉（home-stale・long-id-exec）を統合した HEAD で web/ の全検査を 1 回ずつ実行した。
コードは変えていない。静的 10 検査・functional・nfr はすべて exit 0。S1 の 1 件だけ決定的に失敗し、
指示（再試行・負荷待ちで通さない）どおり記録して止めた。

## 実行環境

- worktree の web/ に offline install: exit 0、ERR_PNPM_NO_OFFLINE_TARBALL は起きず store の写しは不要。
- 開始時 loadavg 24.2（13:46）から 13:49 に 3.8 まで下がったところで検査開始。
- nfr 終了時 loadavg 13.8、S1 終了時 16.6 で、S1 中は共有 host の負荷が上昇し続けていた。

## 結果

| 検査 | コマンド | exit | 件数 | 所要時間 | loadavg（終了時） |
| --- | --- | --- | --- | --- | --- |
| install | `corepack pnpm@12.6.0 -C web install --offline --frozen-lockfile` | 0 | lockfile 変更なし | 0.7s | 3.37 |
| typecheck | `... typecheck` | 0 | | 1.1s | 3.31 |
| lint | `... lint` | 0 | 322 files / 5 warnings | 0.3s | 3.31 |
| test | `... test` | 0 | 59 files / 356 passed | 8.4s | 2.86 |
| build | `... build` | 0 | 2278 modules | 1.1s | 2.49 |
| check:boundaries | `... check:boundaries` | 0 | | 0.2s | 2.54 |
| check:parity | `... check:parity` | 0 | | 2.2s | 2.41 |
| check:secrets | `... check:secrets` | 0 | | 0.7s | 2.41 |
| gen:types --check | `... gen:types --check` | 0 | 差分なし | 0.2s | 2.12 |
| mobile-audit | `... mobile-audit` | 0 | 31 path × 4 幅 ok | 25.9s | 2.28 |
| functional e2e | `corepack pnpm@12.6.0 -C web e2e` | 0 | 206 passed / 8 skipped / 0 failed | 70.0s | 2.54 |
| e2e:nfr | `corepack pnpm@12.6.0 -C web e2e:nfr --retries=0` | 0 | 95 passed / 0 failed | 23.8s | 13.81 |
| S1 latency | `WEB_E2E_LATENCY_WORKERS=1 corepack pnpm@12.6.0 -C web e2e latency/transition.spec.ts --retries=0` | **1** | 30 passed / 1 failed | 103.4s | 16.59 |

## 失敗の原因（S1）

- 落ちた試験: `S1 /projects: JSON 0/5/10s でも shell 遷移は独立`（web/e2e/latency/transition.spec.ts:24、project nfr-latency）
- アサーション: `expect(Math.abs(measures[2].url - measures[0].url), "URL 10s-0s").toBeLessThanOrEqual(100)`（transition.spec.ts:77）
- 値: `measures[0].url = 184.38`（delay 0）、`measures[1].url = 115.41`（delay 5000）、`measures[2].url = 15.67`（delay 10000）→ 差 **168.71 > 100**
- delay 別の絶対値アサーション（url/heading ≤ 300ms）は 3 回とも満たしている。失敗は 0s 計測の url だけが 184ms まで跳ねたことで生じた差の閾値超過。
- 経過: nfr 終了時 loadavg 13.8、S1 終了時 16.6 で負荷が上昇中。計測値の形状（0s が最大）は前回の S1 /providers 一過性失敗（url 計測 233ms に跳ねて URL 10s-0s が 100ms 超、再実行で 30ms・pass）と同種。
- 指示どおり負荷待ち・再試行は行わず、この 1 回を正とする。

## 範囲

- `git log --format= --name-only 7eab1be6a4e3..HEAD --not main | grep -E '^(crates|web/server)/'` は空（記録の追加のみ）。
- web/・crates/ に差分なし。試験の skip・削除なし（functional の 8 skipped は spec 内での既存 skip）。
- ログは run の artifacts（install.log・typecheck.log・lint.log・test.log・build.log・check_*.log・gentypes.log・mobileaudit.log・functional.log・nfr.log・s1.log）に残した。

## 未解決事項

- S1 の 1 件失敗（上記）。閾値超過が一過性の負害か、/projects 特有の遅延かは再実行なしでは切り分けられない。
  低負荷時（loadavg 4〜12）の一巡で再検証する必要がある。

## 提案

- なし（再実行・負荷待ちは指示で禁止のため、次の run で低負荷時に S1 を 1 回だけ再検証する）。
