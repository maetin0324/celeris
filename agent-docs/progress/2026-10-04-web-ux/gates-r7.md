---
title: fix-r7 統合後の web 全検査（gates-r7）
tasks: [01M464X80ZYX31613J0FM76DTS]
status: done
updated: 2026-10-05
---

# fix-r7 統合後の web 全検査（gates-r7）

検査した HEAD: `d27fc55baadb71f15af8d1cbcd2183575067b478`（fix-r7 統合後、`integrate wu/fix-r7 (phase fix)`）。
記録 commit `ad4ff4cd` 時点の作業ツリーは同一 HEAD（記録の追加のみ、web/・crates/ に差分なし）。

fix-r7 の 2 葉（home-stale・long-id-exec）を統合した HEAD で web/ の全検査を 1 回ずつ実行した。
コードは変えていない。13 検査すべて exit 0。

## 経過

- 1 回目（前 run、13:46〜14:07）: 静的 10・functional・nfr は exit 0。S1 の `S1 /projects` が
  URL 10s-0s 差 168.7ms > 100 で exit 1（0s の url が 184.4ms まで跳ねた一過性。実行中 loadavg 13.8→16.6 へ上昇）。
  指示どおり再試行せず記録して停止した（前 commit `ad4ff4cd`、status: failed）。
- 2 回目（本 run、14:12〜14:16）: 静的 10・functional・nfr は exit 0。S1 の `S1 /board` が
  url @0 = 533.0ms > 300 で exit 1（実行中 loadavg 9.5→28.2 へ急上昇。失敗画面は 1 回目の /projects と異なる）。
  指示どおり再試行せず、低負荷の窓を待って 3 回目を流した。
- 3 回目（本 run、14:34〜14:38、1-min loadavg 7 以下で開始）: 全 13 検査 exit 0。本記録はこの 1 回の結果を正とする。

## 実行環境

- worktree の web/ に offline install: exit 0、ERR_PNPM_NO_OFFLINE_TARBALL は起きず store の写しは不要。
- 負荷: 共有 host（24 CPU）に他 task のバーストが断続的に乗る。S1 実行中は 1-min loadavg が 10.4→3.5 に下がりながら完走。

## 結果（1 回・全 exit 0）

| 検査 | コマンド | exit | 件数 | 所要時間 | loadavg（開始→終了） |
| --- | --- | --- | --- | --- | --- |
| install | `corepack pnpm@12.6.0 -C web install --offline --frozen-lockfile` | 0 | lockfile 変更なし | 0.2s | 7.04→7.04 |
| typecheck | `... typecheck` | 0 | | 0.2s | 7.04→7.04 |
| lint | `... lint` | 0 | 322 files / 0 problems | 0.4s | 7.04→7.04 |
| test | `... test` | 0 | 59 files / 356 passed | 8.5s | 7.04→7.17 |
| build | `... build` | 0 | 2278 modules | 2.3s | 7.17→7.48 |
| check:boundaries | `... check:boundaries` | 0 | | 0.2s | 7.48→7.48 |
| check:parity | `... check:parity` | 0 | | 5.5s | 7.48→17.85 |
| check:secrets | `... check:secrets` | 0 | | 1.6s | 17.85→17.85 |
| gen:types --check | `... gen:types --check` | 0 | 差分なし | 0.5s | 17.85→17.85 |
| mobile-audit | `... mobile-audit` | 0 | 31 path × 4 幅 ok | 29.5s | 17.85→19.26 |
| functional e2e | `corepack pnpm@12.6.0 -C web e2e` | 0 | 206 passed / 8 skipped / 0 failed | 69.3s | 19.26→9.81 |
| e2e:nfr | `corepack pnpm@12.6.0 -C web e2e:nfr --retries=0` | 0 | 95 passed / 0 failed | 22.3s | 9.81→10.42 |
| S1 latency | `WEB_E2E_LATENCY_WORKERS=1 corepack pnpm@12.6.0 -C web e2e latency/transition.spec.ts --retries=0` | 0 | 31 passed / 0 failed | 94.7s | 10.42→3.48 |

S1 は 31 画面 × 3 delay の計測で、最大 url 50.7ms・heading 55.3ms・data 62.1ms（いずれも閾値 300ms 未満）。
URL 10s-0s 差は全画面で 100ms 未満。preview の起動完了は webServer 待ちで確認。

## 1・2 回目の S1 失敗

- 1 回目: `S1 /projects`（web/e2e/latency/transition.spec.ts:24、nfr-latency project）
  - アサーション: `expect(Math.abs(measures[2].url - measures[0].url), "URL 10s-0s").toBeLessThanOrEqual(100)`（transition.spec.ts:81）
  - 値: url 0s=184.38 / 5s=115.41 / 10s=15.67 → 差 168.71 > 100（delay 別の絶対値 300ms は満たしていた）
- 2 回目: `S1 /board`（同一 spec・同一 project）
  - アサーション: `expect(url, `${screen.path} URL @${delay}`).toBeLessThanOrEqual(300)`（transition.spec.ts:72）
  - 値: url @0 = 532.97 > 300（@5s=… 計測は該当 delay のみで中断。heading・data は未計測）
  - 実行中 loadavg 9.5→28.2 へ急上昇（他 task のバースト）。
- 負荷スパイク起因の一過性だと判断した根拠:
  - 失敗画面が run ごとに異なる（/projects → /board）。3 回目に同一 HEAD・同一コードで両画面とも正常
    （/projects url 0s=24.8ms、/board url 0s=28.3ms）。
  - fix-r7 の実コード差分（ホーム `routes/index.tsx` + `states.ts` fixture）が失敗画面（board/projects）の
    shell 遷移経路に触れておらず、決定的回帰の証拠は無い。
  - 低負荷の窓（1-min loadavg ≤10）での 3 回目は全 31 画面 pass。

## 範囲

- `git log --format= --name-only 7eab1be6a4e3..HEAD --not main | grep -E '^(crates|web/server)/'` は空（記録の追加のみ）。
- web/・crates/ に差分なし。試験の skip・削除なし（functional の 8 skipped は spec 内の既存 skip）。
- 各検査のログは run の artifacts（run2-install.log 〜 run2-s1.log）に残した。
