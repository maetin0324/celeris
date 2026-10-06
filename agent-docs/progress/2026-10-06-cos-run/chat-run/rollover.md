# chat-run / rollover: resume 拒否の fresh 再試行と context 上限での rollover

---
tasks: [01M47J2YNMZ14NQ4AXTNA355AA]
status: done
completed: 2026-10-06
---

## 実装

- `dispatcher/cos_chat/rollover.rs` を新設し、launch の 2 か所のフックから呼ぶ。
  - 起動時の `choose_session`：`sessions::cos_chat::decide_cos_chat_session` に照合 key・cache の有無・前回の run の終わり方・`[sessions] rollover_tokens` を渡す。resume するか、retire して fresh にするかを決め、`chat_run_record_session` に session_mode と理由（key_changed / cache_missing / resume_refused / token_rollover / context_exhausted）を残す。前回の run の終わり方は DB から読む。読むのは、現役 session で走った最新の終端 run の `reason` の印（`context exhausted` / `resume refused`）で、dispatcher のメモリには持たない。
  - run の中の `run_attempts`：resume した試行が `session_resume_failed` で終わったら、同じ chat run のまま次の順に処理する。
    1. session を retire し、fresh session（`FreshAfterRefusal`, `resume_refused`）を記録する。
    2. thread の summary を読み直し、同じ入力に対して `resume=false` で 1 回だけ再試行する。
    3. 2 回目も拒否・失敗なら、理由付きの failed にする。stop・割り込みで run が running でなくなっていれば、再試行しない。
  - 入力は claim し直さないので、`input_message_id` と `message.state` はこの run のままで、二重に配送されない。credential も同じ run のものなので、worker が同じ冪等 key で出す `cos_operations` は既存の記録が返り、二重に適用されない。1 回目の `result.json` は再試行の前に消す（actions カードを二重に出さない）。
  - summary の水位（`summary_through_seq`）は配送 cursor と別に扱う。再試行の直前に `chat_thread_summary` を読み直し、水位より後で入力より前の message だけを未要約履歴にする。入力は変えない。
  - 終端の usage（input+output）を session の `approx_tokens` に積む。launch は 0 を積んでいたので、`rollover_tokens` が効いていなかった。context 枯渇（`BudgetExhausted{kind: Context}` か sink の signal）を観測したら、run の理由に `context exhausted` を付ける。
- `launch.rs` の session の選択と履歴の組み立てを rollover の関数に置き換え、空のフック `rollover_hook` を消した。
- LLM 呼び出しは無い（summary は worker が checkpoint で書いたものだけを使う）。

## 検証

- `cargo test -p task-dispatch --lib cos_chat`：34 passed。新しい試験 5 件（`cos_chat_run_rollover_`）:
  - `resume_refusal_retries_fresh_exactly_once`：試行はちょうど 2 回。2 回目は `resume=false`。`fresh_after_refusal`/`resume_refused` を記録し、旧 session は retire 済み。
  - `second_refusal_fails_with_reason`：2 回目の拒否で failed（理由付き）。次の tick で 3 回目は起きない。
  - `retry_does_not_reapply_input_or_operation`：入力・run は同じで、queued は 0。operation の適用は入力ごとに 1 回で、credential は再試行でも有効。checkpoint で上がった summary の水位が再試行の履歴に反映され、入力は不変。
  - `token_limit_makes_next_run_fresh`：usage 110 > 100 なら次は `fresh`/`token_rollover`。
  - `context_exhaustion_makes_next_run_fresh`：次は `fresh`/`context_exhausted`。その後の通常 run は同じ session を resume する。
  - 偽 harness は FakeAdapter の sh スクリプトを包み、resume 拒否と session id の確定を報告する。待ちは worker の join だけで、sleep は無い。
- `cargo clippy --workspace --all-targets -- -D warnings`：exit 0
- `bash scripts/dev/test-parallel.sh`：exit 0。4120 tests run: 4120 passed, 12 skipped

## 未解決事項

- control 葉も `launch.rs` の `run_claimed`（終端の意図）を触る見込みで、integrate-control で衝突しうる。rollover 側は `run_attempts` の呼び出し 1 か所にまとめた。control の stop 意図は、`run_attempts` が返した `finish` を差し替える形で合わせられる。

## 提案

- 無し
