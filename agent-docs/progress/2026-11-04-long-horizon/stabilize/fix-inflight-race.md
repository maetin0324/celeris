---
title: script_generation_e2e の in_flight 競合の修正
tasks: [01M6MH1H0RTRBMZF1C113WBQRZ]
status: done
completed: 2026-11-05
updated: 2026-11-05
---
# script_generation_e2e の in_flight 競合の修正

完了日: 2026-11-05

## 結論
`script_generation_e2e_switch_error_rollback_restart` が `stage_switch_to_b` の `start_pass` 直後の `assert_eq!(in_flight, 1)` で left 2 / right 1 になる原因は、dispatcher が「空の走査」の pass をもう 1 回 A へ送ることだった。試験側だけを直した。製品コードは変えていない。

## 原因
- `crates/task-dispatch/src/dispatcher/cos_chat/digest.rs` の `cos_digest_pass` は、先頭で `launch.triage.digest_due = true` にし、空の走査（`list.is_empty()`）が成功したときだけ `false` に戻す。runs を適用した pass の後も `digest_due` は true のままなので、次の tick でもう 1 回 pass が走る。
- mode `ts`（canary 選択）では、この pass が走査の前に TS を呼ぶ（同関数の `call_cos_digest_pass` 経由の `serve_pass`）。空の走査でも A の isolate に pass が入り、in_flight が 1 増える。
- 試験は前の段の `digest_until_ts_applied` が返った後、`in_flight == 0` を待ってから `start_pass` していた。この待ちと `start_pass` の間に dispatcher の次の tick が入ると、空の走査の pass が A で動いている間に試験の pass が重なり、in_flight が 2 になる。in_flight が増えるのは `crates/script-runtime/src/generation.rs` の要求受付（`fetch_add`）、減るのは guard の drop（`fetch_sub`）。
- この pass は dispatcher の tick の中で同期的に終わる。止める必要はなく、終わるのを待てばよい。

## 直し方
`crates/celeris/tests/script_generation_e2e.rs` の `stage_switch_to_b` で、`start_pass` 直後の即時 assert と、drain 中に A が保持する pass の assert を、`wait_for`（出来事待ち）で `in_flight == 1` になるのを待つ形にした。試験の pass は op の答えを止めているので in_flight に残り続ける。重なった dispatcher の pass が終われば 1 件だけになる。sleep の延長や負荷は使わない。他の試験の意味は変えていない。
切替後の `cancelled_in_flight == Some(0)` の assert は、この pass が終わった後の値なので変えていない。

## 証拠
- 修正前の失敗は release gate の final review で観測（800 行付近、left 2 / right 1）。ローカルでは再現できなかった（競合は dispatcher の tick の位相に依存し、決定的に起こす hook が無い）。原因は上のコード読みによる。
- 修正後: `control-ts` で `pnpm install --offline` の後、`CELERIS_SCRIPT_REQUIRE_BUNDLE=1 cargo nextest run -p celeris --test script_generation_e2e` を 3 回実行し、3 回とも 1 test passed（5〜6 秒）。bundle が無いと試験は何もせず早期 return する点に注意。
- `cargo fmt --all -- --check` と `cargo clippy --workspace -- -D warnings` は下の報告のとおり。

## 未解決・提案
- 空の走査の pass が毎回 TS を呼ぶこと自体は仕様の挙動（ADR の S5 系）。変更の提案はしない。
