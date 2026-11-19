# ADR 2026-11-19: Dispatcher の self.store・self.config の出現を既存の owner の操作と同じ読みの 1 回化で純減させる（アーキ v4 H R3）

- 状態: Accepted（本文は設計。この ADR を書いた WorkUnit `adr` は `crates/`・`scripts/`・`web/` を変えていない。実装と実測は後の WorkUnit `cos`・`tree`・`verdict`・`after` が行い、`after` が末尾に付記する）
- task: 01M7THYAPKESR1BG6BQZPJ749P（アーキ v4 H 01M7MZTKBPZPQE4XN451F8X84F の子、人の決定 unmet b）
- 進捗: [2026-11-19-arch-v4-h-r3.md](../progress/2026-11-19-arch-v4-h-r3.md)。この WorkUnit の記録: [adr.md](../progress/2026-11-19-arch-v4-h-r3/adr.md)
- 材料: [H の final-report.md](../progress/2026-11-19-arch-v4-h/final-report.md) の「提案」の 3 と (2) の節、
  [ADR 2026-11-18 dispatcher-store-access-pilot](2026-11-18-dispatcher-store-access-pilot.md)（E）、
  [E の進捗](../progress/2026-11-18-arch-v4-e.md)、[E の adr.md](../progress/2026-11-18-arch-v4-e/adr.md)、
  [E の measure-commands.md](../progress/2026-11-18-arch-v4-e/measure-commands.md)（C3・E1・E2 の snippet）
- 関係: [ADR 2026-11-11 dispatcher-state-ownership](2026-11-11-dispatcher-state-ownership.md)（D2・D3）、ADR-0082（dispatcher.rs の module map）

## 背景

H は E の減り（self.store を書く file −6、self.config −4）を、出現数が 633 → 633 のままの移動として成果に数えなかった
（final-report.md (2)。参照を対象 file から入口の file へ同じ数だけ移した）。R3 の成果は **出現数の純減** である。file の数の減りは求めない。

基準（before）は R3 の開始時の `git merge-base HEAD main`（件名 `integrate wu/close (phase close)`、
`1680de5cd1b4c57d5df3ceeab59d03a3783a0f6c`。measure の WorkUnit `tool` が base.txt に書く sha と同じ）。after は HEAD。
基準の tree で E の C3 を当てた値は、依頼 scope の非試験 69 file、self.store 38 file / 633、self.config 31 file / 220
（H の after と同じ。基準と この ADR の HEAD の間に `crates/`・`scripts/`・`web/` の差分は無い）。

対象 4 file の基準の値（C3 は 1 行の中の字面、E1 は改行をまたぐ `self\n.store` も数える。どれも依頼 scope と論理 scope の両方に入る）:

| file（行数） | C3 store | C3 config | E1 store | E1 config |
|---|---|---|---|---|
| `dispatcher/tree_units.rs`（1353） | 56 | 19 | 63 | 20 |
| `dispatcher/cos_chat/root_done.rs`（477） | 16 | 5 | 22 | 6 |
| `dispatcher/cos_chat/triage.rs`（760） | 13 | 4 | 24 | 4 |
| `dispatcher/cos_chat/fallback.rs`（709） | 13 | 9 | 14 | 9 |
| 参考: `dispatcher/review_verdict.rs`（869） | 32 | 3 | 33 | 3 |
| 参考: `dispatcher/work_units.rs`（2053） | 58 | 14 | 65 | 15 |

## 決定の要約

- D1 R3 が出現を減らす手は 2 つだけ。
  (a) 既存の owner の操作（基準にある `task_ops` の関数・`ExecutionCoordination` の method・同じ file の既存の method）への置き換え。
  1 回の呼び出しが 2 つ以上の参照を置き換えるときだけ出現が減る。
  (b) 同じ流れの同じ読みを 1 回にまとめる。排他の分岐に並ぶ同じ読みを分岐の後の 1 か所にする、同じ呼び出しの中で変わらない設定の値を
  1 回読んで同じ file の private 関数に値（`bool`・`&str`）で渡す、の 2 つの形に限る。
  どちらにも当たらない参照は残し、表に理由を書く。
- D2 棚卸しの結果、(a) で出現が減る参照は 4 file に無かった。対象の参照はほぼ全部が既に `task_ops` の関数の呼び出しか store の 1 操作で、
  既存の操作に置き換えても 1 対 1 で数が変わらない。候補として調べたもの（`tree_plan_checks` と `task_ops::delegate::plan_children`、
  `raise_node_replan_limit` の未回答の確認と `task_ops::tree::open_decision`、`controller_enabled_for` と task-api の `goal_guard`）は、
  入力か意味が違う（下の表の「残す」）。R3 の減りは (b) だけで出す。
- D3 呼び出し元から見た method の名と引数を変えない。他の file から呼ばれる method（`pub(super)`・`pub(crate)` で、呼び出し元が別の file にあるもの）の
  引数は変えない。値を渡すために引数を足してよいのは、呼び出し元が全部同じ file にある private 関数（`triage.rs` の `triage_reconcile`・
  `triage_events`・`triage_notices`・`triage_stagnation`、`fallback.rs` の `fallback_one`・`handoff_message`）だけ。
- D4 transaction の範囲・event・lease の意味、store への書き込みの回数・順序・失敗時の扱い（`?`・warn だけ・`let _ =`）を変えない。
  1 回化は、どの実行の道筋でも store の読みの回数と、読みと書き込みの前後を変えない（読みを書き込みの前へ動かさない）。
  減ってよいのは同じ呼び出しの中の設定の値の読みだけ。
- D5 新しい file・型・trait・Manager・Port・Facade・`task_ops` の関数・method を足さない。同じ file の新しい private の helper も R3 では足さない
  （参照を helper の中へ集めた減りが移動か純減かの境が曖昧になるので。下の「提案」）。production の行（A の C12 の `views.product_only.lines`）を増やさない。
  1 つの変更で行が増えるなら、その変更は行わない。
- D6 `review_verdict.rs` と `work_units.rs` は R3 で実装しない（「review_verdict.rs と work_units.rs の設計と決定」）。WorkUnit `verdict` はこの 2 file を変えない。

## 4 file の参照の表

列: 関数、行（基準の行番号。`*` は C3 に入らず E1 だけが数える改行をまたぐ参照）、読み書きの中身、扱い、減る見込み（C3 / E1）。
扱いの「残す」は理由、「1 回化」は形と、守る条件を書く。減る見込みが「0」の行は残す行。

### tree_units.rs（store 56・config 19、E1 63・20）

| 関数 | 行 | 読み書きの中身 | 扱い | 減る見込み |
|---|---|---|---|---|
| `tree_plan_gate` | store 24、config 28 | `task_ops::tree_plan::unit_gate_plan(store, …, limits, …)`（人の計画の `PUT` と同じ関数） | 残す。既に owner の操作 1 回で、limits はその入力 | 0 |
| `apply_tree_plan_holds` | store 48・50・61 | 採用後の unit を読む → `task_ops::tree_plan::plan_hold_writes` → `work_units_apply`（1 transaction の書き込み） | 残す。読み・owner の計算・1 回の書き込みが別の操作 | 0 |
| `check_tree_liveness` | store 88・92・100*・114・130・173・179・205、config 77 | ready の一覧・計画の有無・ready の候補・`node_liveness_facts`・節点ごとの events・`decision_path`・`StallDetected` の追記・通知 | 残す。92 の計画の読みは `is_tree_node` と重なるが、1 回にすると `is_tree_node` の判定を複製する（規約 2）。130・173 は節点ごとの読み、179・205 は書き込み | 0 |
| `check_stagnation` | store 307、config 308 | `task_ops::stagnation::gather(store, releases_dir, …)` | 残す。owner の操作 1 回 | 0 |
| `root_plan_approval` | store 334、config 328 | 木の有効の確認、`task_ops::plan_gate::approval_facts` | 残す。328 の `enabled` と後の `effective_tree_limits` の limits は別の値 | 0 |
| `open_plan_invalid` | store 346* | 木の決定の一覧から開いている `PlanInvalid` を探す（`planner_flow.rs` から呼ばれる） | 残す。`last_plan_invalid`（最後の 1 件、状態を問わない）と意味が違う | 0 |
| `worker_decisions` | store 385・397・447、config 369 | unit の一覧（WU のときだけ）・決定の一覧・`decision_path` | 残す。3 つは別の読み。呼び出し元は `worker_finish.rs`（高 risk）で引数を変えない | 0 |
| `decision_self_hold` | store 536、config 532 | 木の有効の確認、決定の一覧 | 残す。`dispatch_run.rs` の `apply_plan_invalid_atomic` の後に同じ一覧を読むが、まとめると `dispatch_run.rs` から見た引数が変わる（D3） | 0 |
| `last_plan_invalid` | store 552* | 決定の一覧から最後の `PlanInvalid` | 残す（上と同じ） | 0 |
| `apply_plan_invalid_atomic` | store 573・601、config 563 | 計画の有無の確認、`update_task`（`ExecutionGated`） | 残す。601 は書き込み | 0 |
| `effective_tree_limits` | store 615、config 614 | `task_ops::decision::limit_allowances(store, root, None)` と木の上限 | 残す。owner の操作 1 回 | 0 |
| `replan_limits` | store 631 | events を読み `ReplanLimits` を作る（6 か所から呼ばれる） | 残す | 0 |
| `effective_max_replans` | store 674・679、config 670・671 | task を読み直し、`limit_allowances(store, root, Some(task))`。config は `max_replans` と木の有効（別の値） | 残す。`replan_limits` の呼び出し元が task id しか持たない | 0 |
| `raise_node_replan_limit` | store 700・708*・719・734、config 697 | 木の有効・task・同じ節点の未回答の上限の決定・`decision_path`・`DecisionRequested` の追記 | 残す。`work_units.rs` の `replan_gate` が確認なしで呼ぶので、中の確認は外せない。`task_ops::tree::open_decision` は節点（task id）で絞らないので置き換えられない | 0 |
| `replan_exhausted_ask` | config 759 | 木の有効の確認 | 残す | 0 |
| `is_tree_node` | store 783* | 計画の有無（`task.tree` が無いとき） | 残す | 0 |
| `tree_limit_hold_for` | store 807・818・823・842、config 802 | `tree_counters`・`open_decision`・`decision_path`・上限の決定の追記。呼び出し元に `health_review/design_gate.rs`（高 risk）がある | 残す。別の owner の操作 3 つと書き込み 1 つ | 0 |
| `tree_plan_checks` | store 861・865・877、config 870・871 | `check_task_unit_repos`・仮の子の `build_child_task`（roles・genres）・`cross_department_questions` | 残す。`task_ops::delegate::plan_children` は delegate の spec を受け、unit の spec を受けない（入力が違う） | 0 |
| `reconcile_tree_units` | store 902・903・916・925・929・942・951・989・**996・998・1001**・1056・1068・1081*・1121*・1134・1146、config 1013*・1073・1074 | 開いた task unit を持つ task の一覧・親・計画・events（人の gate）・unit・子・`child_failure`・鏡の `work_units_apply`・**鏡の後の unit の読み直し**・`answered_decisions`・`build_child_task`・子ごとの events・`tree_child_create`・`work_unit_transition`・追記 | **996・998・1001 を 1 回化**: `changed` の後の 3 つの読み（hold あり: 1 回、hold なし: 読む → `promote_newly_ready` → 読む）を、929 の `units` を `let mut` にして `if changed { units = 読む; if hold.is_none() { promote; units = 読む; } }` にする。道筋ごとの読みの回数（1・2・0）と `promote_newly_ready` との前後は同じ。ほかは残す（1081* は子を作る書き込みの後の読み、925 は鏡の書き込みの前の読みで、1 回にすると読む時点が変わる） | store −1 / −1 |
| `handle_child_infra_failure` | store 1177・1183・1192・1240・1269・1286、config 1197・1198 | events・`answered_decisions`・`build_child_task`・子の作り直し・unit の書き込み・通知 | 残す。`reconcile_tree_units` の 1056〜1074 と同じ並びだが、まとめるには新しい helper が要る（D5。「提案」） | 0 |
| `close_open_task_units`・`cancel_units` | store 1301・1334 | unit の読み・書き込み | 残す | 0 |

### cos_chat/root_done.rs（store 16・config 5、E1 22・6）

この file の `self` は `CosChatLaunch`。store は `Dispatcher` と同じ SQLite の `Arc`。

| 関数 | 行 | 読み書きの中身 | 扱い | 減る見込み |
|---|---|---|---|---|
| `root_done_ingest`・`root_done_advance_all` | store 95・174 | `let store = self.store.clone()`（probe の closure が `&mut self` と並んで store を使うための既存の写し） | 残す。消すと closure ごとに参照が増える | 0 |
| `root_done_advance`（`New`） | store 210・224*・229・235*・249*・264、config 214（2）・247 | 届けの待ちの facts・CoS が使えるかの確認・起点の thread・thread・task の thread・入力の投稿・`root_done_advance`（New → Queued） | 残す。thread の 3 つの読みは「起点の thread が有ればそれ、無ければ task の thread を作る」の順で、同じ並びの owner の操作は無い | 0 |
| `root_done_advance`（`Queued`） | store 281*・289*・**296・306**・319・339・**341**、config 332 | run の一覧・出力の message・**facts（3 つの分岐に 1 回ずつ）**・入力の message・入力の取り消し | **296・306・341 を 1 回化**: 分岐（Completed・Failed 系・live）は「まとめの文」（Completed で出力が有ればその文、他は空）を返すか、待つ間は `return Ok(())` する。分岐の後で facts を 1 回読み、文が空なら `root_done_fallback`、空でなければ `root_done_notify_summary`。どの道筋も facts の読みは 1 回で、Completed の出力の読み・live の取り消しは facts の前のまま | store −2 / −2 |
| `controller_enabled_for` | store 353・360 | task を読み、案件の controller の状態 | 残す。task-api の `goal_guard` は task-dispatch から呼べず（依存の向き）、意味（Enabled か / Enabled でないか）も違う | 0 |
| `thread_label_prefix` | store 366 | thread の label | 残す。呼ぶ 2 か所（まとめ・fallback）は排他 | 0 |
| `root_done_notify_summary`・`root_done_fallback` | store 429、config 384*・417 | web の URL（2 か所は排他）・system message の追記 | 残す | 0 |
| `root_done_finish` | store 458*・463・472 | project のための task の読み・通知・`root_done_advance` | 残す。`RootDoneFacts` は project を持たない | 0 |

### cos_chat/triage.rs（store 13・config 4、E1 24・4）

| 関数 | 行 | 読み書きの中身 | 扱い | 減る見込み |
|---|---|---|---|---|
| `triage_ingest` | store 395 | `cos_digest_shadow_reconcile(&self.store)` | 残す | 0 |
| `human_inbox_touching`・`human_inbox` | store 422・438 | `task_ops::human_inbox::{human_inbox_touching, human_inbox}` | 残す。owner の操作 1 回 | 0 |
| `triage_stagnation`・`triage_reconcile`・`triage_events`・`triage_notices` | **config 462・492・585・652**（`self.config.triage.policy_version.clone()`） | 4 つとも `triage_ingest` だけから同じ tick に呼ばれ、同じ設定の値を 1 回ずつ読む | **1 回化**: `triage_ingest` の先頭で 1 回読み、4 つの private 関数に `policy: &str` で渡す（D3。呼び出し元は全部 `triage_ingest`）。設定は tick の中で変わらない。`triage_notices` の signature は 1 行の 100 字に収める | config −3 / −3 |
| `triage_stagnation` | store 472* | `cos_triage_reconcile`（種類ごとの書き込み） | 残す | 0 |
| `triage_reconcile` | store 494・504・510*・515*・521・525*・533*・549 | event の先頭・資格待ちの解決・種類ごとの reconcile・2 つの cursor の確認と初期化・通知の最新 | 残す。2 つの cursor は別の key。`triage_events` の cursor と先頭の読みは、この関数の書き込みの後の読みで、1 回にすると読む時点が変わる | 0 |
| `direct_escalate` | store 570* | 直接の通知の書き込み | 残す | 0 |
| `triage_events` | store 586*・598・602・635・640 | cursor・event の先頭・`let store = self.store.clone()`（既存の写し）・触れた task の解決・取り込み | 残す | 0 |
| `triage_notices` | store 653*・665*・694 | cursor・通知の page・取り込み | 残す | 0 |
| `triage_launch`・`triage_claim` | store 711*・747*・755 | 待ちのある thread・claim・claim した run の読み | 残す。claim と run を 1 回で返す store の操作は無い | 0 |

### cos_chat/fallback.rs（store 13・config 9、E1 14・9）

| 関数 | 行 | 読み書きの中身 | 扱い | 減る見込み |
|---|---|---|---|---|
| `fallback_inbox_item` | store 460・462 | `task_ops::human_inbox::item_task_id`・`question_for_task` | 残す。2 つを 1 回で行う `task_ops` の関数は無い | 0 |
| `triage_fallback` | store 535、**config 500（2）・503・510・511・553（2）** | `enabled && unavailable_reason.is_none()` を 500 と 553 で 2 回、`enabled` を 510 で もう 1 回、db の path・待ちの秒数 | **1 回化**: 先頭で `let enabled = self.config.enabled;` と `let summary_on = enabled && self.config.unavailable_reason.is_none();` を 1 回読み、500・553 は `summary_on`、510 の `let enabled` は消す。503・511 は残す。`fallback_one` は設定を変えない | config −3 / −3 |
| `fallback_one` | store 574・589・595*・604 | 通知の開きの確認・解決（source closed）・outbox の claim・解決（fallback） | 残す。589 と 604 は排他の分岐の書き込みで、1 か所にすると分岐の flag が増え行も増える | 0 |
| `handoff_message` | store 630・643、**config 641（2）** | item の報告・hand-off の system message。`summary_requested` を item ごとに設定から読み直す | **1 回化**: `triage_fallback` の `summary_on` を `fallback_one` → `handoff_message` に `bool` で渡し、641 の読みを消す（D3。呼び出し元は全部 `fallback.rs` の中） | config −2 / −2 |
| `notice_source` | store 613 | 通知の文の読み | 残す | 0 |
| `retry_failed_summary_requests`・`queue_summary_request` | store 661・669・692・700 | 一般の thread の読み・作成と system queue の書き込み | 残す | 0 |

### file ごとの減る見込み

| file | C3 store | C3 config | E1 store | E1 config |
|---|---|---|---|---|
| `tree_units.rs` | 56 → 55（−1） | 19 → 19 | 63 → 62（−1） | 20 → 20 |
| `cos_chat/root_done.rs` | 16 → 14（−2） | 5 → 5 | 22 → 20（−2） | 6 → 6 |
| `cos_chat/triage.rs` | 13 → 13 | 4 → 1（−3） | 24 → 24 | 4 → 1（−3） |
| `cos_chat/fallback.rs` | 13 → 13 | 9 → 4（−5） | 14 → 14 | 9 → 4（−5） |
| 計（4 file） | −3 | −8 | −3 | −8 |

4 file は依頼 scope と論理 scope の両方に入るので、見込みは 4 scope とも同じ差: request/C3 store 633 → 630・config 220 → 212、
request/E1 759 → 756・227 → 219、logical/C3 661 → 658・226 → 218、logical/E1 791 → 788・233 → 225（基準の値は `tool` の before で確かめる）。
どの file も参照が 0 にならないので、`dispatcher_store_files`・`dispatcher_config_files` と分母（69・76）は変わらない見込み。

見込みは小さい。対象の参照の大部分は既に owner の 1 操作で、D1〜D5 の範囲で純減できるのはこの 11 出現だけと判断した。
1 回化のどれかが D4 か行の上限（D5）で行えなければ、その分を減らさずに進捗に理由を書く。数を合わせるために別の file を変えない。
4 file の計で store・config の両方が 1 以上減らなければ、`after` は check を落として replan を求める。

## 数え方と数えないもの

- 主の数: E の measure-commands.md の C3（依頼 scope = dispatcher/ の非試験 69 file、論理 scope = 69 − `project_decider_ts.rs` ＋ `#[path]` の 8 file = 76 file）。
  補助: E1（改行をまたぐ参照も数える）、E2（crate 全体 `crates/task-dispatch/src` の非試験 file の self.store・self.config・SqliteStore）。
  metric は `dispatcher_files`・`dispatcher_store_files`・`dispatcher_store_hits`・`dispatcher_config_files`・`dispatcher_config_hits` を
  request/C3・request/E1・logical/C3・logical/E1 の 4 scope で、H の struct-store.tsv と同じ key・scope・checker（`a-c3`・`e-e1`）の文字列で書く。
  before は `tool` の base.txt の sha（開始時の `git merge-base HEAD main`）、after は HEAD。
- 成果に数える条件（全部を満たすこと）:
  1. request/C3 で `dispatcher_store_hits` と `dispatcher_config_hits` が減り、E1 でも同じだけ減る（改行の入れ方で数を動かしていない）。
  2. E2 の crate 全体の self.store・self.config の出現が dispatcher/ と同じだけ減る（dispatcher/ の外へ移していない）。SqliteStore を書く file が増えない。
  3. 4 file 以外の dispatcher/ の file の出現が 1 つも増えない（file ごとの before・after の差で見る）。
  4. 分母（依頼 69・論理 76・crate の非試験 file）が増えない。
  5. production の行（C12 の `views.product_only.lines`、分類規則は base に固定）が増えない。
- 数えないもの（E の ADR と同じ。H の objective の共通の規則）:
  - 参照を別の file・別の field・別の crate へ移しただけの減り（`task_ops` などへ新しい関数を足して呼ぶことを含む）。既にある `task_ops` の関数を呼ぶのは移動ではない。
  - getter・別名（`let s = &self.store;`・`let cfg = &self.config;`・`let t = &self.config.triage;` のような struct や部分の struct への参照・
    `let Self { store, .. } = self;`・`fn store(&self)`・`&self.store` を引数で渡す形）。D1 (b) で許すのは、葉の値（`bool`・`u64`・`String`・`&str`）を 1 回読んで使い回すことだけ。
  - field の梱包（store と config を新しい struct に包む）、新しい Manager・trait・Port・Facade、新しい `task_ops` の関数。
  - 分母の増加（file を足す）、file の併合・削除、改行で 1 行の字面を割ること（E1 で見る）。
  - 既存の写し（`root_done.rs` 95・174、`triage.rs` 602 の `let store = self.store.clone()`）を増やすこと。R3 は写しを足さない。

## review_verdict.rs と work_units.rs の設計と決定

### review_verdict.rs（store 32・config 3）

順序（基準の `on_review_finished`）: (1) supervisor の review の entry を外す（review lock）→ (2) 部長の session の記録 →
(3) runs 行の確認（120 行 `run_index_get`: 完了した reviewer の run の行が `Running` でなければ遅れた結果として捨て、quota の event だけ残す）→
(4) 計画の開き直しの確認（130 行 `work_units_for` と `next_work_unit`。review の間に計画が開き直されたら判定を捨て、`ReviewPlanReopened` を当てる）→
(5) 判定の反映（delivery の読み書き・`apply_transition_with_events`）。(3) と (4) の前後が stale review・restart・replan の反例の本体である
（`cex_restart_old_reviewer_pass_after_ownerless_close_is_ignored`・`cex_late_review_pass_after_replan_is_dropped`・
`cex_late_review_pass_after_repair_is_dropped`・`cex_review_late_pass_on_the_old_candidate_does_not_pass_the_rebased_candidate`）。

設計: 1 回化の候補は 257・263 行の `events_for`（reviewer の基盤の失敗と再投入の数。排他の 2 分岐の同じ読み）の 1 つだけで、store −1。
ほかの 30 の参照は、別の読み・別の書き込み（`append_event` 9・`apply_transition`・`apply_transition_with_events` 3・delivery 2）で、
1 回にまとめると transaction の範囲か書き込みの順序が変わる（D4）。読みを判断の純粋関数（`review_decision::judge_review`）と入口に分ける形は、
出現を移すだけで減らさない（H が数えなかった形）。

決定: **R3 では実装しない**。理由: (1) 純減の見込みが store −1 だけで、(3)・(4) の順序を守る反例 4 本の本体の関数に触れる risk に見合わない。
(2) WorkUnit `verdict` が並行の `cos`・`tree` と同じ段に入り、review の順序の変更を同じ統合で見分けにくい。
次の task で実装する条件: `on_review_finished` の (1)〜(5) の順序を固定する試験（反例 4 本に加え、(3) と (4) の前後を入れ替えると落ちる試験）が先にあること、
1 回化で減る出現が 3 以上あること、変える行が 257〜270 行の分岐の中に収まること。

### work_units.rs（store 58・config 14、本体 2053 行）

本体の行が閾値 2000 を超えているので、repo health の `size.responsibility-growth` と source-size が 1 行の増加でも落ちる。WorkUnit の遷移
（`wu_dispatch_gate` の 11 参照・`deferred_work_unit_trigger`・`reconcile_parallel_tasks` など）と commit の副作用（`spawn_work_unit_checks` の 7・
`record_check_hand_off`・`take_over_handed_off_checks`・`prepare_work_unit_workspace`）が同じ file に並ぶ。

設計（次の task の形）: 遷移は「store から読んだ unit の行と events → 次の遷移と event の列」の純粋関数にし、commit の副作用（git・checks の起動・
hand-off の書き込み）は今の入口の method が 1 transaction の書き込みの後に行う（ADR dispatcher-state-ownership D3）。分けると新しい file が要り
（分母が増える。成果に数えない）、200 行を超える関数の分割が先に要る（同 ADR D2 の 5）。lease・restart・並列 WU の反例
（`cex_two_parallel_work_unit_runs_finishing_in_one_tick_lose_no_update`・`cex_run_finish_and_work_unit_check_finish_in_one_tick`・
`cex_restart_after_crash_between_wu_and_task_transitions_is_idempotent`・`cex_restart_after_wu_done_promotes_dependents_once`）の本体に直接触れる。

決定: **R3 では実装しない**。理由: (1) 遷移と副作用の分離は file の追加（分母の増加）か行の増加を伴い、R3 の数え方では成果にならない。
(2) 行を増やせない file で、1 回化の候補（同じ関数の中の同じ読み）を探して行を減らす作業は、遷移の分離の設計と別の task にした方が審査の範囲が狭い。
次の task で実装する条件: 遷移の純粋関数の置き場（既存の `task_core` の WorkUnit の module か `ExecutionCoordination`）を ADR で決めること、
`work_units.rs` の本体の行を減らす方向だけで変えること、上の反例 4 本と `structure_guard_*` が通ること、分母を増やす分は成果に数えないと先に書くこと。

## 反例試験と領域の試験

既存の反例試験を弱めず、名前を変えずに通す（E の ADR の 12 本。すべて `crates/task-dispatch/src/dispatcher/tests/` の下）。

| 範囲 | 試験 |
|---|---|
| lease | `cex_lease_restart.rs`: `cex_stale_holder_late_finish_does_not_overwrite_the_reassigned_run`、`cex_lease_renew_after_expired_snapshot_keeps_the_holder` |
| restart | `cex_lease_restart.rs`: `cex_draining_instance_reclaims_its_expired_lease_without_redispatching`、`cex_restart_after_crash_between_wu_and_task_transitions_is_idempotent`、`cex_restart_after_wu_done_promotes_dependents_once` |
| stale review | `cex_lease_restart.rs`: `cex_restart_old_reviewer_pass_after_ownerless_close_is_ignored`。`cex_review_authority.rs`: `cex_review_late_pass_on_the_old_candidate_does_not_pass_the_rebased_candidate` |
| 並列 WU | `cex_lease_restart.rs`: `cex_two_parallel_work_unit_runs_finishing_in_one_tick_lose_no_update`、`cex_run_finish_and_work_unit_check_finish_in_one_tick` |
| replan | `cex_late_completion.rs`: `cex_late_review_and_integration_after_replan_are_dropped`、`cex_late_review_pass_after_replan_is_dropped`、`cex_late_review_pass_after_repair_is_dropped` |

12 本がこの ADR を書いた時点の HEAD にあることを `git grep -l "fn <名>\b" -- crates/task-dispatch` で確かめた（adr.md）。

領域の試験（どれも `bash scripts/dev/test-affected.sh` が task-dispatch の変更で走らせる。全体の試験は release gate だけ）:

- `tree_units.rs`: `tree_decisions.rs`・`tree_gate.rs`・`tree_*` の試験（子 task の鏡・人の gate・`promote_newly_ready`・子の作り直し）、`structure_guard_*`。
- `root_done.rs`: `cos_chat_root_done.rs`（New → Queued、Completed のまとめ、空の出力・Failed・期限切れの fallback、入力の取り消し）。
- `triage.rs`: `cos_chat_triage.rs`・`cos_chat_triage_inbox_thread.rs`・`cos_chat_triage_stagnation.rs`・`cos_chat_triage_digest_script_compare.rs`。
- `fallback.rs`: `cos_chat_triage_fallback.rs`（`cos_chat_triage_fallback_quota_or_login_unavailable` を含む）・`cos_chat_triage_fallback_authorization.rs`・`cos_chat_triage_fallback_timing.rs`。
- 構造の guard: `structure_guard.rs` の `store_access_guard_work_units`・`store_access_guard_cos_chat`・`store_access_guard_detects_each_violation`（E の 6 file が 0 のまま）。

R3 は試験を足さない。1 回化は振る舞いを変えないので、守る不変条件（読みの回数・書き込みの順序・分岐ごとの結果）は上の既存の試験が検出する。
`after` が測定（C3・E1・E2・C12）で純減と移動の無さを確かめる。

## 撤去対象

- `triage.rs` の 4 つの `let policy = self.config.triage.policy_version.clone();`（`triage_ingest` の 1 つに置き換える）。
- `fallback.rs` の 553 行の `self.config.enabled && self.config.unavailable_reason.is_none()`、510 行の `let enabled = self.config.enabled;`（先頭へ）、
  641 行の `summary_requested` の読み。
- `root_done.rs` の `Queued` の 3 つの分岐の `collect_facts` の呼び出し（分岐の後の 1 つに置き換える）。
- `tree_units.rs` の `reconcile_tree_units` の 994〜1004 行の if の連鎖の重なった読み（996・998・1001 行。コメントは残す）。
- 旧新の並存・互換分岐・切替 flag は置かない。試験を消さない・弱めない（method の名を変えないので、既存の試験がそのまま新しい形を通る）。
- 文書: `docs/architecture-map.md` と ADR-0082 の module map は変えない（file の責務と入口は変わらない）。

## 実装前 brief（`cos`・`tree` が使う）

1. 根本原因: 同じ tick・同じ呼び出しの中で、同じ設定の値（`policy_version`・`enabled`/`unavailable_reason`）と同じ store の読み（facts・unit の一覧）を
   関数や分岐ごとに読み直している。症状は self.store・self.config の出現の多さ（依頼 scope 633・220）と、読みの所在の重なり。
2. 責務 owner: 4 file の各関数（`triage_ingest`・`triage_fallback`・`root_done_advance`・`reconcile_tree_units`）が自分の流れの読みを持つ。
   store の読み書きの owner は `SqliteStore` と `task_ops` の既存の関数のまま。
3. 既存の再利用先: `task_ops` の既存の関数（`collect_facts` は `root_done_probe.rs` の既存のもの）、上の領域の試験と反例 12 本、
   E の measure-commands.md の C3・E1・E2、A の C12。
4. 選択肢: (a) 採用: 同じ流れの同じ読みを 1 回にまとめ、同じ file の private 関数に値で渡す。(b) 不採用: 新しい helper・getter で参照を集める（移動。D5）。
   (c) 不採用: 既存の `task_ops` の関数への 1 対 1 の置き換え（数が減らない）。(d) 不採用: `review_verdict.rs`・`work_units.rs`（上の決定）。
   (e) 不採用: `tree_units.rs` の `build_child_task` の 2 か所を 1 つの helper にする（新しい helper。「提案」）。
5. 新しい状態・分岐・fallback・port: 無し。足すのは private 関数の値の引数（`policy: &str`・`summary_on: bool`）と局所の変数だけ。
6. 撤去対象: 上の「撤去対象」。
7. 不変条件: transaction の範囲・event・lease の意味、書き込みの回数・順序・失敗時の扱い、道筋ごとの store の読みの回数と書き込みとの前後
   （領域の試験と反例 12 本）。他の file から呼ばれる method の名と引数（コンパイル）。4 file 以外の出現・分母・production の行が増えない（`after` の測定）。
   高 risk の path（`worker_finish.rs`・`leases.rs`・`ownerless_runs.rs`・`health_review/`）と policy の file（`scripts/dev/repo-health/` ほか）を変えない（`git diff --name-only`）。

## 計画の WorkUnit への写し

- `cos`: `cos_chat/root_done.rs`・`triage.rs`・`fallback.rs` の表の「1 回化」だけを行う（見込み store −2・config −8）。他の file を変えない。
- `tree`: `tree_units.rs` の `reconcile_tree_units` の 1 回化だけを行う（見込み store −1）。
- `verdict`: D6 により `review_verdict.rs`・`work_units.rs` を変えない。
- `after`: 4 scope の 5 metric と E2・C12 を測り、「数え方と数えないもの」の条件 1〜5 を確かめ、この ADR に付記する。

## 提案（R3 の範囲の外）

- `tree_units.rs` の `reconcile_tree_units`（1056〜1074）と `handle_child_infra_failure`（1183〜1198）は、`answered_decisions` → `build_child_task`（roles・genres）の
  同じ並びを持つ（store 4・config 4）。同じ file に 1 つの private 関数を置けば store −2・config −2 になるが、新しい helper の減りを数えるかの規則
  （「同じ並びが 2 か所以上あり、helper の後の総数が減るなら純減に数える」など）を先に決める必要がある。
- `dispatch_run.rs` の `apply_plan_invalid_atomic` と `decision_self_hold` は同じ流れで同じ決定の一覧を読む。まとめるには `dispatch_run.rs` から見た引数を変える（R3 の D3 の外）。
