---
tasks: [01M59KXY854W5C65GTG9GVQVAC]
status: done
completed: 2026-10-19
---
# 多軸評価 2a/P0: 生記録の型・検証・冪等 JSONL 追記（WU core）

## 実装
- `crates/campaign/src/human_ops.rs`（lib.rs で公開）。`time` を dev-dependencies から dependencies へ移した（新規 crate なし）。
- 型: `HumanOpRecord`（定義書 §2〜3 の全欄を持つ 1 行 1 record、`deny_unknown_fields`、Option 欄の省略は形式エラー）、
  `TimingSpan`（端が None なら長さも None）、`AttemptRecord`、`DurationStat`（欠測があれば合計を出さない）、列挙型一式。
- 検証 `validate()`: schema・record_id（材料から再計算）・experiment_set×arm の組・`trial_key` と identity 欄の整合
  （`TrialKey::idempotency_key()` を再利用）・RFC 3339 UTC 時刻・end>=start（job も submit<=start<=end）・
  null の時刻には `missing` の理由必須・型ごとの必須/不使用欄・reason 必須の op_kind・actor にメール不可。
- `compute_record_id()`: 定義書 §2.3 の材料を `manifest::canonical` で並べた sha256（`hop-<hex>`）。recorded_at/host/note は材料外。
- `HumanOpLog::append`: 検証→既存と照合（同一本体は `Duplicate`、食い違いは `Conflict`）→1 write+fsync。
  末尾改行の無い file にも行を繋げない。壊れた file には追記しない。
- `read_records`: 壊れた行は `HumanOpError::Line{line,..}`（1 始まり）。`resolve`: correction を欄ごとに適用（訂正の訂正は最後の行が勝つ）。
  `op_spans`（片方だけの op も残す）、`redo_counts`（attempt 数−1）。

## 証拠
- `cargo test -p celeris-campaign --test human_ops`: 10 passed（重複/再開で行が増えない、missing が 0 にならない、A/B 同一入力の同一 record_id、不正入力の拒否、壊れ行の行番号）。
- `bash scripts/dev/test-parallel.sh`: exit 0（passed 5826 / failed 0）。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。

## 未解決事項
- 並行追記の check と write の間は排他していない（O_APPEND の 1 write）。同一 id の重複行が万一入っても、読み出し側が本体同一なら 1 件に潰し、食い違えば行番号付きで拒否する。
- `cargo clippy -p celeris-campaign --tests` は既存 tests/launcher.rs・env_lock.rs の redundant_closure で落ちる（本 WU 以前から。gate は `--workspace` で通る）。
- `wait_kind`・attempt の `result` の値域は定義書に列挙が無いため、wait_kind は非空文字列、result は ok/failed/cancelled/timeout/abandoned とした。

## 提案
- 定義書 §2.1 に `wait_kind` の値域と attempt `result` の列挙を付記する（runbook WU で）。
