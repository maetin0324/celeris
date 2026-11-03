---
title: S4 rust-split — digest を snapshot→command→検証→apply に分ける（挙動不変）と比較 fixture
tasks: [01M6EVP9GT07K2G01EMW0E6HNC]
status: done
completed: 2026-11-02
updated: 2026-11-02
---
# S4 rust-split — digest の分割と cos_digest の比較 fixture

完了日: 2026-11-02（unit rust-split）。挙動を変えない Rust だけの変更。TS は書いていない。

## 結論
- `CosTriageItemReport`・`CosTriageDecision`・`CosTriageOption`（`crates/task-core/src/chat/triage.rs`）に `Serialize`・`Deserialize`（`deny_unknown_fields`）・`JsonSchema` を付けた（別 commit）。
- 新 module `crates/task-core/src/chat/digest.rs`: DTO `CosDigestInput`・`CosDigestCommand`（`kind` = `post_digest`／`requeue_run`）、純粋関数 `cos_digest_decide`、検証関数 `validate_cos_digest_commands`（違反は `CosDigestViolation`、`code()` が S4-3 の code を返す）。
  文面の組み立て（旧 `digest_message`・`judgment`・`options_line`）、`kind_label`・`card_kind` を dispatcher から移した。dispatcher 側は `pub(crate) use` で同じ関数を参照する（重複定義なし）。
- `crates/task-dispatch/src/dispatcher/cos_chat/digest.rs` の `digest_one` を `digest_snapshot`→`cos_digest_decide`→`validate_cos_digest_commands`→`apply_digest_commands` に組み替えた。apply は command の順に `chat_assistant_message_add_once`・`cos_triage_requeue_run` を呼び、最初の失敗で止める（投入の失敗で requeue しない点は従来どおり）。
- fixture: `cargo run -q -p task-core --example dump-cos-digest-fixtures` が `control-ts/fixtures/cos_digest/cases.jsonl` を出す（2,066 行 = 手書き 66 + 固定 seed の SplitMix64 生成 2,000、入力の重複なし、約 4.5 MB）。

## 挙動不変の根拠
- 検証が拒むのは参照実装が出さない形だけ。参照実装が落ちうる規則 6（text 64 KiB 超・key 128 byte 超）は、従来も store（`chat_daemon_message_add_keyed`・`chat_assistant_message_add_once`）が同じ上限で Err を返し、message も requeue も起きなかった。今も Err で同じ。
- `chat_run_get` の失敗を `reason = None` にする扱い、`requeued > 0` で `dirty` を立てる扱いは同じ。

## 受け入れ条件と証拠（提出時の HEAD で実行）
- 条件 0（既存の inbox thread 試験が無修正で通る）: `git diff $CELERIS_WU_BASE -- crates/task-dispatch/src/dispatcher/tests/` は空。`cargo nextest run -p task-dispatch -E 'test(/cos_chat_triage::/) | test(/fallback/)'` → 65 passed（`inbox_thread` の 11 件を含む）。
- 条件 1（cases.jsonl が example の出力と byte 一致、2,000 行以上）: `cargo run -q -p task-core --example dump-cos-digest-fixtures | cmp - control-ts/fixtures/cos_digest/cases.jsonl` → 一致。試験 `script_fixtures_cos_digest_match_committed_fixture` が同じ関数の出力と commit 済み file の一致を確かめる。`script_fixtures_cos_digest_cases_are_valid_and_cover_boundaries` が全行の型付き往復・参照出力の検証通過・run_state 5 種（未知の `cancelled` を含む）と item state（escalated/fallback/running/handled）の網羅を確かめる。
- 条件 2（検証関数が規則ごとの違反を拒む）: `script_fixtures_cos_digest_validation_rejects_each_rule` が規則 1〜6 を 1 件ずつ壊した command（14 通り）を拒み、code（`key_mismatch`・`scope_mismatch`・`card_outside_items`・`requeue_state_invalid`・`duplicate_command`、規則 6 は code なし）を確かめる。`script_fixtures_cos_digest_dtos_deny_unknown_fields` が未知の欄の拒否を確かめる。`cargo nextest run -p task-core --test script_fixtures` → 12 passed。
- `bash scripts/dev/test-affected.sh` → exit 0（nextest 6,980 passed・21 skipped、web 697 passed、control-ts 48 passed）。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。

## commit（件名）
- phase s4: rust-split: CosTriageItemReport と入れ子の型に Serialize・Deserialize・JsonSchema・deny_unknown_fields を付ける
- phase s4: rust-split: digest を snapshot→decide→validate→apply に分け、cos_digest の比較 fixture を足す

## 付記（S4-2・S4-9）との食い違い
- 名前: 付記 S4-9 は `digest_decide`・`validate_digest_output`・`CosDigestOutput`・`DigestViolation` と書くが、unit の objective に従い `cos_digest_decide(&CosDigestInput) -> Vec<CosDigestCommand>`・`validate_cos_digest_commands`・`CosDigestViolation` にした。置き場所も task-core（dispatcher ではない）。TS 側の出力の包み（`{commands: [...]}`）は ts-digest／hook unit で決める。
- `run_state` は付記の 4 値の enum ではなく文字列にした。今の `digest_message` は未知の値を `run は <state> で終わった` と書くので、enum にすると挙動が変わる。
- 規則 6 の違反を `MissingPostDigest`・`TextTooLarge`・`KeyTooLarge` の 3 つに分けた（どれも code なし）。
- `docs/api/script-host/host-v1.schema.json` への DTO の追加はしていない（領域の登録と一緒に hook unit が行う）。

## 未解決事項
- なし（この unit の範囲）。

## 提案
- close unit で付記 S4-9 の関数名を実装の名前（上記）に合わせる。
