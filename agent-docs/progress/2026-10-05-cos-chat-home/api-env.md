---
title: CoS チャットホーム api-env（CoS run の celerisctl が daemon 自身の API を使う）
tasks: [01M46VVAD0ZAVZ9C4Q0KJM9ESV]
status: done
updated: 2026-10-07
---
# CoS チャットホーム — api-env WorkUnit

live-check の不具合 2（CoS run の `celerisctl` が既定の `~/.config/celeris` を読み、別の daemon の API へ送る）を直した。

## 変更
- `crates/task-dispatch/src/dispatcher/cos_chat/launch.rs`: `cos_run_env` が CoS chat run と triage run（同じ `start_thread` を通る）の env を組み立てる。
  中身は run credential、`CELERIS_API_URL`（`[api] listen` から作る `http://127.0.0.1:<port>/api/v1`。空なら入れない）、`PATH`（daemon 自身の実行ファイルの dir を先頭に 1 回だけ置き、daemon の PATH を後ろに続ける）。
  以前は PATH を触っておらず、daemon と同じ release の celerisctl が先に見つかる保証はなかった。
- `crates/celerisctl/src/commands/cos_ops.rs`: `api_config` の優先順を `--api-url` > `CELERIS_API_URL` > `CELERIS_CONFIG` の `[api] listen` にした。
  `/api/v1` で終わるかの検証は env にも同じく掛け、誤りの文言には値の出どころ（`--api-url` / `CELERIS_API_URL`）を出す。
- 試験 `cos_chat_run_launch_sets_api_url_env_and_path` は `cos_chat/launch/tests.rs`（launch の子 module）に置く。attempt 1 は `dispatcher/tests/cos_chat_launch.rs` に置いて範囲 check に落ちたので、attempt 2 で移した。dispatcher の試験 fixture は `dispatcher::tests` の private 関数で launch の子からは使えないため、launch で adapter の `with_env` に渡す `cos_run_env` の値（`CELERIS_API_URL`・credential・PATH の先頭）を直接確かめる形にした。
- 前置き（`task_worker::cos_chat::build_prompt`）は `--api-url` を必須とは書いていないので、変更していない（task-worker は並行する WU の範囲）。

## 証拠
- `cargo nextest run -p celerisctl -p task-dispatch -E 'test(cos_chat_ops_ctl_) | test(cos_chat_run_launch_)'`: 21 passed。
  新しい試験は `cos_chat_ops_ctl_prefers_api_url_env_over_config`（env が config に勝つ・flag が env に勝つ・env の検証）と
  `cos_chat_run_launch_sets_api_url_env_and_path`（run の env の `CELERIS_API_URL` と PATH の先頭、継いだ PATH が残ること、listen なしでは入らないこと、重複なし）。
- 範囲 check（`CELERIS_WU_BASE` 比の差分が celerisctl・cos_chat/launch・progress だけ）: exit 0。
- `bash scripts/dev/test-parallel.sh`: exit 0、passed 4589 / failed 0 / ignored 14。
- `cargo clippy --workspace -- -D warnings`: exit 0。

## 未解決事項
- adapter の `with_env` は config の env より後に足すので、`[adapters.<id>]` の env が `PATH` を持っていても CoS run では daemon の PATH を土台にした値が勝つ。今の本番 config に PATH の上書きはない前提。必要になったら、config の PATH を後ろに続ける形にする。
- 実機での確認は live-check2 で行う。

## 提案
- なし。
