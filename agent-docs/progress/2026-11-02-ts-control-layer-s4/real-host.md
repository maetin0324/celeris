---
title: S4 real-host — 本物の deno_core host で TS の async pass を mode compare・ts で走らせ、apply と fallback・再起動を確かめる
tasks: [01M6EVP9GT07K2G01EMW0E6HNC]
status: done
completed: 2026-11-03
updated: 2026-11-03
---
# S4 real-host — 本物の host での mode ts の試験

完了日: 2026-11-03（unit real-host）。境界 ADR 付記 S4-14・S4-15 の本物の host での確認。

## 結論
- `crates/celeris/tests/script_compare_cos_digest.rs` に 4 件を足した（既存 5 件と合わせて 9 件）。本物の bundle（`control-ts/dist`）を `ScriptRuntimeHandle` に載せ、dispatcher の digest pass を mode ts・compare で走らせる。userns・外部ネットワーク・LLM は使わない。
- 試験で不備を 1 件見つけて直した: daemon の起動時の復元（`crates/celeris/src/daemon/script_layer.rs` の `restore_modes`）は `compare` の行だけを `rust` へ戻し、`ts` の行を戻していなかった（mode-ts unit の未解決事項に書かれていたもの）。付記 S4-14 のとおり、generation の要る mode（`needs_generation()`。`compare`・`canary`・`ts`）を対象にした。直す前の code では `..._daemon_ts_restart_keeps_one_message` が「mode が ts のまま」で落ちることを確かめた。

| 目的文の項目 | 試験 | 確かめたこと |
|---|---|---|
| mode ts の tick で TS の pass が op を await し、command が検証の後に apply される | `script_compare_cos_digest_ts_mode_applies_ts_commands` | completed と stopped の 2 run。記録は run ごとに 1 件、`mode = ts`・`applied_by = ts`・`fallback = null`・`outcome = match`。op の log は `list_runs`・`read_run` が `ok`（store から読んだ）。message・item の行は mode rust の基準と byte 一致。2 回目の tick と作り直した dispatcher の tick で増えない |
| TS の command を apply した証拠 | 同上（後半） | 本物の bundle の出力の `post_digest.text` に印 `(ts-applied)` を足す bundle（規則 1〜9 には通る）では、DB の 2 件の message の本文が印で終わる。`applied_by = ts`・`outcome = mismatch`・requeue は基準と同じ |
| compare では TS の op が Rust の snapshot から答え、store の読み出しは 1 回だけ | `script_compare_cos_digest_compare_answers_ops_from_rust_snapshot` | tick（Rust の読み出しと apply）の後、TS の pass を待つ前に `cos_inbox_items`・`chat_runs` の表の名前を変えても `match`。TS の op はすべて `memo`。`applied_by = rust` |
| 常に違う答え・throw・op の denied・deadline 超えで Rust の command | `script_compare_cos_digest_ts_mode_falls_back_to_rust` | 4 種とも message・item は基準と byte 一致、`applied_by = rust`。`fallback.code` は順に `run_missing`（`invalid_output`）・`ts_error`（`exception`）・`ts_error`（安全違反 `denied_scope`、一覧に無い run の read を dispatcher が拒否）・`ts_error`（`timeout`、答えない promise。CPU は使わない） |
| mode ts のまま daemon を再起動しても message は 1 件、bundle が無ければ rust に戻る | `script_compare_cos_digest_daemon_ts_restart_keeps_one_message` | 同じ bundle の daemon は ts を保ち、記録は 1 件（`mode = ts`）。再起動の後も message（冪等 key `cos-triage-digest:run-a-done`）は 1 件で記録も増えない。`[script]` の無い daemon は `system:script-startup` で rust に戻し、新しい run を Rust が digest する（記録は増えない） |

mode ts へは compare を経てだけ入れる（`TsRequiresCompare`）ので、試験は human の actor で compare→ts と切り替える。

## 受け入れ条件と証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- 条件 0（`CELERIS_SCRIPT_REQUIRE_BUNDLE=1` で 0 skipped）: `corepack pnpm@12.6.0 -C control-ts run bundle` の後、`CELERIS_SCRIPT_REQUIRE_BUNDLE=1 cargo nextest run -p celeris --test script_compare_cos_digest` → 9 passed、0 skipped（3 回続けて同じ）。
- 条件 1（mode ts で TS の command が apply された記録）: 上の `..._ts_mode_applies_ts_commands` が `applied_by = ts` の記録と、印の付いた TS の文面が DB に入ったことを確かめる。
- `CELERIS_SCRIPT_REQUIRE_BUNDLE=1 bash scripts/dev/test-affected.sh` → exit 0（対象 crate は celeris・celerisctl、nextest 879 passed・0 skipped）。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo clippy -p celeris --test script_compare_cos_digest -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- 範囲: `sh "$CELERIS_WU_SCOPE_PATHS"` の変更 path は `crates/celeris/src/daemon/script_layer.rs`・`crates/celeris/tests/script_compare_cos_digest.rs`・この記録。

## commit（件名）
- phase s4: real-host: 本物の host で mode ts の apply・fallback・再起動を確かめ、起動時の復元を ts にも当てる
- phase s4: real-host: 進捗の記録を足す

## 未解決事項
- 付記 S4-14 の起動時の復元の条件のうち「manifest の `cos_digest` に capability が足りない」は、`restore_modes` が bundle の hash だけで判定するので扱っていない（hash が同じなら capability も同じなので、配備済みの bundle では起きない想定。未確認）。
- daemon の試験は、cold の isolate で 200 ms を超えたとき Rust の apply（`ts_error`・`timeout`）も許す。TS の apply は tick の試験で確かめている。

## 本番への移行
- 本番の `cos_digest` は `rust` のまま。起動時の復元が `ts` の行も戻すようになったが、本番に `ts` の行は無い想定なので挙動は変わらない。
