---
title: "A 腕の認証とアカウント可変状態を試行 identity ごとに分離（P0-1）"
tasks: [01M56HCNQX7S26ZYGY071A19E4]
status: done
updated: 2026-10-18
completed: 2026-10-18
---
# A 腕の試行 state 分離と CLI 停止・再起動 e2e

設計は ADR `agent-docs/adr/2026-10-18-a-trial-scoped-state.md`。WU: trial-state（Sandbox・marker）→ preflight-split（新規/再開検査）→
cli-e2e（本書の e2e）。WU ごとの記録は `2026-10-18-trial-state-isolation/` 配下。

## 区分
| 項目 | 区分 |
|---|---|
| A の認証は `.credentials.json` 1 件の ro mount、transcript/memory/設定は `<out>/trial-state/<campaign>/<arm>/<case>/rep<N>/` | 実装済み |
| 新規試行検査 / 再開検査（identity・lock 不一致・混入の拒否） | 実装済み・無 LLM 検証済み |
| 実 CLI 入口 → preflight → executor の停止（SIGSTOP/SIGKILL）・再起動・同一実行への再接続 | 無 LLM 検証済み（stub A を実 bwrap で実行。実 process・実 task-api router） |
| 別 arm/case からの前試行識別文字列の不可視、混入 state の起動前拒否 | 無 LLM 検証済み |
| 実 claude（実 LLM・実アカウント）での同一確認 | 未検証（承認待ち。本測定の開始・実測は行っていない） |
| 本番 host の実 pilot 経路確認 | 未検証 |

実経路確認済み（実 LLM を通した確認）は無い。旧 pilot の結果は流用していない。

## 試験: `crates/campaign/tests/trial_state_cli_e2e.rs`
- 実 `celeris-campaign watch --executor harness` を子 process で起動。task-api は実 router（`task_api::serve_with_listener`、
  loopback）で、案件は実 `POST /projects` と `/repos`、A のアカウントは `[accounts] claude_dir` の実 scan（`GET /llm/accounts`）。
  手書きの API 形式・DB 補完なし。manifest は final.json（A/B/C × 2 case = 6 試行、max_parallel=1）。
- A の stub は bwrap 内で transcript・memory に `HIST-<arm>-<case>-<rep>` を書き、`trial-state` の兄弟を grep して
  見えた物を `foreign.txt` に残し、gate file が開くまで待つ。gate は state dir 内の file（時計も負荷も使わない）。
- `killed_cli_restarts_and_reattaches_to_the_same_trial_without_resubmit`:
  CLI 1 を SIGSTOP（`T` 状態を確認）→ その間に stub が先へ進む → SIGKILL。再開検査の拒否（marker の rep を書き換えると
  exit 1・`trial-state`・launch 増なし）→ 復元 → 同じ引数で CLI 2 を起動。結果: 6 試行すべて `launches`=1 行（二重投入なし）、
  台帳 6 行・全 done、idempotency key と予約額は kill 前と同一（予約は解放されない）、`trials.jsonl` の tokens は全試行 120
  （再起動前 105 + 後 15 を合算。0 化なし）、`watch.end=settled`。6 試行の履歴は互いに別の 1 文字列、`foreign.txt` は全て空、
  `trial-state` root の一覧も見えず、アカウント dir の `projects/old` は試行へ届かない、state の `.credentials.json` は空 file
  （mount 先）。認証値・API token は CLI の stdout/stderr・state に出ない。
- `contaminated_new_trial_state_is_refused_before_any_launch`: 新規試行 state に他 run の projects 履歴、または別試行の
  marker があると exit 1（`trial-state … not empty`）。台帳も `harness-state` も作られず、混入は削除されず残る。

## 証拠コマンドと結果
- `cargo test -p celeris-campaign --test trial_state_cli_e2e` → 2 passed（約 11 秒）
- `cargo fmt --all -- --check` → exit 0
- `cargo clippy --workspace -- -D warnings` → exit 0（`--tests` も exit 0）
- `bash scripts/dev/test-parallel.sh` → 下記「全体試験」

## 全体試験
`bash scripts/dev/test-parallel.sh` → exit 0。nextest passed 5726 / failed 0 / ignored 15（243 binaries）、doctest exit 0。

## 本番への移行
なし。この WU は試験と文書だけで、config 欄・API・本番 host に変更は無い（前 WU の `Sandbox` 変更も config 欄の削除は無い）。
実 pilot 前に人が行う手順: `celeris-campaign preflight --out <本番 out> …` を通す（`trial-state` の check が出る）。

## 未解決・提案
- 実 claude での再接続確認は承認待ち（実 LLM を使うため。本 task では行っていない）。
