# ADR 2026-11-05: 第 4 フェーズ P4-A — 残課題の契約（不変条件と試験の対応）・性能のベースライン・B/C の分担

- 状態: 採用。
- 根拠: `agent-docs/progress/2026-11-05-phase4/p4a.md`、raw は `agent-docs/progress/2026-11-05-phase4/p4a/`。前提は [第 3 フェーズの達成の不変条件](2026-11-05-phase3-achievement-invariants.md)、[P1-I](2026-11-05-phase3-p1i.md)。

## 文脈
第 3 フェーズ（`2026-11-05-phase3-achievement-invariants.md` の I1〜I14・K1〜K43、P0-B〜P2-J）の後に 7 件の残課題がある。
第 4 フェーズの子 B〜H（親 01M6PAK8E8RH3ZMQC0FAD8ZNAA）が着手する前に、残課題ごとの所在（file・関数・query）・
守る不変条件と既存の試験・新しく足す試験・数値のベースラインと再現の command を決める。B と C は A に依存し、互いには
依存しない。

## 決定

### D1 残課題の所在と担当
| # | 残課題 | 所在（実 file・関数・query） | 担当 |
|---|---|---|---|
| G1 | 達成の後に根拠が reopen されても achieved のまま | `crates/task-dispatch/src/dispatcher/project_guard.rs` の `pre_decide` 1 段目（`state.phase == Achieved && !human` なら `achieved-quiet` で判断しない）。decide〜commit の間の reopen は `store/project_wake.rs` の `project_root_event_mark` と `StaleReason::Evidence`（`rejected:stale_evidence`）で止まるが、commit の後の reopen は HumanChanged の wake 以外で起こされない | B |
| G2 | Roadmap の Outcome に決定的な checks が無い | `crates/task-core/src/roadmap.rs` の `RoadmapOutcomeSpec`（id・title・acceptance・priority・depends_on・dropped）に checks が無い。`Project.request` の Outcome block は `crates/task-core/src/project_proposal.rs` の `checks: Vec<OutcomeCheck>` を持ち、P0-E で root の `Check::Command` に結線済み | B |
| G3 | replay v2 の new C06・C10 は oracle pass で controller_achieved false | raw は `agent-docs/progress/2026-11-05-phase3/p0c/replay-v2/facts/<trial>.json`（trial は `arm-map/` で case と arm を引く）。C06: `questions ["root_failed:o2:1"]`、o2:1 の root は `cancelled`、承認は 259,500 秒と 522,900 秒。C10: `questions ["attempts_exhausted:o1"]`、o1:3 は CoS の instructed root（`created_by cos`）で done | B（妥当な停止か不具合かの判定） |
| G4 | blocked の task の数に比例して dispatch が遅い | 計測で `Dispatcher::tick` の段 `tick_cos_chat_launch`（`dispatcher/cos_chat/launch.rs`）が支配的で、その中の `triage_events`（`cos_chat/triage.rs`、event に触れた task があれば `human_inbox` を作る）と `triage_fallback`（`cos_chat/fallback.rs:459`、退避の候補があれば `fallback_one` が `human_inbox_items` を作る）が、`task_ops::human_inbox::human_inbox`（`crates/task-ops/src/human_inbox.rs:264`）を 1 tick に最大 2 回組み立てる。100 件で 1 回約 190 ms（約 1.9 ms/件） | C |
| G5 | project wake・判断の行の肥大化 | `crates/task-core/migrations/0112_project_controller.sql` の `project_wakes`・`project_controller_decisions`、`0104_script_cos_digest.sql` の `script_compare_log`。P1-I の仮想 72 時間で wake 1,860 行・判断 1,440 行（20 案件）、消す仕組みは無い | D（C の後） |
| G6 | 実 LLM の統括 CoS の旧新比較が無い | 比較器 `scripts/dev/general_cos_eval.py`（オフライン）、replay `scripts/dev/test-general-cos-consult-e2e.sh`。実 LLM の観測は 0 件 | E（人の承認の後） |
| G7 | 本番 RSS 182→589 MB/26 分の原因が未確認 | P1-I の `crates/celeris/tests/long_run_soak.rs`・`scripts/dev/long-run-soak.sh`。非本番の傾きは本番の 8.1〜9.5%（全区間） | F（人の承認の後） |

### D2 不変条件と試験の対応（守るもの・足すもの）
| 不変条件 | 既存の試験（実在） | 足す試験（担当） |
|---|---|---|
| Rust が根拠を数え直さない達成は書かない（I1〜I4・I11、K1b） | `crates/task-dispatch/src/dispatcher/tests/project_achievement.rs` の `project_achievement_*`（14 件、`*_between_decide_and_commit_is_stale` を含む） | 達成 → 根拠の root の reopen・re-review fail・IntegrationReopened → phase が achieved から waiting/active に戻り、旧達成の記録は残る（B）。到達 → 失効 → 修正の root → 再達成の e2e（B） |
| check の失敗は reviewer の合格に優先する（P0-E） | `tests/long_horizon_pilot_review.rs` の `pilot_review_check_*`・`review_check_wins.rs` | Roadmap の Outcome の checks が root の `Check::Command` になり、失敗で達成しない（B）。checks の変更は人の決定（Roadmap の proposal → decide）を通る（B） |
| 旧 milestones API は 410 | `crates/task-api/tests/lifecycle.rs` の `milestone_lifecycle_endpoints_are_gone` | 変えない（全子） |
| ready の取りこぼし 0・二重実行 0・wait/depends_on/human gate/lease の規則 | `dispatcher/tests/tick_and_dispatch.rs`・`task_waits.rs`・`human_gates.rs`・`orphan_takeover.rs`・`ownerless_runs.rs`、`crates/celeris/tests/long_run_soak.rs` の restart | 600/2,000/10,000 件の blocked で同じ規則を assert する（C）。受信箱の項目・退避の通知の内容が最適化の前後で同じ（C） |
| controller は LLM を呼ばず、未消費の wake・未適用の効果を残さない | `dispatcher/tests/project_controller_soak.rs` の `project_controller_soak_72h_{off,shadow,enabled}` | 消してよい行の条件（消費済み wake・効果 none の判断）と、消さない行（未処理 wake・pending effect・cursor・最後の判断と根拠・replay に要る行）の試験（D） |
| TS だけの変更の edit→verify で cargo 0 回 | release gate の script-bundle-gate（`scripts/selfdeploy/script-bundle-gate.sh`） | 未計測（第 3 フェーズの共通受入 (b) の残り。H で測る） |

### D3 性能のベースラインの計測の口
- `TickReport` に `step_us: Vec<(&'static str, u64)>`（tick の段ごとの µs、実行順）を足した。判断には使わない。
- 試験 `crates/celeris/tests/tick_scale_baseline.rs`（`tick_scale_baseline`）。本物の dispatch で blocked にした task の行を
  SQL で N 件に複写し（本文の task id も置き換える）、空 tick・`store.list(None)`・dispatch の遅延（`Env::dispatch` と同じ手順）と
  段ごとの時間を `celeris.tick-scale-baseline/1` の JSON で出す。`TICK_SCALE_NATURAL=1` は本物の dispatch で N 件を作る対照。
- 複写の妥当性: 100 件で dispatch の遅延 p50 は本物 673 ms・複写 642 ms、dispatch 中の `tick_cos_chat_launch` は 593 ms・585 ms で
  一致した。**空 tick は一致しない**（本物 5 ms、複写 227 ms）。原因は未確認のため、空 tick の数値は本物の対照だけを使う。
- C の目標の基準（同じ host・同じ command で before/after を取る）: blocked 600 件で dispatch の遅延 p50 4,824 ms（P1-I の
  26 分の計測では 655 件で 3,953 ms）。目標 p50 ≤ 500 ms。

### D4 B と C の並列の範囲（衝突を避ける file の持ち主）
- B: `crates/task-core/src/roadmap.rs`・`project_proposal.rs`・`store/roadmap*.rs`・`store/project_wake.rs`、
  `crates/task-dispatch/src/dispatcher/project_guard.rs`・`project_decider_ts.rs`・`project_wake.rs`・`project_resume.rs`、
  `crates/task-ops/src/project_snapshot.rs`・`project_controller_view.rs`、`control-ts` の project の policy、task-api の roadmap の route、
  試験 `dispatcher/tests/project_achievement.rs`・`long_horizon_bench.rs`・bench の fixture。
- C: `crates/task-ops/src/human_inbox.rs` と受信箱の組み立て（`build_attention` 等）、`crates/task-dispatch/src/dispatcher/cos_chat/`
  （`triage.rs`・`fallback.rs`・`launch.rs`）、`dispatcher.rs` の `tick`、`crates/task-core/src/store/` の task の一覧・索引、
  試験 `crates/celeris/tests/tick_scale_baseline.rs`・`long_run_soak.rs`。
- 両方が触れうる file: `dispatcher.rs`（B は触れない想定）、`crates/task-core/src/event.rs`（B が event を足すなら schema の再生成。
  C は足さない）。migration の番号は B=0115、C=0116、D=0117 を予約する（0115・0116 は第 3 フェーズで予約され未使用）。
- D は C の後（同じ store と tick を触る）。E・F・G は人の承認が要る実験の準備までを非本番で行う。

## 守ること（全子）
Project/Roadmap/root/Task tree の意味を変えない。Goal の新しい大きなモデル・無限に running の root を足さない。
TS は判断、Rust は永続化・安全・権限・状態遷移を決める。human_required・許可・本番操作の保護を弱めない。
K2/K4/IEEE Access の凍結評価と本番 DB を変えない。release promote・systemd・本番 config・本番 shadow・実 LLM の評価・
24〜72 時間の連続稼働は人の承認の後だけ。
