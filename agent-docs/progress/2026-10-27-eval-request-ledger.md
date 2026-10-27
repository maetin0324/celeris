---
title: 論理要求の台帳を export する（eval requests）
tasks: [01M5XXF7QCZ1WZDS0FDH82YC39]
status: done
updated: 2026-10-27
---
# 論理要求の台帳を export する（eval requests）

完了日: 2026-10-27

## 変更
- `phase 1: 論理要求の台帳を celerisctl eval requests で export する` — ADR
  [2026-10-27-eval-request-ledger](../adr/2026-10-27-eval-request-ledger.md) を足した。`SqliteStore::request_ledger`
  （`crates/task-core/src/store/request_ledger.rs`、出力の型は `crates/task-core/src/request_ledger.rs`）と
  `celerisctl eval requests --since --until [--out]`（`crates/celerisctl/src/commands/eval.rs`）を足した。
  読み取りだけ。migration・API の変更は無い。
  - 1 行 = `(source_kind, source_key)`。差戻しで key が変わった decision・approval の item は、覆された operation の
    `updated_at` と同じ時刻・同じ thread・同じ source_kind の item として元の要求に寄せる（ADR D3）。
  - 担当の再起動（`cos_triage_requeue_run`）で消える `run_id` は、一次対応の入力 message（`item_ids=[…]`）から全 run を引く。
  - 主体は A3 の `InterventionActorKind::classify`（A2 の `by` と thread の種類）。不明は `null`。

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）
- `cargo test -p task-core --lib request_ledger` → 2 passed
  - `renotified_revised_and_returned_request_is_one_row_with_times_and_actors`: 再通知 3 回・担当の再起動 1 回・
    差戻し 1 回（key が decision-D1 → decision-D2）・改訂 1 回の要求が 1 行。初回 AI 応答（R2 の first_output_at）、
    人の待ち（escalation 10:21 → 人の回答 10:30）、解決（human:alice）、再開（解決後の最初の run）、方針参照、campaign 対応を確かめる。
  - `renotified_notice_is_one_row_and_unknowns_stay_null`: notice の count 3 revision が 1 行、範囲外は出ない、不明は null。
- `cargo test -p celerisctl --test eval_requests` → 1 passed（CLI。再通知と再起動で 1 行、`--out`、DB の byte が変わらない、逆の範囲は非 0）
- `bash scripts/dev/test-affected.sh` → exit 0（Rust 6623 passed・21 skipped、web 98 files / 685 tests passed）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0

## 本番 DB の写しでの照合手順（人が実行する。本番 DB には書かない）
1. 読み取り専用の写しを作る: `sqlite3 ~/.local/celeris/celeris.sqlite3 ".backup '$TMPDIR/ledger.sqlite3'"`
   （本番 DB の場所は運用の設定に合わせる。`celerisctl db backup` でもよい）。
2. export: `celerisctl --db "$TMPDIR/ledger.sqlite3" eval requests --since 2000-01-01T00:00:00Z --until 2100-01-01T00:00:00Z --out "$TMPDIR/requests.json"`
3. 元表と照合する:
   - item 数: `sqlite3 "$TMPDIR/ledger.sqlite3" "SELECT COUNT(*) FROM cos_inbox_items"` と `jq .summary.items` が一致する。
   - 論理キー数: `SELECT COUNT(*) FROM (SELECT DISTINCT source_kind, source_key FROM cos_inbox_items)` から、差戻しで key が
     変わった数（`jq '[.requests[].keys | length - 1] | add'`）を引いた値が `jq .summary.requests` と一致する。
   - notice: `SELECT COUNT(*) , COUNT(DISTINCT source_key) FROM cos_inbox_items WHERE source_kind='notice'` と
     `jq '[.requests[] | select(.source_kind=="notice")] | length'`（起票時の値は 963 item / 56 キー）。
4. 写しを消す（`rm "$TMPDIR/ledger.sqlite3"`）。

本番の写しでの実行は、この run では行っていない（未確認）。

## 未解決
- 差戻しの寄せは同じ transaction の時刻の一致に頼る。operation の `updated_at` と item の `created_at` が同じ文字列で
  書かれる前提（`cos_operation_override_at`）が変わったら寄せが外れる（行が増える）。その場合は item に元 item の参照を
  持たせる migration を足す。
- notice の解決主体（既読）は events に主体が無いので常に null。
- 人の作業時間（human-ops /2）との結合はしていない（ADR の範囲外）。

## 提案
- `cos_operation_override_at` が作る item に元 item の id を列で持たせると、D3 の時刻一致の推定が要らなくなる。
