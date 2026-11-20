---
title: アーキ v4 H R3 cos — cos_chat の root_done.rs・triage.rs・fallback.rs の self.store・self.config を同じ読みの 1 回化で減らす
tasks: [01M7THYAPKESR1BG6BQZPJ749P]
status: done
updated: 2026-11-19
completed: 2026-11-19
---
# cos（WorkUnit）

完了日: 2026-11-19

## 0. 結論

- [ADR dispatcher-narrow-ops-round2](../../adr/2026-11-19-dispatcher-narrow-ops-round2.md) の表の「1 回化」の 3 つを、表のとおりに行った。
  3 file の self.store・self.config の出現は C3 で store 42 → 40（−2）・config 18 → 10（−8）、E1 で store 60 → 58（−2）・config 19 → 11（−8）。
  ADR の見込み（store −2・config −8）と同じ。
- 変えた file は `crates/task-dispatch/src/dispatcher/cos_chat/` の root_done.rs・triage.rs・fallback.rs と、この記録だけ。
  `structure_guard.rs`・`cex_*.rs`・試験の file・他の file は変えていない。
- 新しい file・型・trait・helper・`task_ops` の関数・method は足していない。足したのは同じ file の private 関数の値の引数
  （`policy: &str` を 4 つ、`summary_requested: bool` を 2 つ）と局所の変数（`policy`・`enabled` の移動・`summary_on`）だけ。
- 行数（`git diff --numstat`、3 file とも production）: root_done.rs 477 → 472（+17 −22）、triage.rs 760 → 760（+16 −16）、
  fallback.rs 709 → 709（+13 −13）。3 file の計で −5 行。どの 1 回化も行を増やしていない（ADR D5）。

## 1. 変えた参照と使った操作

行は基準（base.txt の commit。この WorkUnit の base と 3 file は同じ）の行番号。

| file・関数 | 基準の参照 | 変えた形 | 減り（C3 / E1） |
|---|---|---|---|
| root_done.rs `root_done_advance`（`Queued`） | store 296・306・341: `collect_facts(&self.store, row.task_id, probe)` を 3 つの分岐（Completed・Failed 系・live）で 1 回ずつ | `match` が「まとめの文」（Completed で出力があればその文、他は空）を返す式になり、待つ間は分岐の中で `return Ok(())`。分岐の後で `collect_facts` を 1 回（今の 335 行）読み、文が空なら `root_done_fallback`、空でなければ `root_done_notify_summary` | store −2 / −2 |
| triage.rs `triage_ingest` → `triage_reconcile`・`triage_events`・`triage_notices`・`triage_stagnation` | config 462・492・585・652: `self.config.triage.policy_version.clone()` を 4 つの private 関数が 1 回ずつ | `triage_ingest` の先頭（今の 371 行）で 1 回読み、4 つに `policy: &str` で渡す（呼び出し元は 4 つとも `triage_ingest` だけ。ADR D3）。中の `&policy` は `policy` にした | config −3 / −3 |
| fallback.rs `triage_fallback` | config 500（2）・510・553（2）: `self.config.enabled && self.config.unavailable_reason.is_none()` を 2 回、`self.config.enabled` を 1 回 | 先頭で `let enabled = self.config.enabled;`（510 から移した）と `let summary_on = enabled && self.config.unavailable_reason.is_none();` を 1 回読む（今の 500・503 行）。500 は `if summary_on`。553 の `if` は、`handed` へ入れる腕を `Ok(Some(report)) if summary_on` にして外した（`summary_on` が偽なら `handed` は空で、`queue_summary_request` は呼ばれない。前と同じ） | config −3 / −3 |
| fallback.rs `fallback_one` → `handoff_message` | config 641（2）: `summary_requested` を item ごとに設定から読み直す | `triage_fallback` の `summary_on` を `fallback_one` → `handoff_message` に `summary_requested: bool` で渡し、641 の読みを消した（呼び出し元は全部 fallback.rs の中。ADR D3）。641 の上の D4 のコメントは `summary_on` の定義の上へ移した | config −2 / −2 |

使った既存の操作: 置き換え先の store の操作は変えていない。`collect_facts`（root_done_probe.rs の既存の関数）、`unavailable_reason`・`handoff_text`
（fallback.rs の既存の関数）、`inbox_source`・`notice_source`・`sources_by_kind`・`sources_for_touched`（triage.rs の既存の関数）を、前と同じ引数の値で呼ぶ。
減りはすべて ADR D1 (b)（同じ流れの同じ読みを 1 回にまとめる）。(a)（既存の owner の操作への置き換え）で減る参照は ADR D2 のとおり無かった。

守ったこと（ADR D4）:

- root_done.rs: どの道筋も facts の読みは 1 回のまま。Completed の出力の読み（`chat_message_get`）と live の入力の取り消し（`chat_message_cancel`）は
  facts の読みの前のまま。待つ間（`overdue` が `None`）は facts を読まずに戻るのも前と同じ。書き込み（`root_done_fallback`・`root_done_notify_summary`）の分岐の条件も同じ。
- triage.rs: 設定の値は tick の中で変わらない（production の code に `config.triage.policy_version`・`config.enabled`・`config.unavailable_reason` への代入は無い）。`triage_stagnation` の早い return の前に読むのは設定の値だけで、store の読みの回数と順序は変わらない。
- fallback.rs: `retry_failed_summary_requests` → `candidates` → item ごとの `fallback_one` → `queue_summary_request` の順序と、`summary_on` が偽のときに
  summary を頼まない条件は同じ。`handoff_message` の system message は前と同じく `summary_on` に依らず書く。

## 2. 残した参照と理由

ADR の表の「残す」のとおり。主なもの:

- root_done.rs: `let store = self.store.clone()`（95・174。probe の closure のための既存の写し）、`New` の起点の thread・thread・task の thread・投稿・
  `root_done_advance`（別の読み書き）、`controller_enabled_for` の task と controller の状態（task ごとの読み。task-api の `goal_guard` は依存の向きで呼べない）、
  `web_base_url`（`root_done_notify_summary` と `root_done_fallback` は排他）、`root_done_finish` の project の読み・通知・状態の確定。
  `New` の `!self.config.enabled || self.config.unavailable_reason.is_some()` は file の中で 1 か所だけなので、tick の入口へ動かしても出現は減らない（移動になる）。
- triage.rs: `latest_event_id` の 2 回（`triage_reconcile` と `triage_events`）は、`triage_reconcile` の書き込みの後に `triage_events` が読み直すもので、1 回にすると読む時点が変わる。
  2 つの cursor（events・notices）は別の key。`cos_digest_shadow_reconcile(&self.store)`・`human_inbox*` は owner の操作 1 回。
- fallback.rs: `candidates(&self.config.db_path)`・`fallback_after_secs`（別の値）、`fallback_one` の 589 と 604 の解決（排他の分岐の書き込み）、
  `retry_failed_summary_requests`・`queue_summary_request` の `cos_general_thread_id`・`cos_general_thread_ensure`・`chat_system_queue_*`（読みと作成と書き込みが別の操作）。

## 3. 検査の結果（提出時の HEAD で実行）

| 検査 | command | 結果 |
|---|---|---|
| 減りと移動の無さ | `sh agent-docs/progress/2026-11-19-arch-v4-h-r3/measure.sh shrink crates/task-dispatch/src/dispatcher/cos_chat/root_done.rs crates/task-dispatch/src/dispatcher/cos_chat/triage.rs crates/task-dispatch/src/dispatcher/cos_chat/fallback.rs` | exit 0。`shrink: ok files 138 -> 138 paths 3 file store+config 79 -> 69`（非試験 file の集合は同じ、どの file も E1 が増えていない） |
| 作業ツリーの 4 scope | `sh agent-docs/progress/2026-11-19-arch-v4-h-r3/measure.sh rows .` を metrics.tsv の before と比べた | 変わった行だけ: request/C3 store 633 → 631・config 220 → 212、logical/C3 661 → 659・226 → 218、request/E1 759 → 757・227 → 219、logical/E1 791 → 789・233 → 225、crate/E2 store 683 → 681・config 240 → 232。`dispatcher_files`・`*_files`・SqliteStore の file 数は同じ |
| 領域の試験・反例 | `cargo nextest run -p task-dispatch -E 'test(/cos_chat\|cex_\|structure_guard/)'` | 277 tests run: 277 passed |
| 影響の試験 | `bash scripts/dev/test-affected.sh` | exit 0。repo-health verdict=pass（fail_grade=0 new=0 worsened=0）、nextest 3938 tests run: 3938 passed, 11 skipped（celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch）、doc test 全部 ok |
| lint | `cargo clippy --workspace -- -D warnings` | exit 0 |
| 書式 | `cargo fmt --all -- --check` | exit 0（差分なし） |

C12（production の行）は rev にしか当てられないので、`after` の WorkUnit が HEAD で測る。この WorkUnit の 3 file は計 −5 行（上の 0 節）。

## 4. 未解決事項

- 無し。`tree_units.rs` は WorkUnit `tree`、after の測定・ADR の付記は WorkUnit `after` が行う。
