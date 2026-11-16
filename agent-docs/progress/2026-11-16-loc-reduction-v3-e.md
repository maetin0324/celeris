---
title: LoC 削減 v3 E — task-dispatch・celeris の整理（production 純減 1,038 行 / 閾値 2,500 行。未達）
tasks: [01M7J3NKVAH9NG53C5FEFNQJYK]
status: blocked
updated: 2026-11-16
---
# LoC 削減 v3 E — task-dispatch・celeris の整理

親 01M7J3N68JRNTP5Q4G934QWF36（依頼キー loc-reduction-v3-20261116）の子 E。依存は D（01M7J3NKV8E6P9WAWGMJR3KB1F）。候補は子 A の [進捗](2026-11-16-loc-reduction-v3-a.md) の E の割当（C2・C6・C7・C8・C12・C13・C14・C21）。

## 結論
- 試行 1 の production 全言語の純減は unit（`git merge-base HEAD main` → HEAD）で **1,038 行**。受け入れの size check の閾値 2,500 行に届かず、size check は exit 1。この task は done にしない。
- 累積（第 2 マイルストーン 5,000 行）は満たす。固定（開始時 main 415a19a7004cb6a73b872e861fa8d1cf634dddbb → HEAD）で 6,297 行、C・D・E の unit の合計で 2,701 + 2,558 + 1,038 = 6,297 行。両者の差は 0 行で、main に入った無関係の production の変更は無い（unit 基点は D の最後の commit の main）。
- E の write set（task-dispatch・celeris）で、承認なしに消せる本体の死んだ code は少ない。rustc の dead_code を使った走査（task-dispatch の `pub fn` を一時的に `pub(crate)` にし、他 crate の本体が名前で使う関数は戻して `cargo check`）で残ったのは試験用の口 21 個（約 200 行）だけだった。残りの候補は承認が要るか、効果が小さい（「提案」）。閾値に届かせるには人の判断が要る。

## 消した対象（production）
- celeris `notify.rs`: daemon が呼ばない旧い直の走査一式。`scan`・`scan_at`・`scan_approval_pending`・`scan_question_blocked`・`scan_bad_news`・`scan_cluster_login_needed`・`scan_secretary_reply`・`scan_task_failed`・`scan_decisions`・`scan_plan_approvals`・`scan_phase_checkpoint`・`transition_key`・`decision_breadcrumb`・`cost_label`・`decision_line`・`schedule`・`schedule_routes`・`InboxOutboundState`・`DigestOutboundState`・`select_batch`・`Candidate`・`link`・`node_name`・定数 `TASK_SCAN`・`REPORT_SCAN`・`MESSAGE_SCAN`・`PROJECT_SCAN`・`BAD_NEWS_PREFIX`。module doc に「互換試験用に残すが daemon は呼ばない」とあった旧経路で、送信は CoS の outbox（`select_routes_batch`）だけが使う。
- task-dispatch `dispatcher/cluster.rs`: `Dispatcher` の 1 行委譲 15 個（`set_cluster_connector`・`set_cluster_connect_pending`・`set_tunnel_forward_ensurer`・`set_tunnel_probe`・`set_tunnel_listener_probe`・`set_cluster_ssh_command_override`・`remote_ssh_settings`・`set_cluster_command_probe`・`set_cluster_master_watcher`・`take_tunnel_events`・`clusters_needing_login`・`tunnel_reachable`・`tunnel_listener_present`・`tunnel_target_healthy`・`tunnel_last_error`）。C2 の一部。呼び手は owner の `ClusterRuntime` を直接呼ぶ（dispatcher の中は `self.cluster`、daemon の結線は accessor `Dispatcher::cluster()` 1 個）。`set_cluster_liveness_probe` は write set の外の campaign（`dagbench/celeris_side.rs`）が呼ぶので残した。
- 新しい Runtime・Manager・Actor・port は足していない。足したのは accessor 1 個（4 行）だけ。

## 測定（受け入れの size check と同じ command。分類規則は開始時 main の source-size-report.py・toml に固定。提出時の HEAD で実行）
| 点 | production 全言語 | production Rust | own_total_with_tests |
|---|---|---|---|
| unit 基点（`git merge-base HEAD main`） | 381,079 | 334,964 | 1,037,546 |
| HEAD | 380,041 | 333,926 | 1,034,469 |
| 純減（unit / 固定） | 1,038 / 6,297 | 1,038 / 6,297 | 3,077 / 9,010 |

- 他の class: external_test は 461,147 → 459,111（旧経路の試験の削除）、tooling は 25,198 → 25,195（human-text の許可 3 行）。inline_test・generated・vendor・fixture・evaluation_infra・schema・migration・lockfile は unit 基点と HEAD で同じ。docs はこの進捗の分だけ増える。
- 累積の内訳（unit の合計）: C 2,701（[C の進捗](2026-11-16-loc-reduction-v3-c.md)）、D 2,558（[D の進捗](2026-11-16-loc-reduction-v3-d.md)）、E 1,038。合計 6,297 は固定計測の 6,297 と一致する。

## 変更（commit の件名）
- loc-reduction-v3 E: daemon が呼ばない notify の旧い走査（scan・schedule・schedule_routes・select_batch）と旧経路の試験を消す
- loc-reduction-v3 E: Dispatcher の cluster・tunnel の 1 行委譲 15 個を消し、呼び手は cluster_mut から ClusterRuntime を直接呼ぶ
- loc-reduction-v3 E: cluster の accessor を cluster() にし、bootstrap は 1 度借りて hook を挿す。消した notify の文言の human-text の許可 3 行を消す
- loc-reduction-v3 E: 進捗を書く

## 試験の変更（規約 10）
- `crates/celeris/tests/notify.rs`: 旧い走査の判定の試験 31 個（approval_pending・question_blocked・bad_news・secretary_reply・task_failed・decisions・plan approval・milestone の判定ほか）を消した。契約（daemon が notify の走査で通知を作る）は CoS の outbox への置き換えで廃止済みで、本体はもう走査を持たない。送信の仕組みの試験 7 個（1 回送って記録、3 回失敗で諦める、URL を出さない、429 を数えない、秘密の読み込み、送信の試し、偽の webhook）は残し、`select_batch` を本体と同じ `select_routes_batch` に、種を `CosEscalation` に替えた。
- `crates/celeris/src/notify/tests.rs`: `schedule_routes` の試験 2 個と、消した `link` の試験 1 個を消した。`root_done_summary` の選択の試験ほかは残した。
- task-dispatch `dispatcher/tests/cluster_tunnel.rs`・`session_resume.rs`・`health_review/flow_tests.rs`、celeris `lib/tests.rs`: 消した委譲の呼び出しを owner の呼び出しに替えただけで、検査の内容は変えていない。
- lease・restart・replan・stale-result の反例試験は変えていない。test-affected の nextest で通っている（「証拠」）。

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）
- `cargo fmt --all -- --check` → exit 0。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `bash scripts/dev/test-affected.sh` → 下の「test-affected」の行を参照。
- 受け入れの size check（閾値 `2500 1 - - -`）→ exit 1、`FAIL ['prod.unit=1038<2500']`。

## 受け入れ条件との対応
- 条件 0（統合・新しい大きな仕組みを足さない・反例試験）: 旧い notify の走査（同じ通知の判断の第 2 の経路）を消し、Dispatcher の cluster の薄い委譲を消した。足したのは accessor 1 個。反例試験は変えず、test-affected で通る。ただし R4 の残骸の統合は C2 の cluster の分しか進んでいない。
- 条件 1（第 2 マイルストーン）: 固定 6,297 行、C・D・E の unit の合計 6,297 行で、5,000 行以上。差は 0 行で、無関係の main の変更は無い。
- 条件 2（size check）: exit 1。unit の production 純減 1,038 行で、閾値 2,500 行に届かない。

## 未解決
- 閾値 2,500 行に 1,462 行足りない。E の write set で承認なしに消せる候補は、試験用の口 21 個（約 200 行。試験の書き換えが要る）、C14 target sweep の型の写し（25〜45 行）、残りの 1 行委譲（約 100 行）で、全部でも約 350 行。

## 提案
- 承認が要る削除（A の見積もりと、この run の走査の結果）: C7 旧 `[[genres]]`+`[[roles]]` からの harness の互換（100〜220 行）、C8 `[scratch]` 無効時の旧 build-cache の経路（80〜250 行）、C12 試験用の ablation の分岐（本体 30〜70 行）、C13 旧 worktree の配置（25〜60 行）、未配線の失敗経路の判断（task-dispatch `dispatcher/route_failure.rs` の `decide_route_failure`・`classify_failure`・`FailureOrigin`・`FailureAction`、`route_launch.rs` の `admit_by_availability`・`LaunchAdmission`。本体から呼ばれず、試験と `tests/policy_close.rs` だけが呼ぶ。約 120 行）。合計 約 355〜720 行。
- 全部を承認しても閾値 2,500 行には届かない見込み。閾値を E の write set で届く値に下げるか、write set を広げる（例: 未配線の判断が使う task-core `route_select` の escalation・availability）かを人が決める。
- `tree_units.rs` の stall・infra failure の通知（`NotificationKind::TaskFailed`）は、tick loop が cos_v1 の後に legacy として捨てる。書いてすぐ捨てる経路なので、CoS の feed に載せるか書き手を消すかを決める。
- task-core `chat/triage.rs` の `cos_triage_cutover` と celeris `notify/triage.rs` の `cutover`（C6）は、新しい DB の初期化（cos_v1 の印と events の cursor）も兼ねるので、この run では消さなかった。消すなら task-core の migration で初期化する変更が要る（write set の外）。

## 本番への移行
- 無し（config の欄・DB・API は変えていない）。
