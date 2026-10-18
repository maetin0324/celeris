---
task: 01M56HCNTPN0H2VGP4JE0JYRM9
title: 本測定移行 P1-3 の進捗（K2/K4 実資産・能力 probe）
status: 実経路確認済みは K4 再計算・K2 組立・cap-probe 7/8 能力。K2 実 build の正負例と sirius-link は未成立（real-evidence.md）
updated: 2026-10-18
---
# P1-3 進捗

区分: 実装済み / 無 LLM 検証済み / 実経路確認済み / 承認待ち / 未検証。

| WU | 内容 | 区分 | 根拠 |
|---|---|---|---|
| cap-probe | 必要能力の固定（case.json）、A と同一経路の probe、B/C 同等、不足は env_failure で preflight 停止 | 実装済み・無 LLM 検証済み（fixture/stub runner で `cap_probe` 6 件） | 2026-10-18-mlsys-cap-probe/cap-probe.md |
| 〃 実 A sandbox 実走 | userns 要。人が host で | 未検証（人の実行証拠が未提供。exit・出力要点なし） | cap-runbook §1 |
| 〃 cap env 実値（UCX prefix・sirius 管理 socket） | host 側の承認が要る | 承認待ち（実値の承認・提供なし。値は記録しない） | cap-runbook §0 |
| grader-real | env/model 失敗の分離、K2 正負例・K4 再計算の script と試験 | 実装済み・無 LLM 検証済み（fixture 9 件。実 asset で組立・lock・patch・K4 再計算 ok） | 2026-10-18-p1-3-real-asset-grade/grader-real.md |
| 〃 K2 実 build の正負例 | async-ucx 入れ子 submodule 欠落で env_failure 停止 | 未検証（asset 追加と `--build` の人の実行証拠が未提供） | cap-runbook §2 |
| 〃 known-good patch（k2-fix.patch, k2-known-good-v2）の asset store 配置 | 配置済み・hash 一致（2026-10-18） | 確認済み | cap-runbook §2 |
| k2-scope | K2 の judged_scope、報告の主張制限 | 実装済み・無 LLM 検証済み（`k2_scope` 6 件） | 2026-10-18-p1-3-real-asset-grade/k2-scope.md |
| close | cap-runbook・本記録・全体検査 | 実装済み | 本 dir |
| Sirius 接続到達確認 | `ssh -O check` のみ | 未検証（人の実行証拠が未提供。exit・出力要点なし） | cap-runbook §3 |

有料ジョブ・本測定・LLM 呼出しは起動していない。

## K2 範囲の選択と理由
選択肢 2「判定範囲を事前明示」を採用（ADR agent-docs/adr/2026-10-18-k2-judged-scope.md）。K2 は implementation・experiment のみ判定し、analysis・review は not_judged。
理由: 分析/レビューの最小判定を足すには既知の誤り list と模範解答の新設が要り、結果を見た後に grader を変えない・巨大な問題集を作らない方針に反する。
K2 の成功は「実装と実験の判定を通った」ことだけを意味する。grade_case_ref を更新したので lock は新 hash で作り直し、旧試行と混ぜない。

## 旧 pilot 結果を流用しない
旧 pilot（取消済み task を含む）の結果・試行・lock は本 P1-3 の実経路証拠に使わない。取消を新しい実測承認とみなさない。
無 LLM 検証は fixture・stub・実 asset の決定的検査のみで、実経路確認は人手順（cap-runbook）の結果で初めて上がる。

## 本番への移行
- 本 task は本番 host・config・release・DB を変更していない。
- 人が行うこと: cap-runbook §0〜§3（cap env の承認と作成、実 A sandbox での cap-probe、asset store への入れ子 submodule と k2-fix.patch の追加（配置済み）と lock 固定、`verify-real-assets.sh --build`、Sirius 接続確認）。
- 本測定の開始は別途の承認（新しい測定 ID で campaign lock を作り直す）。

## 全体検査
- `bash scripts/dev/test-parallel.sh`: exit 0（passed 5739 / failed 0 / ignored 15）
- `cargo clippy --workspace -- -D warnings`: exit 0
- `cargo fmt --all -- --check`: exit 0

## 未解決
- 実 build の asset 不足（上記）。実 A sandbox 実走の結果。

## close 再実行（check 不合格の修正）
- 原因: `progress-index.sh --check` が `2026-10-17-root-dependency-waits-for-integration.md` の front matter の title・updated 欠落で exit 1。同ファイルに title/updated を足して解消。
- 全体検査: test-parallel 5739 passed / 0 failed（exit 0。初回は `a_program_pin_and_watch` の watch 再接続試験が高負荷で 1 件失敗、単独 3 回と再実行全体で合格＝時間依存の flaky）、clippy exit 0、fmt exit 0。

## host 実走証拠の記録（record-host, 決定 h1 後）
- 決定 h1 の回答は「人が runbook を実行し、結果を添える」方針の選択のみで、cap-probe（A/B/C）・`verify-real-assets.sh --build`・Sirius 接続確認の exit code と出力要点は回答にも thread 添付にも無かった。
- よって実経路確認済みに上げた項目は 0 件。上表の該当行は理由付きで 未検証 / 承認待ち のまま。証拠は作らない。
- 人が実行したら、各手順の exit と要点（鍵・socket 実値は除く）を渡し、該当行のみ更新する。

## 実資産・host 実走の記録（record-real, 決定 h-real 後）
人の host 実走結果を反映した（詳細は real-evidence.md）。区分:

| 項目 | 区分 | 根拠 |
|---|---|---|
| 実 asset 組立・SHA256SUMS・lock 照合（async-ucx 入れ子を含む） | 実経路確認済み | verify-real-assets.sh（build なし）exit 0 |
| K4 実 raw の再計算と grader 照合 | 実経路確認済み | 同上 `real_k4_raw_recompute_matches_the_grader` ok |
| cap-probe K4 A（python3・stdlib） | 実経路確認済み | exit 0、paid_calls_made 0 |
| cap-probe K2 A/B/C: cc・c++・cmake・git・ucx-pkg・min-build-ucx・min-build-cmake | 実経路確認済み | 3 arm とも ok（libucx-dev 1.18.1、ro_mounts 追加なし） |
| cap-probe K2 sirius-link | 未検証（修正済み・再実走待ち） | 初回 exit 255（sandbox 内 ssh_config 所有者拒否）。`-F /dev/null` に修正、host で直接は Master running |
| cap-probe K2 autotools・libclang | 未検証（能力を追加。実走なし） | worker host には無い |
| K2 実 build の known-bad fail / known-good pass | 未検証（env_failure で止まる） | autoreconf・libclang 不足、ucx snapshot が asset store に未配置 |
| ucx（ucx1-sys/ucx 938ffcd1）snapshot の asset store 配置 | 承認待ち | artifacts/ucx-938ffcd1.tar を人が配置 |
| K2 の input（lock）変更 | 実装済み | 新しい測定 ID で測定 lock を作り直す（旧と混ぜない） |
