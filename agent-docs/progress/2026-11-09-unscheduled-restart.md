---
title: 起動直後の task_unscheduled の誤検知（子や依存を待つ ready の task）
tasks: [01M6ZS10R7YCVNNE2A9EHJ68N1]
status: done
updated: 2026-11-09
---
# 起動直後の task_unscheduled の誤検知（子や依存を待つ ready の task）

完了日: 2026-11-09

ADR: `agent-docs/adr/2026-11-09-unscheduled-restart-hold.md`（実装より先に commit）。

## 原因（コードで確かめた）
- `Dispatcher::tick` は `check_liveness_and_stagnation` を `dispatch_ready` より前に呼ぶ。ready の task に付くメモリの
  待ち（M1〜M7・M9・M10。`DelegatedChildren`・`DependsOnIntegration` を含む）は `dispatch_ready` の中の
  `note_dispatch_wait` だけが書く。起動直後は `dispatch_waits` が空で `checked_at` が `None` なので、最初の tick の
  停滞の判定はこれらの task を「待ちの理由なし」と見て、閾値（1800 秒）を過ぎていれば `task_unscheduled` に出していた。
- `awaiting_children`・`awaiting_human` は `reviewing` の task にしか付かず、`task_unscheduled`（`ready` だけを見る）には
  効かない（ADR D3）。

## 変更
- `ADR: 起動直後の停滞の判定を dispatch の走査が 1 回終わるまで保留する（task_unscheduled の誤検知）` — 原因、
  案 (a) DB の事実から子の待ちを作る・(b) 判定の保留・(b') `task_unscheduled` だけ保留・(c) tick の順序の入れ替えの比較、
  採った (b) とその理由、メモリにしか無いほかの待ちの扱い。
- `停滞の判定を起動後の最初の dispatch の走査まで保留する（task_unscheduled の再起動直後の誤検知）`:
  - `task_ops::stagnation::scan_due(checked_at, scan_interval_secs, waits_known, now)`（`crates/task-ops/src/stagnation.rs`）。
    判定を走らせる時刻の判定をここに一本化した。
  - `StagnationWatch.waits_known`（`crates/task-dispatch/src/dispatcher/tree_units.rs`）。`check_stagnation` は
    `scan_due` が偽なら判定せず、`checked_at` も進めない。`set_stagnation`（閾値の差し替え）は `waits_known` を引き継ぐ。
- `停滞の保留: tick の dispatch の行を if に書き換え、dispatcher.rs の行を増やさない` — `tick` は `dispatch_ready` が
  `Ok` で返ったら `waits_known = true` にする（source-size の `dispatcher.rs` の上限 2402 行を越えないよう、
  `if … else { 0 }` を `if` に書き換えた。`report.dispatched` の既定は 0）。
- 試験:
  - `crates/task-dispatch/src/dispatcher/tests/unscheduled_restart.rs`（注入した時計 `test_now`。最後の出来事から 1 時間後に
    起動した daemon）:
    `unscheduled_restart_first_tick_skips_parent_waiting_for_children_but_reports_no_reason_task`（子を待つ親は最初の tick で
    出ず、メモリの待ちに `DelegatedChildren` が入る。待ちの理由の無い ready の task〈入力の揃わない知識整理 task〉は
    2 回目の tick〈+60 秒〉の判定で出る）、`unscheduled_restart_dependency_integration_wait_is_not_reported`（M10 の
    依存の統合待ちは 4 回の判定で一度も出ない）。どちらもメモリの待ちを使わない事実では出ること（旧い最初の判定が見た状態）を
    先に確かめる。
  - `crates/task-ops/src/stagnation/tests.rs` の `unscheduled_restart_scan_due_waits_for_memory_waits_then_keeps_the_interval`。
  - `cos_chat_triage_stagnation.rs` の fixture は、`set_accepting_new_work(false)` の前に 1 回 tick する（一度 active で
    走査した後に新しい run を起こさない instance）。保留の導入で、走査を一度もしない instance は判定しなくなったため。
  - `dep_integration.rs` の `setup` を `pub(super)` にして新しい試験から使う（中身は変えない）。

## 旧い経路の撤去
- `check_stagnation` の中の間隔の判定（`checked_at` と `scan_interval_secs` の比較）を消し、`scan_due` だけが決める。
- config・API の欄・画面は変えていない。消す互換分岐・flag は無い。

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）
- 修正を外した確認（`scan_due` に `waits_known` の代わりに `true` を渡した一時の作業ツリー。commit していない）:
  `cargo nextest run -p task-dispatch --no-tests=fail -E 'test(/unscheduled_restart_/)'` → 2 本とも失敗。最初の tick で
  `TaskUnscheduled` の finding が出た（`unscheduled_restart.rs` の最初の tick の assert）。戻して 15 本合格。
- `cargo nextest run -p task-dispatch -p task-ops --no-tests=fail -E 'test(/unscheduled_restart_|stagnation/)'` → exit 0、
  15 本合格。
- `bash scripts/dev/test-affected.sh` → exit 0。check-health は verdict=pass（new=0・worsened=0）、nextest は
  6453 本合格・23 本 skip（355.8 秒）、`script bundle SKIP: 0 件`、全体 12 分 9 秒。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- 上の 3 つは実装の最後の commit（`停滞の保留: tick の dispatch の行を if に…`）の作業ツリーで実行した。その後の commit は
  この進捗ファイルだけ。

## 本番への移行
- 本番 config の変更は要らない（`[maintenance.stagnation]` の欄は変えていない）。release を promote するだけでよい。

## promote の後に operator が見る値
- promote の直後（最初の 1〜2 分）に、`cos_inbox_items` に `source_kind = 'task_unscheduled'` の行が新しく出ないこと。
  例: `SELECT source_key, created_at FROM cos_inbox_items WHERE source_kind='task_unscheduled' AND created_at >= '<promote 時刻>';`
  が 0 行。
- 出た場合は `GET /api/v1/tasks/{id}` の `waits` を見る。空なら正しい検知（本当に待ちの理由が無い ready）。
  `delegated_children` や `depends_on_integration` が入っていれば、この修正が効いていない。
- promote の直後の 1 tick の間は daemon の snapshot の停滞の欄が「分からない」になる（受信箱の停滞 4 種の行は保留され、
  閉じない）。2 回目の tick から今までどおり判定する。

## 未解決
- 起動直後から disk critical が続くと、`dispatch_ready` が走らないので停滞の判定は disk が戻るまで保留される（ADR D3）。
  その間も受信箱の停滞の行は保留されるだけで消えない。
- web の待ち表示は daemon の snapshot を読む。起動直後の最初の tick の前（数秒）はメモリの待ちが表示されない。今回は変えていない。

## 提案
- 子の待ち（M7）を DB の事実から作る案 (a) は、`awaited_children` が CoS chat の store を読むため、API の待ちの組み立てに
  chat の store を渡す変更が要る。web の待ち表示を再起動に強くしたいなら、別 task で dispatch の門と DB の待ちの正本を
  一本化する設計から始めるのがよい。
