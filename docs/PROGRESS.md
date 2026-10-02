# PROGRESS — taskd

## browser: ADR-0115 権限分離 launcher

run `01M3X8SRB3X08AXW8WK5PY7P9N` で launcher 実装・設定・host unit/手順書を統合後に検査。`cargo fmt --all -- --check` と `cargo clippy --workspace -- -D warnings` は exit 0。workspace test は sandbox の user namespace probe が `EPERM` となり、ADR-0095 DB guard を使う `instance_handoff` 5 件が失敗して exit 101（`CELERIS_ISOLATION_TESTS=skip` を付けても同じ。skip は browser isolation 試験だけに適用）。launcher ptrace 試験は `celeris-browser` user と `celeris-browser-launcher.socket` が存在しないため `SKIPPED (not passed)`。host 管理者に [browser-launcher-host-setup.md](ops/browser-launcher-host-setup.md) の準備を依頼し、準備後に実 process 証跡を追加する。機密能力は未解放。本節の詳細は [phase-browser-4](progress/phase-browser-4.md)。

現在地: **構造リファクタリング完了（2026-09-30、下記）。Phase 119、Phase E6、Phase F4b まで本番反映（release c51837427ac5、schema 28）。F5-1 dogfood の 3 回目を準備中。Browser capability Phase 1〜4 は追跡表どおり P4-A/B/C 一部達成で、別 host UID 実証と本番機密能力解放は後続（2026-10-01 にリファクタ後の main へ取り込み中）**。以後の追記は `docs/progress/phase-F.md` へ。

## 構造リファクタリング完了（2026-09-30、ADR-0079 / ADR-0082 / ADR-0083）

inline test の外出しと責務分割を完了した（worktree、main 未 merge）。前後 LOC 表と 2,000 行超ファイルの分類は [phase-structure-refactor.md](progress/phase-structure-refactor.md)。

- 証拠（最終 HEAD aaa6a1ca）: `cargo test --workspace` → exit 0、2,935 passed / 0 failed / 8 ignored、4 分 57 秒。`cargo fmt --all -- --check` と `cargo clippy --workspace -- -D warnings` → exit 0。test 属性数（`git grep -hE '#\[(tokio::)?test'`）は f34f060 = 2,907 → HEAD = 2,907。`git diff --quiet f34f060 HEAD -- crates/task-core/migrations docs/api/v1 docs/protocol config` → exit 0（互換性差分ゼロ）。`source-size-report.py --strict` → 0 active warning / 1 excepted。
- 結果: Rust inline test 100,831 → 6,160 行。2,000 行超の production は 10 本 → 1 本（`task-api/src/types.rs`、理由付き例外）。
- 未解決: main への取り込みと本番昇格は人。`paperqa.rs`（1,948）・`dispatcher.rs` facade（1,868）・`execution_plan/validation.rs`（1,739）が閾値に近い。
- 提案: 新しい責務は分割後の子 module に置き、facade に戻さない。`release.sh` の source-size-report 表示段で閾値接近を毎回確認し、2,000 行を超えたら同じ手順（test 外出し → 責務移動）で割る。

詳細な履歴と証跡は下記の分割ファイルを参照。既存の `docs/PROGRESS.md` 参照はこの目次を入口として維持する。

## 目次

- [Browser capability Phase 1](progress/phase-browser.md) — ADR-0078、既存 harness + agent-browser、管理者 grant・session・監査・dashboard 導線。最新 main 再統合後の gate 2026-09-28（Rust 2678 passed、GUI 1173 passed、mobile-audit 0 violations）。本番未昇格。
- [Browser capability Phase 2](progress/phase-browser-2.md) — ADR-0080、task policy からの制限生成・手動登録 credential broker（celeris-credentiald）・WAITING_FOR_AUTH/APPROVAL・Live View 本人限定。main a525af2 追従後の検査 2026-09-29（Rust 2865 passed、GUI 1213 passed）、検証 SHA `9737e9708124` の gate ok=true、verify ok=true / live_ok=false（旧版の SchemaTooNew）。本番未昇格。
- [Browser capability Phase 1〜4 の main 統合](progress/phase-browser-main-merge.md) — 2026-10-01、`478e86c4` とリファクタ後 main `2eb1b030` がともに祖先となる作業ブランチで、migration 0035/0036・schema 36 と ADR 0099〜0114 を確認。`cargo test --workspace` exit 0（3,206 passed / 0 failed / 12 ignored）、`cargo clippy --workspace --all-targets -- -D warnings` exit 0、source size active warning 0 件（既存例外 1 件）。P3-B frame、別 host UID・A13、機密能力の本番解放、本番設定と昇格は未解決。検証のコマンド・exit・テスト数はリンク先に記録。
- [Browser capability Phase 3](progress/phase-browser-3.md) — ADR-0099（制御 lease）/ 0100（live proxy ACL）/ 0101（identity 契約）/ 0113（P3-C control gate 配線）。P3-B live proxy・P3-C takeover は store・task-api・worker・GUI まで配線し e2e `phase3_` 3 passed。P3-C の worker 側 control gate は 2026-09-30 に run loop（`ActionServer::serve`）へ配線完了、`browser_control_gate_wire` 5 passed。P3-A は封緘・保管・失効・削除まで、利用（復元）は P4-A/P4-B の deliver_state（2026-09-30 実装済み）を参照。2026-09-30 の検査（Rust 3055 passed / 0 failed、clippy exit 0）。本番未昇格。

- [Browser capability Phase 4](progress/phase-browser-4.md) — ADR-0102。P4-A の実 runtime と production 起動経路は D3/D4 まで接続済み。P4-B の実 sink は未接続。P4-C は worker 起動前の `route` を接続し、未適合の機密要求を拒否。specialist・実 fixture 実行は未。2026-09-29、本番未昇格。 2026-09-29 run 01M3QGRCAHDK1AB4R9WBHJ9XHZ: ADR-0106 で同一 host UID の実 bwrap runtime（実 chrome-headless-shell・CDP pipe・6 namespace・ro root・socket 不可視・netns 遮断・controller kill/再起動回収）と稼働中 session への復元結合を実装（`browser_runtime_isolated` 4 passed、workspace 2977 passed、clippy exit 0）。その後の D3/D4 配線で egress proxy 結合と production 起動経路を実装。別 UID 実証は未。 2026-09-30 unit ipc（ADR-0109 D1〜D3）: `celeris-credentiald` に injection-only IPC（`injection.sock`、SO_PEERCRED 役割、稼働中隔離 session・CDP 対象・auth_section・lease の照合、sink FD への 1 frame、receipt のみ）を実装。`tests/injection_ipc.rs` 12 passed、workspace 3016 passed/0 failed、clippy exit 0。実 CDP sink・実攻撃・H3 端から端は後続 unit、機密能力は未解放。
- [Browser capability Phase 4](progress/phase-browser-4.md) — ADR-0102〜0108。P4-A/B の実 runtime・sink は未接続。P4-C は実 agent-browser/loopback fixture を ACP RPC・明示 Claude CLI・browser-specialist wrapper の scripted LLM で各7/7 実行し、その ledger を routing と実 browser fallback に接続。実 LLM 比較は ACP CLI/認証待ち。2026-09-30、本番未昇格。
- [Browser capability Phase 4](progress/phase-browser-4.md) — ADR-0109。P4-B wire-harness: `71640d79` を commit `35ab5564` として取り込み、実 broker の injection.sock と実隔離 browser CDP sink の harness 結線を追加。`cargo test -p task-worker --test browser_injection_wire` 2 passed / 0 failed / 0 ignored（実 browser、skip 無し）、`cargo test -p celeris-credentiald` lib 14 + broker 12 + injection_ipc 12 passed、`cargo clippy --workspace -- -D warnings` exit 0。unit attacks: `tests/browser_injection_attacks.rs` で実 chrome・実 broker の A1〜A17 を実行し `cargo test -p task-worker --test browser_injection_attacks` 2 passed（A8 の DOM 複製再表示は RedisplayGuard 未配線で区間後に sentinel が agent に見える所見＝未達、A13 実別 UID は未）、本番 H3 の実装・端から端検証は次項の h3-prod 記録を参照。2026-09-30、本番未昇格・機密能力未解放。
- [Browser capability Phase 4](progress/phase-browser-4.md) — ADR-0110（h3-prod, task 01M3RVA7P40BFPNCKY362WC243）。controller 所有 1 Chromium/CDP 共有（relay・token/Origin 検査・認証区間中の全面遮断）と管理者 site policy 由来の trusted selector（`TrustedLogin`・`validate_trusted_login`・broker の `selector_mismatch`）を D3 の照合順で `browser.rs` の H3 開始/終了に結線し、task-api e2e `production_h3_injects_once_without_exposure`（実 daemon 経路・実 bwrap・実 chrome-headless-shell・実 broker・実 CDP pipe・loopback fixture・scripted LLM で 1 回注入し event/live/LLM 入力/SQLite DB/WAL/run log/artifact の 6 面を sentinel 検索）と負の対照 `injected_leak_is_caught_by_the_same_scanner` を追加。closeout（run 01M3S43TR5TQJ4PQ40VVTR6VY0）の最終検査: `cargo test --workspace` exit 0（3041 passed / 0 failed / 11 ignored）、`cargo clippy --workspace -- -D warnings` exit 0、`cargo fmt --all --check` は初回 diff 2件（`task-dispatch/src/dispatcher.rs`・`task-worker/src/lib.rs`、いずれも本 WorkUnit 外の既存フォーマット崩れ）を `cargo fmt --all` で解消し再検査 exit 0。未解決: 本番 broker の admission は `Attested` のみで、この host は同一 UID のため `SameUid` 判定となり、H3 の機密起動（実注入）は本番経路では拒否のまま。e2e の正例は `same-uid-harness` feature 限定の試験 admission で得ており、本番昇格はしていない。
- [Browser capability Phase 4](progress/phase-browser-4.md) — WorkUnit attacks-merge（2026-09-30）。h3-prod merge commit `717c7733` 取り込み後、`crates/task-worker/tests/browser_injection_attacks.rs` の `CredentialPolicy` fixture 初期化に `login_url`/`password_selector`/`submit_selector` が無く `cargo test --workspace` が E0063（exit 101）で失敗していたのを修正（fixture の login URL・selector を追加、攻撃試験の期待値・A8 所見は不変）。`browser_credential.rs` の未使用 `use_credential`/`top_level_origin`/`Segment::origin`（旧 bridge 経路、H3 では不要）を削除。`cargo test -p task-worker --test browser_injection_attacks --test browser_injection_wire --test browser_cdp_sink --test browser_h3_wire --test browser_shared_cdp` → 全 5 バイナリ各 2 passed / 0 failed。`cargo test --workspace` → exit 0（3043 passed / 0 failed）。`cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all --check` → exit 0。本番コード・ADR は変更していない。
- [Browser capability Phase 4](progress/phase-browser-4.md) — WorkUnit unlock（2026-09-30、ADR-0112）。P4-B 判定: attacks A0〜A17（22 印、A8 は redisplay で合格）・h3-prod e2e・負の対照が全て通過したため、適合記録 `ConformanceResult.evidence`（試験名・結果）を追加し、`injection_attack_suite`/`auth_section_observation_stop` は要る試験が全て `passed` の証拠があるときだけ通る。記録は `scripts/browser-conformance.py --p4b-evidence` が実試験から作り、静的登録は無し。解放後も記録の無い backend・非隔離 runtime は拒否（新規試験 3 件）。`cargo test --workspace --no-fail-fast` exit 0（3047 passed / 0 failed / 11 ignored、初回 fail-fast は無関係の `instance_handoff` 3 件が負荷で失敗し再実行で通過）、`cargo clippy --workspace -- -D warnings` exit 0。本番 admission は `Attested` 必須で同一 UID host では拒否のまま（ADR-0110、変更なし）。未解決: A1 競合未再現・A4 OOPIF 未再現・A13 別 UID 実 process 未試験・P4-A 件の証拠化・実 ledger への runner 実行。本番未昇格。
- [Browser capability Phase 3/4](progress/phase-browser-4.md)（2026-09-30、task 01M3SM0WN346ABGF42QV02RZTP、WorkUnit record）。final review が挙げた 3 件の未達を実装した WorkUnit（control-gate・deliver-state・attacks-a1a4）を統合したブランチで検査: `cargo test --workspace` exit 0（**3055 passed / 0 failed / 11 ignored**）、`cargo clippy --workspace -- -D warnings` exit 0。P3-C control gate の run loop 配線（ADR-0113、`browser_control_gate_wire` 5 passed）、P3-A/P4-A の deliver_state（試験 admission の成功経路を実 bwrap + 実 chrome-headless-shell で実証、本番 `Attested` の `SameUid` 拒否は維持、`browser_restore_deliver` 3 passed）、P4-B の A1 target_changed・A4 OOPIF target_mismatch（実再現、`browser_injection_attacks` に `ATTACK-A1-TARGET-CHANGED-OK`・`ATTACK-A4-OOPIF-OK`）を解消。A13（別 UID 実 process）と本番 admission（`Attested`、同一 UID host では拒否）は未解決のまま残す。コード変更なし（検査と文書更新のみ）。本番未昇格。
- [Browser capability Phase 1〜4 受け入れ行の追跡表](progress/phase-browser-acceptance.md)（2026-09-30、task 01M3SPF94RDWTPWHNDEQD68VB9）— **現在の判定の正本**。P1-1〜9・P2-1〜13・P3-B・P3-C は全行合格。P3-A は P3-A-8（本番 identity 復元の成功）、P4-A は P4-A-7（別 host UID 実証）が後続 `01M3SPN8H05EJ3DHPVEGEYTMEH`、P4-B は P4-B-6（本番機密能力の解放）が後続 `01M3SPN8HPHPWZ32F0AG986TWS`・A13 が後続 `01M3SPN8HE6A1TZ54GBZGHMZYJ`、P4-C は P4-C-6 が後続 `01M3SPN8HPHPWZ32F0AG986TWS`（決定 sep-uid=a）。H4/H5/H7・ADR-0080 H1/H2/H3/H6 の決定適合節を含む。検査は `bash scripts/check-browser-acceptance.sh`（表の判定・引用テストの実在・file:line・引用 cmd の実行）: 2026-09-30 exit 0（76 行、引用テスト 141 件、file:line 30 件、22 群の cargo test が全部 ok）。判定を 1 行空にすると exit 1（`FAIL: P2-2: 判定が '合格' でも '後続' でもない（''）`）、合格行を task id 無しの後続にすると exit 1（`FAIL: A5: 後続行に ULID の task id がない`）を手で確認し、文書は戻した。同じ branch で `cargo test --workspace --no-fail-fast` exit 0（3055 passed / 0 failed / 11 ignored）、`cargo clippy --workspace -- -D warnings` exit 0。phase-browser-3.md・phase-browser-4.md の状態行と行ごとの判定は追跡表に合わせた。（再 run 2026-09-30: 統合後 check で `browser_shared_cdp::real_shared_cdp_and_auth_section` が高負荷時に 10 秒待ちで偽 Timeout（`sandbox TCP to controller relay failed`）→ sandbox 内 probe と test の待ちを 60 秒の期限式に、TLS fixture 待ちを 30 秒にした。検査スクリプトは `... ok` 判定を stdout だけで行うようにし、test の stderr 割り込みによる偽の不合格を防いだ。再実行: `cargo test --workspace` exit 0（3055 passed）、`cargo clippy --workspace -- -D warnings` exit 0、`bash scripts/check-browser-acceptance.sh` exit 0（22 群・141 件 ok）、P2-2 の判定を空にすると exit 1 を再確認して戻した。）（再 run 2 2026-09-30: 統合後 check で `cluster_login::tests::totp_needs_code_reports_the_exact_prompt` が高負荷時に 30 秒待ちでも Timeout → 偽 ssh が直前に書かれた askpass を exec して ETXTBSY で黙って失敗しうるため、4 つの偽 ssh で askpass を `sh` 経由で読ませた。再実行: `cargo test --workspace` exit 0、`cargo clippy --workspace -- -D warnings` exit 0、`bash scripts/check-browser-acceptance.sh` exit 0（22 群・141 件 ok）、P2-2 の判定を空にすると exit 1（`FAIL: P2-2: 判定が '合格' でも '後続' でもない（''）`）を再確認して戻した。）これより上の Phase 3/4 の行の「満たす／未達／一部」は記録時点の判定。本番未昇格。
- [Browser capability H3 追跡表同期](progress/phase-browser-acceptance.md)（2026-09-30、task 01M3SPF94RDWTPWHNDEQD68VB9、WorkUnit h3-doc-sync）。並行 WorkUnit restore-obs-stop（commit `fa801f18`）が `restore_in_session`（`crates/task-api/src/browser_identity.rs:307`）に、controller への投入前に `observation_stopped` を記録し session 終了まで解除しない結線を実装したのを受け、決定適合 H3 節の「矛盾（後続）」「復元経路は後続」を削除し、新テスト `restore_enters_observation_stop_until_session_end`（task-api）・`restored_session_refuses_agent_observation`（task-worker）を cmd 付きで追加、判定を「適合」に更新。`docs/progress/browser-followups.md` の `01M3SRZ4X8NHRE0BB1QXMBTPKJ` を解決済みと記録し、celerisctl で cancel 済みであることを確認（既に Cancelled）。検査: `bash scripts/check-browser-acceptance.sh` exit 0（23 群、引用テスト 143 件、全部 ok）。`cargo test --workspace` exit 0。`cargo clippy --workspace -- -D warnings` exit 0。

- [Phase 1–50（Phase 0 の初期記録を含む）](progress/phase-001-050.md)
- [Phase 51–100](progress/phase-051-100.md)
- [Phase 101–150](progress/phase-101-150.md)
- [Phase E](progress/phase-E.md)
- [Phase F](progress/phase-F.md) — 最終報告: [ADR-0074 Phase F 最終報告](execution-parallel-report-2026-09-28.md)（2026-09-28）。F6: 既存の Task / 案件を後から分解の経路に入れる（`POST /tasks/{id}/execution/decompose`・MCP `task_decompose`・retry の再判定）、案件の名前・説明の編集（2026-09-28、未昇格）
- [Phase R（再帰的な task 分解、ADR-0079）](progress/phase-R.md) — R0（設計: 節点は task だけ、段階の unit は leaf か子 task、max_depth 3、決定の要求、子は親ブランチへ取り込み、案件計画の廃止）完了 2026-09-28。R1a（plan/3 の型と検証、`Task.tree`、migration 0031 = **schema 31、昇格は stop → start**、木の Event 8 種、`[execution.tree]` 既定 `enabled = false`）完了 2026-09-28。R1b（kind task の unit からの子 task の生成・状態の写し・子を含む段階の完了・`awaiting_children`・subtree の中止の連鎖〈`parent_cancelled`〉・木での委譲の禁止・`review: human` の途中確認。migration なし）完了 2026-09-28。R1c（子のブランチ `celeris/<child_id>` を段階の基点から切る・統合 WU が子のブランチを親ブランチへ merge して `PhaseIntegrated.merged` に子を残す・子の最終レビューは親のブランチと比べる・子は `deliveries` / 人の取り込み〈409 `tree_child`〉/ `TaskReady` を持たず root だけが main へ。migration なし）完了 2026-09-28。R2a（深さの gate の閾値 `5 + gate_depth_step × (d − 1)`・木の子は shadow / off でも gate を採用・計画の採用時の unit の gate〈leaf ↔ task の上げ下げ、`UnitGateOverridden`〉・木の上限〈計画の段階・段階あたり・子 task・`max_depth`・木の leaf / run / replan / トークン〉の超過は `kind: limit` の決定の要求と `blocked(decision)` で超えた分だけを止める・深さ別の reviewer の run と定価を含む木の数え上げ。migration なし）完了 2026-09-29。R2b（/3 の planner に深さ・残りの深さ・leaf の基準・計画と木の残りの上限・祖先・`stages_hint`〈`Task.routing.stages_hint`〉を渡す・/3 の 2 回不正は atomic に倒さず `kind: plan_invalid` の決定の要求〈task は ready のまま run を止める〉・/3 の replan〈全体を書き done の unit は持ち越し〉・子の work の失敗 → 親の replan〈子の理由と checkpoint を planner へ、同じ unit から attempt + 1 の子〉・節点の `max_replans` 超過は `limit:max_replans`・子の基盤の失敗は 1 回だけ自動で作り直し、再度なら unit `blocked(infra)` と障害通知。migration なし）完了 2026-09-29。R3a（決定の要求の回答 API〈`GET /decisions`・`GET /tasks/{id}/decisions`・`POST /decisions/{id}/answer|withdraw|revise`〉と MCP `decision_list` / `decision_answer`〈`tasks:interact`〉・採用で計画の決定を path 付きの要求にし答えの無い決定に依存する unit だけを止める・回答の効き目は選択肢 → 効き目の表で決定的〈待つ unit の再開、limit の `raise-once`・`replan`・`withdraw`、plan_invalid の `replan`〈note を planner へ〉・`atomic`・`cancel`〉・答えを子の objective と leaf の前置きに固定の書式で注入・worker の `result.json` の `decisions`〈`self` だけがその unit を止める、上限超過は 1 件に束ねる〉・受信箱の `decisions` と件数・Discord 通知〈run ごとに束ね、24 時間後に 1 回だけ再通知〉。migration なし）完了 2026-09-29（worktree、main 未 merge）。R3b（root の /3 の計画は決定を含む・`review: human` の段階・上限の 0.8 以上のどれかで `awaiting_plan_approval` に止まり〈`PlanGate`・`PlanApprovalRequested`、unit を 1 つも起こさない〉、`POST /tasks/{id}/execution/plan-gate {action: approve|replan|withdraw}` と MCP `task_plan_gate`〈`tasks:interact`〉で応える・承認の要らない計画は報告の流れに 1 件だけ〈通知なし〉・受信箱の `attention.plan_approval` と通知 `plan_approval`〈計画の決定を束ねる〉・木の生存確認〈`task_core::tree::liveness`、理由なく止まった節点に `liveness_timeout_secs` 既定 600 秒で `StallDetected` と `tree-stall:` の障害通知を 1 回〉・人の replan の 1 回目が不正でも 2 回目の試行が起きる・木の子の `task_failed` と子の run の悪い知らせを鳴らさない。migration なし）完了 2026-09-29（worktree、main 未 merge）。R4a（`GET /tasks/{id}/task-tree?root=`〈ADR の `/tree` は作業ツリーの閲覧が使っているため別名〉で節点ごとの段階・unit・導出値〈`awaiting_children` / `awaiting_plan_approval` / `held_on_decision` / `blocked_infra`〉と roll-up〈自分の分と subtree: role ごとの run・reviewer の run と定価・トークン・定価と完全性・quota・壁時計・leaf・子 task・未回答の決定〉と木の上限の使用・純粋関数 `task_core::tree_metrics::rollup`・`GET /projects/{id}` の `root_totals`・`GET /metrics/execution?group_by=depth`〈U-R7〉・段階の途中報告の `child_units`・replay が /3 の replan で消えた / 書き直した unit を events から同じに作り直す。migration なし）完了 2026-09-29（worktree、main 未 merge。replan が行の `phase` を書き換えない既存の不具合を見つけた〈未修正、phase-R.md〉）。R5a（案件計画・途中目標の書き込み・`POST /plans` を 410、`auto_advance` を 422、`celerisctl projects plan` を削除・途中目標の自動作成 / Go / ADR-0077 / 判定 run / `milestone_ready` を停止・`is_root_task`・`POST /tasks/{id}/pause|resume`〈subtree、`ready_tasks` が祖先を辿る〉・`GET /projects/{id}` は途中目標を既定で隠し `?include_frozen=true` で返す〈凍結 25 行、非終端 7 行〉・CoS の指針〈1 依頼 = 1 `create_task`、`stages_hint`、`add_milestone` は理由付きで落ちる〉・`stages_hint` の書き込み口。migration なし〈schema 33〉）完了 2026-09-29（worktree、main 未 merge）。R4b（GUI の木タブ・決定と承認の受信箱・案件ページの root 一覧）は main に併合・昇格済み。R5b-prep（人の `PUT/POST /tasks/{id}/execution-plan` と `celerisctl execution plan set|put --config` の /3 が daemon の実効の `[execution.tree]` で検証され planner と同じ経路〈unit の gate・計画の決定 origin human・木の上限・1 トランザクション〉を通る、人の計画は PlanGate を挟まず報告だけ、`POST /tasks/{id}/tree/adopt` と `celerisctl tree adopt` と計画の unit の `adopt`〈done / failed の既存の task を unit done で結び、統合は既に基点にあれば skipped〉、`/plans/new` の撤去、以前の途中目標は開いたときだけ `include_frozen=true`、手順書 `docs/ops/adr-0079-r5b-runbook.md`。migration なし〈schema 33〉）完了 2026-09-29（worktree、main 未 merge）。次は R5b（本番の移行と dogfood。手順書のとおり人が実行）。R7-1（クラスタ job〈PBS / Slurm〉の durable wait: `result.json` の `{"type": "wait", "kind": "cluster_job", ...}` で run を閉じ、daemon が `qstat -xf` / `sacct` を `poll_secs` ごとに poll して終われば続きの run〈前置きに job の最終状態と Exit_status〉、上限で人に聞く、中止は qdel しない。ADR-0090、migration 0034 = **schema 34、昇格は stop → start**）完了 2026-09-30（worktree、main 未 merge）
- [Phase P0: dispatcher.rs の責務分割](progress/phase-P0-dispatcher.md) — ADR-0082、production を `dispatcher/` の子モジュール 18 本へ移し `Dispatcher` を facade に（17,224 → 2,430 行）。task-dispatch 450 + 4 passed で不変、workspace test / clippy exit 0。完了 2026-09-29（worktree、main 未 merge）
- [Phase guardrail: source-size-report](progress/phase-guardrail.md) — ADR-0083、production/inline test/生成物を区別する warning-only ツール（既定 exit 0、`--strict` のみ非 0）と gitignore された untracked `mod` 参照の検出。`scripts/tests/` に 28 tests。`release.sh` の `cargo-clippy` 直後に表示段を追加。現 HEAD で cargo test --workspace 2886 passed / 0 failed、clippy 0 warnings、`--strict` は 21 件（audit.md §9 が既知としていた残存分）。完了 2026-09-30（worktree、main 未 merge）
- [Phase K（知識ベース）](progress/phase-K.md) — K-1（知識の置き場の整理と配置ガード。案件の `slug` = migration 0029）完了 2026-09-28（worktree、main 未 merge）
- [Phase G（ビルドキャッシュの 2 層化、ADR-0075）](progress/phase-G.md) — G0（設計）完了 2026-09-28。G1（scratch pool + semantic GC + celerisctl / metrics）完了・本番反映 2026-09-28。G2（sccache L1 の配線 + `CARGO_INCREMENTAL=0`）完了 2026-09-28。G3（L2: webdav の階層 cache server + flusher + L2 の GC + 監視）完了 2026-09-28（worktree、main 未 merge。cache server の有効化は人）。G3-fix1（継いだ `RUSTC_WRAPPER` / `SCCACHE_*` を run と checks から外す）完了 2026-09-28（worktree）。SD-1（release / verify の所要時間の短縮: 共有 target・GUI の段の skip・本番依存の cache・verify の所要時間）完了 2026-09-28（worktree）。SD-2（release の gate の `cargo-test` をテストバイナリ並列に: cargo-nextest 0.9.146、214 s → 115 s〈実行 202 s → 78 s〉）完了 2026-09-28（worktree）。SD-3（`truncate_phase_report` を挙動不変で O(n²) → O(S log n) に: 該当テスト 45.9 s → 0.02 s、並列 gate 69 s → 58 s）完了 2026-09-28（worktree）

各 Phase の詳細・証跡・申し送りは上記の分割ファイルを参照。既存の `docs/PROGRESS.md` 参照はこの目次を入口として維持する。

## F5-fix8: クラスタの ssh master の維持と切断の記録（ADR-0078）

2026-09-28 実装完了（未昇格、schema 30）。`ControlPersist=yes` の明示・鍵認証の再接続の抑制・切断の通知と回数（`cluster_connection_log`、
`GET /clusters` の `stats`）。[記録](progress/phase-F.md#f5-fix8-pegasus-の-ssh-master-を長く保ち無駄な再接続をやめ切断を数えて知らせるadr-00782026-09-28)。

## Phase F5-1 dogfood（再レビュー対応）

worker・review の完了を JoinHandle で明示同期し、実時間の待機回数に依存しない検証へ変更。
[実装と検証の記録](progress/phase-F.md#f5-1-review-repair)を参照。

## Web GUI Phase 0（2026-09-29、設計・移行計画）


- 判断と実装方針: [ADR-0081](adr/0081-web-spa-frontend.md)。現行 GUI の全 42 route、操作・認証・通知・SSE・file viewer・mobile 要件と Phase gate は [feature parity matrix](web/feature-parity.md)、Phase 1〜7 の session 単位の作業・受け入れ条件・検証方法・人の判断点は [implementation plan](web/implementation-plan.md) を参照。計画で参照する遅延 baseline は main `06e9a03cffe8` 時点で 5 秒遅延時約 15 秒、10 秒時約 30 秒、遅延 5 秒 + tick 2 秒で `/tasks` → `/tasks/:id` は 120 秒後も未完了。詳細と再現手順は WU `baseline` の成果物に記録。
- GUI 不変 gate: `git diff --quiet 06e9a03cffe8 -- gui ':!gui/docs/adr/0002-frontend-stack.md'` → exit 0。GUI の差分は旧 ADR `gui/docs/adr/0002-frontend-stack.md` の supersede 追記だけ。
- Rust gate: `cargo test --workspace` → exit 101（sccache 起動時 `Operation not permitted`、rustc コンパイル開始前）。`cargo clippy --workspace -- -D warnings` → exit 101（指定 `CARGO_TARGET_DIR` 内の `.cargo-build-lock` を read-only filesystem のため開けず）。どちらもコード検査に到達せず、コード起因か判定できていない。
- GUI gate（`gui/`）: `pnpm typecheck` / `pnpm test` / `pnpm build` は各 exit 1。pnpm 11.27.0 の依存事前確認がユーザー cache の SQLite database を開けず、各コマンドの実処理は開始しなかった。テスト数は未取得。main との比較も未実施。
- 未解決と提案: sccache と `CARGO_TARGET_DIR` が書き込み可能な環境で Rust 2 gate を再実行し、pnpm store が利用できる環境で GUI 3 gate と main 比較を再実行してテスト件数を記録する。今回の GUI 差分 gate `git diff --quiet 06e9a03cffe8 -- gui ':!gui/docs/adr/0002-frontend-stack.md'` は exit 0。旧 ADR 追記を含む GUI 全体の差分は新 ADR-0081 に supersede として記録済み。ADR・parity・計画の相互リンクを確認済み。

## Web GUI Phase 1（完了 2026-09-30、scaffold と gateway）

P1-01〜P1-09 完了。以後の Web GUI の記録は [progress/phase-web.md](progress/phase-web.md) へ（Phase 1 の証拠・未解決・提案もそこ）。

## Web GUI Phase 3（完了 2026-09-30、中核の画面 P3-01〜P3-15）

P3-01〜P3-15 完了。証拠・未解決・提案は [progress/phase-web.md の Phase 3 節](progress/phase-web.md#phase-3完了-2026-09-30中核の画面-p3-01p3-15)。

## Web GUI Phase 4（完了 2026-10-01、管理の画面 P4-01〜P4-17）

P4-01〜P4-17 完了。GUI/web 静的検査・parity e2e・V3・mobile-audit は exit 0。`cargo test --workspace` は exit 0（2,886 passed / 0 failed / 7 ignored）、`cargo clippy --workspace -- -D warnings` は exit 0（warning 0）。証拠・未解決・提案は [progress/phase-web.md の Phase 4 節](progress/phase-web.md#phase-4完了-2026-10-01管理の画面-p4-01p4-17)。

## Web GUI Phase 5（完了 2026-10-01、横断 gate P5-01〜P5-04）

P5-01〜P5-04 完了。Latency は 30 path で URL/見出し最大 69.4/90.1 ms、10 秒遅延時の差は最大 15.5/16.5 ms、H1 fallback は fixture で 1 回。Security X1〜X6/X8、mobile/a11y 30 path × 4 幅（axe critical/serious 0、横溢れ 0）、parity 総点検 X10/X11/X15 は合格。H6 の期間・合格条件は人の決定待ち。証拠・未解決・提案は [progress/phase-web.md の Phase 5 節](progress/phase-web.md#phase-5完了-2026-10-01横断-gate-p5-01p5-04)。

## Web GUI Phase 6（P6-01〜P6-03 完了 2026-10-01、並行運用の準備）

P6-01 の web 配布物、P6-02 の web ADR-W3・systemd unit・非 blocking release 段、P6-03 の dogfood 手順を整備。dogfood-mode=a の人の回答に従い、2026-10-01 18:32 UTC に release `bf54b41ad627` の web gateway を本番 daemon 向けに loopback `127.0.0.1:7720` で起動。LAN `192.168.1.103:7721` の入口も起動し、両方の `/healthz` は release 一致。Playwright で PC 1440px・スマホ 390px の各 6 画面と主要 GET API 6 件が HTTP 200、gui/ :7700 は継続して HTTP 200。`cargo clippy --workspace -- -D warnings` は exit 0。`cargo test --workspace` は 261 passed、`releases_api` の user scope bus 接続エラー 2 件のみを人の判断に従い環境由来として除外（同 suite 6 passed / 2 failed、再実行でも再現）。H10 の staging は release `bf54b41ad627` で verify exit 0・web parity 3 passed。N-1 互換は schema 差で `live_ok=false`。H6・H9 は人の決定待ち、H7 は配信切替判断待ち。LAN の別端末からの実到達は未確認。本番 release パスは前 run の staging 成果物への symlink なので保持が必要。詳細は [progress/phase-web.md の Phase 6 節](progress/phase-web.md#phase-6p6-01p6-03-完了-2026-10-01並行運用の準備p6-04-以降は未着手)。

## Web GUI 最終整合（task close-out、2026-10-01）

**検証 sha:** `d95b1653859d4dd5e3ded68b0abbba793a3a3eac`（前の子の成果を取り込んだ HEAD、記録更新前）。V1 install/test/typecheck/build は成功（GUI 84 files / 1249 tests）。V2 frozen install/typecheck/lint/test/build/gen:types/boundaries/secrets/parity(--require-phase 6) は成功（Vitest 24 files / 178 tests、Node 41 tests）。parity e2e は最初 cutover の store path 前提で失敗したが、一時 store を用意した再実行で 103 passed / 8 skipped。selfdeploy 7 本と parity commit 祖先検査は成功。`cargo clippy --workspace -- -D warnings` は exit 0。`cargo test --workspace --no-fail-fast` は初回と1回の再実行がともに exit 101、25 targets failed。主因は sandbox の user namespace 拒否（`Operation not permitted`、browser isolation の `NoChildPid`）と、それに伴う instance handoff/delegation 系失敗。初回だけ `browser_shared_cdp` の TCP relay 失敗も発生。crates/ は変更せず、Rust gate は main の別 task で扱う。人の adr-place 判断 (a) により当時 ADR 0082・0083・0096 は `docs/adr/` に置いたままだったが、後の人の決定 adr-scope により `docs/web/adr/`（web ADR-W1〜W3）へ移設した（[phase-web の adr-place / adr-scope の記録](progress/phase-web.md#adr-place--adr-scope-の記録)）。詳細・各 exit・失敗群は [phase-web 最終整合節](progress/phase-web.md#最終整合task-close-out)。

再試行（run `01M3WSCR1TVZ16NDM2BYKF8E2X`、attempt 2）の**検証 SHA は `d387be16a00426b03a48b7e11849611d8cd19047`**（記録 commit 前）。前回レビューで失敗した `/tasks/new` parity e2e の遷移待ちを修正。V1 の frozen install/test/typecheck/build は各 exit 0（GUI 84 files / 1,249 passed）。V2 の frozen install/typecheck/lint/test/build/gen:types/boundaries/secrets/parity(--require-phase 6) は各 exit 0（Vitest 24 files / 178 passed、Node 41 passed）。全 parity e2e は exit 0（103 passed / 8 skipped）、selfdeploy 7 本も各 exit 0。前子の祖先・完了 parity commit・差分範囲の検査と `cargo clippy --workspace -- -D warnings` は exit 0。`cargo test --workspace --no-fail-fast` は初回と指定された1回の再実行がともに exit 101（各 3,130 passed / 77 failed / 12 ignored、25 targets failed）。失敗例は `instance_handoff::normal_mode_does_not_inject_the_smoke_builtins`（namespace の `Operation not permitted`）、`browser_shared_cdp::real_shared_cdp_and_auth_section`（`unshare: Operation not permitted`）、`browser_runtime_isolated::real_browser_in_runtime_facts_and_restore_refused_on_same_uid`（`NoChildPid`）。両回の全失敗名とメッセージは run artifacts に記録。namespace 制約に伴う crates/ の失敗は未解決で、crates/ は変更せず main の別 task で扱う。詳細は [phase-web の再試行節](progress/phase-web.md#最終整合の再試行run-01m3wscr1tvz16ndm2bykf8e2xattempt-2)。

**この exit 101 の記録は、下の「最終 gate（2026-10-02）」の記録で置き換わった。** crates/ は無変更で、原因は run のサンドボックスによる user namespace 制約だった。サンドボックスを外して実行したところ `cargo test --workspace` は exit 0（3,206 passed / 0 failed / 12 ignored）、`cargo clippy --workspace -- -D warnings` も exit 0 だった。

## Web GUI 最終 gate（2026-10-02、HEAD `c63d53c21d46`）

前回 2 回の最終整合記録（attempt 1・attempt 2）はいずれも `cargo test --workspace --no-fail-fast` が sandbox の user namespace 制約で exit 101 となり、reviewer が Phase 完了 gate 未達とした。crates/ は本 task で無変更なのでこれは環境の問題と判断し、Bash サンドボックスを外して（dangerouslyDisableSandbox）、Celeris が渡した `CARGO_TARGET_DIR` / `RUSTC_WRAPPER` のまま再実行した。結果: `cargo test --workspace --no-fail-fast` exit 0（3,206 passed / 0 failed / 12 ignored）、`cargo clippy --workspace -- -D warnings` exit 0。V1（gui/、pnpm@11.27.0 固定）の install/test/typecheck/build は各 exit 0（Vitest 84 files / 1,249 passed）。V2（web/、pnpm@12.6.0 固定）の install/typecheck/lint/test/build/`gen:types --check`/`check:boundaries`/`check:secrets`/`check:parity --require-phase 6` は各 exit 0（Vitest 24 files / 178 passed、Node test 41 passed）。`git diff --quiet $(git merge-base HEAD main) -- gui crates docs/api` は exit 0（差分なし）。install・build 後も追跡ファイルへの変更なし。詳細な表とコマンドは [phase-web の最終 gate 節](progress/phase-web.md#最終-gate2026-10-02head-c63d53c21d46)。

dd6219db の probe 修正（`crates/task-worker/tests/browser_shared_cdp.rs` で WebSocket frame を最後まで読む）は crates/ の範囲外変更として revert した。必要な修正は main 向けの別 task で入れる。

**scope 復元後の最終 HEAD 検証（記録 commit 前）:** `eb19cfe6d85ab49c4542cda261456d8702dd229b`。段 1 の Rust 結果も同じ HEAD（記録 commit を除きコード差分なし）。`cargo test --workspace` は exit 0（3,206 passed / 0 failed / 12 ignored）、`cargo clippy --workspace -- -D warnings` は exit 0。V1 GUI（pnpm@11.27.0）の test/typecheck/build は exit 0（84 files / 1,249 tests）。V2 web（pnpm@12.6.0）の typecheck/lint/test/build/`gen:types --check`/`check:boundaries`/`check:secrets`/`check:parity --require-phase 6` は各 exit 0（Vitest 24 files / 178 tests、Node 41 passed）。両 install も frozen lockfile で成功。GUI 指定 install は既定 store の SQLite open error で exit 1 となったが、`--store-dir /tmp/celeris-pnpm-store` の再実行は exit 0。前の exit 101 は sandbox の unshare/user namespace `Operation not permitted` によるもので、本節の cargo 結果で置き換える。scope 復元の `git diff --quiet $(git merge-base HEAD main) -- gui crates docs/api docs/adr` は exit 0。install・build 後の status は記録対象以外が空。詳細は [phase-web の scope 復元後検証節](progress/phase-web.md#web-最終-head-検証scope-復元後)。

## Web GUI 最終再検証（2026-10-02、HEAD `a75d882e42a7`）

scope 復元後の指定 web 検証は全て exit 0（Vitest 24 files / 178 passed、Node 41 passed）。GUI は pnpm@11.27.0 frozen install・test・typecheck・build が exit 0（84 files / 1,249 passed、SQLite の既定 store 問題は `/tmp/celeris-pnpm-store` で回避）。`check:secrets` の down gateway port race を blackhole upstream で除去し、前回の `/api/health: expected 502, got 404` は再現せず。crates/ は差分ゼロのため Rust test（3,206 passed / 0 failed / 12 ignored）・clippy（exit 0）は revert-rust の記録を引き継ぐ。`git diff --quiet $(git merge-base HEAD main) -- gui crates docs/api docs/adr` は exit 0。詳細は [phase-web の scope 復元後検証節](progress/phase-web.md#web-最終-head-検証scope-復元後)。

## Rust gate 再実行（repair、2026-10-02、HEAD `080eeda00176`）

この run で `cargo test --workspace` をフレッシュ実行した結果 exit 101。`instance_handoff` の5 testが worker DB guard で必要な user namespace の `Operation not permitted` により失敗し、並行起動に依存する2 testも失敗した。`browser_shared_cdp` の単独実行も exit 101（`inner_shared_cdp` は pass、`real_shared_cdp_and_auth_section` は `unshare ... Operation not permitted`）。このため sandbox 外での Rust workspace test は未検証であり、過去の pass 件数をこの run の結果としては扱わない。`cargo clippy --workspace -- -D warnings` は exit 0。web の gen:types・boundaries・secrets・parity check は exit 0。crates/ は変更なし。詳細は [phase-web の Rust gate 再実行節](progress/phase-web.md#rust-gate-再実行repair、run-01m3x948ker5j9p5dj3nsrtszw-attempt-2)。

## Web GUI dogfood（開始 2026-10-01、release bf54b41ad627）

- 状態: 本番 daemon 向けの web gateway `127.0.0.1:7720` と LAN 入口 `192.168.1.103:7721` を起動。gui/ :7700 は継続稼働。PC 1440px・スマホ 390px の読み取り確認は合格。
- H6: 期間・合格条件・判定日は人の決定待ち。決まるまで cutover しない。H9: 通知方針は人の決定待ち。H10: release `bf54b41ad627` の staging verify exit 0、読み取り parity 3 passed。
- 配置上の問題（2026-10-01）: 本番 release パスが前 run の staging 成果物を指す symlink。参照先を dogfood 中に削除しない。NFS 実体コピーは途中で中止。再起動時は web unit と LAN socket を手動で start する。恒久化の対応・再確認結果は未記入。
- 端末確認の残り: LAN の別の物理端末からの到達・操作は未確認。結果を確認したら追記する。
- 期間中の問題記録: `<日付>｜<画面>｜<端末・ブラウザ>｜<現象>｜<重大度>｜<対応・タスク ID・再確認結果>` の形で 1 件ずつ追記する。
## Phase browser-3 再試行（2026-09-29, task 01M3Q2FPRCF34F00PBZSMNSZE8）

- 認証区間（ADR-0080 H3）を worker → store op（task-api `auth-section` と共通）→ control 状態へ配線、API で takeover/renew を 409 拒否、実 `forward_events` が区間中 progress・artifact・live event を 0 件にする。詳細・証拠は [phase-browser-3](progress/phase-browser-3.md)。
- 証拠: `cargo test --workspace` exit 0（2931 passed）、`cargo clippy --workspace -- -D warnings` exit 0、`cargo test --workspace auth_section` exit 0。
- 追加（同 task の Run #2）: GUI `browser-control.server.ts` の auth_section 拒否の試験（`gui/test/unit/browser-control.test.ts`、4 passed、`pnpm test` 1236 passed）。テスト専用の未配線経路 `BrowserLive`／`CliCloser` を削除。最終証拠: `cargo test --workspace` exit 0（2929 passed / 0 failed）、`cargo clippy --workspace -- -D warnings` exit 0、`cargo test --workspace auth_section` exit 0。
- 行ごとの判定: P3-A 保管側は満たす・復元は未（P4-A 後、`isolation_required` で拒否）／P3-B 満たす／P3-C 満たす（API・GUI・認証区間）。（2026-09-29 時点。現在の判定は [追跡表](progress/phase-browser-acceptance.md): P3-A 一部達成〔P3-A-8 後続 `01M3SPN8H05EJ3DHPVEGEYTMEH`〕／P3-B 達成／P3-C 達成）
- 未解決: identity 復元は P4-A 後（ADR-0101 D3）。worker 側 control gate（human control 中に agent の操作を止める）の run loop 配線は未。

## Phase browser-4（2026-09-29, task 01M3Q49ZTST3XQ9DGF6AGNR0XG）

- ADR-0102 時点の判定（現在の判定は [追跡表](progress/phase-browser-acceptance.md): P4-A・P4-B・P4-C とも一部達成で、未達行は名前付き後続 task）: P4-A 未達（実隔離・出口制御・orphan 回収未接続）／P3-A 復元未達（稼働中隔離 session に未結合）／P4-B 未達（実 sink・peer role 未接続）／H3 の実装維持（機密起動は停止）／P4-C 一部接続。後続の ADR-0106 で P4-C を進めた。詳細は [phase-browser-4](progress/phase-browser-4.md)。
- 証拠: `cargo test -p task-core browser_isolation` 14 passed、`cargo test -p task-core browser_backend` 7 passed、`cargo test -p celeris-credentiald injection` 6 passed、`cargo test -p task-api restore_is` 2 passed、`cargo test -p task-api --test browser_e2e` 4 passed、`cargo test -p task-worker production_backend_route --lib` 1 passed、`cargo test --workspace` exit 0、`cargo clippy --workspace -- -D warnings` exit 0。
- attempt 3 検査: `cargo test -p task-worker browser --lib` 31 passed、`cargo test -p task-api --test browser_e2e` 4 passed、`cargo test --workspace` exit 0（2957 passed / 0 failed / 既存ignored 7件）、`cargo clippy --workspace -- -D warnings` exit 0。
- attempt 3: `CredentialUse` を起動前の必須能力へ追加し、承認済みでも未適合なら拒否。`IdentityRestore` 宣言にも P4-B 適合を必須化。API 結合テストは legacy wait の登録・承認・拒否と未消費を確認する4件へ更新。旧認証成功・実注入の証拠ではない。
- 人の回答反映: ADR-0103 で bubblewrap+subuid/subgid と固定 agent-browser 0.38.1 + 既存 harness の specialist を採用。回答待ちは解消。旧 resolve.sock と plugin bridge の秘密返却を廃止し、有効 lease を持つ同一 UID の別 worker process の実 IPC も拒否。lease 未消費・sentinel 非露出を検査。
- 未解決: P4-A の実 runtime・namespace と filtering proxy の結合、P4-B の peer role と実 CDP sink、P4-C の実 LLM 同一 task 比較が必要。P4-A/B/C の継続小タスク3件を delegate.json に提案（採用・完了は未確認）。内部 origin の追加なし。

- run `01M3QCTV524JJ41X9MSFS0756V` 最終検査: `cargo test --workspace` exit 0（2957 passed / 0 failed / 既存 ignored 7件）、`cargo clippy --workspace -- -D warnings` exit 0、`cargo fmt --all --check` exit 0。旧 IPC の秘密取得拒否・承認後拒否・lease 未消費を含む。P4-A/B/C の実適合は未達。

- run `01M3QGRCAHDK1AB4R9WBHJ9XHZ`: ADR-0105。P4-A 行を「一部達成」に更新。証拠: `cargo test -p task-worker --test browser_runtime_isolated` → 4 passed（実 bwrap + 実 browser、controller SIGKILL 後に process 残らず、starttime 一致の再起動回収）、`cargo test --workspace` → exit 0（2977 passed / 0 failed）、`cargo clippy --workspace -- -D warnings` → exit 0。環境制約: subuid が親 uid_map 外（決定 p4a-uid で同一 UID）、agent-browser 本体は host に無く同梱 browser で代替。未解決: egress 中継（netns listener → celeris-browser-egress）、production 経路切替、別 UID 実証（docs/ops 手順書）。restore は `SameUid` で拒否のまま。

- run `01M3QDM7H5RYRZF2RNCARHQWX6`: ADR-0104。実Unix/TCP DNSのegress transport（9試験）と独立 `celeris-browser-egress`（6子プロセス試験）を追加。private/IPv6/DNS/proxy/CNAME負例・IP固定・親死亡SIGKILL/waitpid回収が成功。P4-A全体は未達（worker/runtimeとの接続、別UID実証、runtime orphan、identity復元が未）。P4-B/Cの実適合も未達、H3と機密起動拒否は維持。subuid mapping は親user namespaceの範囲外でEPERM、設定変更なし。詳細・証拠は [phase-browser-4](progress/phase-browser-4.md)。
- run `01M3QGRCA745JCZTKDBDY9R83B`: ADR-0106。P4-C の静的適合登録を削除し、実測 ledger 読み込み・specialist adapter 登録・公開能力の実行時 fallback と無候補拒否を追加。実 agent-browser 0.38.1 と loopback fixture の scripted driver 三件は各7/7 case。driver は同一で ACP/Claude の実 harness protocol を使っていないため、scripted ledger は routing に使えず P4-C 完了とは判定しない。詳細は [phase-browser-4](progress/phase-browser-4.md)。
- run `01M3QJ5CY8366206MQ20A3RBPB`: `--protocol-scripted` runner が ACP・Claude・specialist の実 adapter を scripted harness process で起動。実 agent-browser 0.38.1 と loopback fixture の同一 task で各7/7。生成 ledger を worker の routing と実行時 fallback 試験に渡して成功。機密要求は未適合として拒否を維持。実 LLM 比較は ACP CLI/認証が無いため未実施。詳細は [phase-browser-4](progress/phase-browser-4.md)。
- run `01M3R8T40E9CFNGZG9WHEKMZXH` attempt 2: `python3 scripts/browser-conformance.py --protocol-scripted --fallback-scenario --agent-browser /tmp/p4c-agent-browser/package/bin/agent-browser-linux-x64 --output-dir /var/lib/celeris/workspaces/01M3QGRC6AQ81PWM1XP4C7BH45/wu/real-fallback/artifacts/p4c-real-fallback-final-v3` → exit 0。実固定版 0.38.1 + loopback fixture で三 backend 各7/7。worker の公開 `run_with_candidates` で主 ACP harness を SIGKILL し、Claude が別 session で完了。server は fallback 区間の `POST /clicked` と download を観測。代替適合なし・`CredentialUse` は明示拒否（`fallback-test.json` exit 0）。初回冷間起動の ACP navigation 失敗で runner exit 1 があり、その後の再実行は成功。`cargo test --workspace` と `cargo clippy --workspace -- -D warnings` は exit 0。実 LLM 比較は Claude 認証済みだが ACP/OpenCode CLI がないため未実施し、ADR-0009 P-34 の手順を [phase-browser-4](progress/phase-browser-4.md) に維持。`CredentialInjection`・`IdentityRestore` は拒否を維持、本番未昇格。

- このrunの最終検査: `cargo test --workspace` → exit 0（2972 passed / 0 failed / 既存 ignored 7件）。`cargo clippy --workspace -- -D warnings` → exit 0。`cargo clippy -p task-worker --all-targets -- -D warnings` → exit 0。`cargo fmt --all --check` / `git diff --check` → exit 0。機密機能の実適合・production接続の証拠ではない。

- run `01M3RCSFK5ZTC4JV0YJF17DV7C`（P4-A D3/D4）: daemon の `Started::Running` 直後に instance 別 starttime 照合付き orphan 回収を配線（`browser_startup_reap` 1 passed）。browser 起動を Supervisor/bwrap/sandboxd と egress proxy に切替。`run_with_executable` の実 chrome-headless-shell 試験で、shim → action.sock → sandboxd → proxy → ローカル HTTPS fixture 到達と禁止 flag 拒否を確認（1 passed）。resolver 未設定の起動前固定コード拒否も 1 passed。同一 host UID、agent-browser 0.38.1 本体不在、ADR-0108 D4 の channel message に対して現在の action 配送は `/session/actions` キュー。`cargo test --workspace` / `cargo clippy --workspace -- -D warnings` とも exit 0。D5 復元結合は別 WorkUnit。本番昇格・本番設定変更・内部 origin 追加なし。詳細は [phase-browser-4](progress/phase-browser-4.md)。

- run `01M3REKJA5PF78ZTD0XT42VA0N`（P4-A D5 restore 結合）: `LiveSessionRegistry` を追加し daemon で 1 つ作って supervisor（登録・削除）と API に配線。restore の HTTP に `session_id` を追加し ADR-0108 D5 の順に判定、成功時は controller にだけ渡して 204。`cargo test -p task-api --test browser_restore_live_session` → 1 passed（実 bwrap + 実 chrome-headless-shell の session に `restore_for_session` と HTTP 7 拒否経路、`open_attempts()==0`）。`cargo test --workspace` exit 0（2992 passed / 0 failed）、`cargo clippy --workspace -- -D warnings` exit 0。別 UID 実証は引き続き未解決（同一 UID のため `SameUid` で拒否、成功経路は実 runtime で未実証）。本番昇格・本番設定変更・内部 origin 追加なし。詳細は [phase-browser-4](progress/phase-browser-4.md)。
- run `01M3SEAJBYPPPNWHNQ78ZY1J0J`（P4-B gate-recheck 再試行）: integrate-gate の失敗は `browser_runtime_supervisor` の競合 2 つだった。(1) 試験側: 接続ごとの egress が消えた直後の記録を即 assert していた → 条件の待ちに変更。(2) 本番: `stop_runtime` が `bwrap-init` の非同期終了を待たずに戻っていた → 記録した本人が消えるまで待つよう修正（ADR-0108 追記）。負荷下で supervisor 10/10、attacks・shared_cdp・cdp_sink・h3_injection 各 5/5。`cargo test --workspace && cargo clippy --workspace -- -D warnings` → exit 0（3047 passed）。詳細は `docs/progress/phase-browser-4.md`。

## browser: ptrace 境界分離 launcher 設計

完了日 2026-10-01（task 01M3SPF94RDWTPWHNDEQD68VB9 の record unit `01M3WB98KYN3F3P8YMDSN6T8H8`）。

- 成果: A13（別 UID 実 process 攻撃試験、判定『部分』）の根因 — daemon が user namespace の持ち主となり `CAP_SYS_PTRACE` を持つこと — を [ADR-0115](adr/0115-browser-ptrace-owner-ns-launcher.md) に記録し、権限分離 launcher（namespace の所有を launcher 側に移し daemon から `CAP_SYS_PTRACE` を分離する設計）をまとめた。`docs/progress/browser-followups.md` の `a13-real-process` と `prod-admission-release` 項に ADR-0115 へのリンクと「実装と実 process 検証は後続段階」を追記済み。
- 証拠コマンドと結果:
  - `cargo test --workspace` → exit 0（3206 passed / 0 failed / 12 ignored）
  - `cargo clippy --workspace -- -D warnings` → exit 0（警告なし）
  - いずれも渡された `CARGO_TARGET_DIR` / `RUSTC_WRAPPER` / `SCCACHE_*` のまま実行（sccache EPERM 等の環境要因なし）。
- 未解決事項: ホスト準備（別 host UID / subuid の割当）は人の判断が必要。launcher 本体の実装は未着手。A13 の実 process 再試験（別 UID 前提）は launcher 実装後に行う。本番 admission の `CredentialInjection`・`IdentityRestore` は引き続き未解放。
- 提案: launcher 実装を独立 task として切り出し、完了後に A13 を再試験してから `prod-admission-release` の判断に戻す。

## planner check の書き方の指針（R7-10, 2026-10-02）

完了日 2026-10-02（task 01M3WZ1GFJ0YNERRNT1W0EMCQR、WorkUnit `verify-all`。兄弟 `planner-guide` / `cos-guide` と統合済み）。

- 背景: web の木の replan 24 回のうち 9 回が計画・check・条件の質に起因（2026-10-01 調査）。[ADR-0079 付記 R7-10](adr/0079-recursive-task-decomposition.md#付記-r7-10-check-の-sh-構文兄弟と衝突しない差分-check葉の大きさwebdocs-task-の-cargo受け入れ条件の範囲2026-10-02) に根拠と 5 規則を記録。
- 実装: `crates/task-worker/src/claude_code/prompt.rs` の `PLANNER_CHECK_GUIDANCE` に 5 規則（(1) `/bin/sh`/dash 限定の構文、(2) 段の全 unit の許可パスを除外する範囲外差分 check、(3) 葉は 1 run に収まる大きさ、(4) `web/`/`docs/` だけを変える task は `cargo test --workspace` の代わりに `crates/` 無差分検査、(5) 受け入れ条件・差分 check の範囲に ADR・記録の置き場所を最初から含める）を追記。`crates/task-worker/src/claude_code/tests.rs::planner_prompt_has_the_check_writing_section` で各規則の文言が /2・/3 の planner プロンプトに 1 回ずつ出ることを確認。
- CoS 側: `crates/task-worker/src/preamble.rs` の `actions_instructions()` に (4)(5) と同内容の 2 文を追記（`web/` や `docs/` だけを変える task の cargo 代替検査、ADR・記録の置き場所を acceptance の範囲指定に含める）。`crates/task-worker/src/preamble/tests.rs` で両文がそれぞれ 1 回だけ出ることを確認。
- 証拠コマンドと結果（このWorkUnitで実行）:
  - `cargo test --workspace` → exit 0（全 crate `test result: ok`、失敗 0）
  - `cargo clippy --workspace -- -D warnings` → exit 0（警告なし）
  - いずれも渡された `CARGO_TARGET_DIR` のまま実行。
- 未解決事項: なし。本番 host の操作、設計原則・Phase 順の変更は行っていない。

## reviewer に人の決定・回答と決定的 check の結果を渡す（ADR-0117）

完了日 2026-10-02（task 01M3WZ1GFBPQ8T699ZF3Y66SJ3 の record unit `verify-all`）。

- 経緯: [ADR-0117](adr/0117-review-human-decisions-and-check-results.md) に基づき、`task-worker`（ReviewRequest の拡張と review プロンプトの節）と `task-dispatch`（spawn_review が対象 task と祖先の回答済み決定・決定的 check の verdict を集めて渡す）を実装済み（D1/D2 実装、D3「acceptance 書き換えの入口」は見送り）。この WorkUnit はその統合後の workspace 全体検査。
- 証拠コマンドと結果:
  - `cargo fmt --all -- --check` → exit 0（差分なし）
  - `cargo clippy --workspace -- -D warnings` → exit 0（警告なし）
  - `cargo test --workspace` → exit 0（全 118 テストバイナリで `test result: ok`、`0 failed`。主要クレートの内訳: task-core 620 passed、task-dispatch 498 passed、task-ops 382 passed、task-worker 660 passed / 4 ignored。ブラウザ系の実プロセス試験を含め失敗・flake 無し）
  - いずれも渡された `CARGO_TARGET_DIR` / `RUSTC_WRAPPER` / `SCCACHE_*` のまま実行。外部ネットワークへのアクセスなし。
- 変更範囲: このWorkUnit自体はコード変更なし（検査と本記録のみ）。実装済みの変更は `crates/task-worker/src/claude_code/{prompt.rs,tests.rs}`、`crates/task-worker/src/protocol.rs`、`docs/protocol/worker-protocol.schema.json`、`crates/task-dispatch/src/{review.rs,review/tests.rs,dispatcher/review_spawn.rs,dispatcher/tests/review.rs}`（別 WorkUnit `worker-prompt`・`dispatch-context` でコミット済み、上記 commit に記録済み）。
- 未解決事項: D3（人の決定の note から `PATCH acceptance` を提案する入口）は見送り、ADR-0117 に記録済み。reviewer が実際に人の決定を優先して合格させる end-to-end 実例（web root final review のような実 run での再現確認）はこの WorkUnit の範囲外（unit test レベルでの検証のみ）。
- merge-main（2026-10-02）: 最新 main（33e0a6aa、R7-10 planner check 指針を含む）を本ブランチに merge。`docs/PROGRESS.md` の衝突は上の 2 節（R7-10 を先、ADR-0117 を後）を両方残して解消。コード（`crates/task-worker/src/claude_code/{prompt.rs,tests.rs}`）は自動 merge。ADR 番号 0117 は main の最大 0115 と重複なし。`cargo fmt --all -- --check` / `cargo clippy --workspace -- -D warnings` / `cargo test --workspace` はいずれも exit 0。

## repair objective の許可範囲受け渡し

完了日 2026-10-02（work unit `wire`）。段階統合は同じ phase の非 repair・非 integrate unit、final review は task の全 unit と task acceptance から、変更してよい paths と `git diff` を含む check を集めて repair objective に渡す。重複を除き、辞書順に並べる。範囲外の失敗はファイルを直さず `plan_issue` で報告する指示が入り、既存の `worker_finish` 経路で replan に進むことを確認した。delivery repair は対象外。

- 証拠: `cargo test -p task-dispatch integration_check_failure_is_repaired_when_classified`、`cargo test -p task-dispatch final_review_repair_includes_all_unit_paths_and_task_diff_checks`、`cargo test -p task-dispatch a_plan_issue_checkpoint_triggers_a_replan_and_v2_is_adopted` は各 exit 0。
- `cargo test --workspace` は通常 sandbox で初回 exit 101。`instance_handoff` 5 件が user namespace 作成の `Operation not permitted` により失敗。範囲外のテストは変更せず、ホスト権限で `cargo test -p celeris --test instance_handoff` を再実行して 8/8 通過し、同条件の `cargo test --workspace` は exit 0。
- `cargo clippy --workspace -- -D warnings` は exit 0（警告なし）。

### 統合後（adr・core・wire merge 済み）の再検証 — 2026-10-02（work unit `verify-land`）

段階 build の統合ブランチ（HEAD `0892b0c2`、adr・core・wire 3 葉を含む）で、前回 integrate-build が落ちた check（兄弟葉 adr の docs/ 限定差分検査が core・wire の crates/ 差分に当たっていた点、`cargo test --workspace` が sandbox の user namespace 制限で落ちうる点）を差し替えた上で検査をやり直した。コードは変更していない。

- `cargo test -p task-core execution` → exit 0（143 passed / 0 failed）
- `cargo test -p task-dispatch repair` → exit 0（8 passed / 0 failed。`final_review_repair_includes_all_unit_paths_and_task_diff_checks` を含む）
- `cargo test -p task-dispatch final_review_repair` → 同上のフィルタに含まれ exit 0
- `cargo clippy --workspace -- -D warnings` → exit 0（警告なし）
- `cargo fmt --all -- --check` → exit 0（差分なし）
- `cargo test --workspace` → sandbox のまま 2 回実行し、いずれも exit 0（3209 passed / 0 failed / 12 ignored、`instance_handoff` 8 件を含め userns 制限による失敗なし）。host 権限での再実行は不要だった。flaky な再現は 2 回とも無し。
- `git status` / `git diff --stat` ともに変更なし（crates/ と docs/adr/ は touch していない。本行の追記のみ）。

### 最新 main（ADR-0117）取り込み後の検査 — 2026-10-02（work unit `land-main`）

`main` の `5f14fe7480f1512a0a62c35cf41c1fedb11a6944` を merge commit `42cefd87986368de6379a5afed4e4eabab2f7a41`（親: `112ed0409bc554b29f4c624017200d95708236d5`, `5f14fe7480f1512a0a62c35cf41c1fedb11a6944`）で取り込んだ。`git merge-tree --write-tree main HEAD` の事前確認では `docs/PROGRESS.md` だけが衝突し、`crates/task-dispatch/src/dispatcher/tests/review.rs` は自動 merge。PROGRESS は ADR-0117 節を先、repair 許可範囲節を後に両方残して解消した。

- `cargo fmt --all -- --check` → exit 0。
- `cargo clippy --workspace -- -D warnings` → exit 0（警告なし）。
- `cargo test -p task-core -p task-dispatch -p celeris` → exit 101。unit tests は task-core 622/622、task-dispatch 499/499、celeris lib 214/214 pass。`celeris --tests` の `instance_handoff` は 8 件中 5 件失敗。うち3件は ADR-0095 worker db guard の user namespace 作成が `Operation not permitted`、2件は handoff dispatch/standby 起動待ち失敗。失敗はこの task の許可範囲外の sandbox 制約によるため、テスト・実装を変更せず報告する。
- 追加の `cargo test -p task-core -p task-dispatch -p celeris --lib` → exit 0（合計 1335 passed / 0 failed）。
- 追加の `cargo test -p task-core -p task-dispatch -p celeris --tests -- --skip instance_handoff` → exit 101（`--skip` は個別 test 名に対するフィルタのため binary を除外せず、上記 `instance_handoff` 5 件で失敗）。
- `git merge-base --is-ancestor main HEAD` → exit 0。feature code は変更せず、検査記録のみ追記。

## browser: ptrace 境界分離 launcher 実装・実 process 実証完了

完了日 2026-10-02（task 01M3VFQK2ZSPJ89VDA1F4FMA2G、WorkUnit `real-evidence` / `close-out`）。

- 成果: [ADR-0115](adr/0115-browser-ptrace-owner-ns-launcher.md) に沿って `celeris-browser-launcher` を実装し、人が host 準備手順（[browser-launcher-host-setup.md](ops/browser-launcher-host-setup.md)）どおりに `celeris-browser` user・subuid/subgid・systemd socket/service を整えた環境で、launcher 経由の実 Chrome について daemon UID からの ptrace・`/proc` 読取り拒否を実証した。既存の daemon 所有 runtime（same-uid / subuid wrapper）経路は維持し、launcher 経由は設定（`[browser] runtime` / `launcher_socket`）で選択する。
- 証拠（host の人の実行、詳細は [phase-browser-4.md](progress/phase-browser-4.md) の該当 run 記録）:
  - `CELERIS_LAUNCHER_TESTS=require cargo test -p task-worker --test browser_launcher_ptrace -- --nocapture` → **exit 0（1 passed、0 failed、0.15 s）**。
  - 正の対照: 同 UID 子プロセスへの `PTRACE_ATTACH=0`／`PTRACE_DETACH=0`（検査手段自体が機能することの確認）。
  - launcher 観測・Chrome: `owner=Some(296608)`（Chrome userns owner、subuid）、`NS_GET_OWNER_UID(launcher)=Some(296608)`、daemon UID からの同 NS open は `errno=Some(13)`（EACCES）。
  - `verify_isolation=Ok (launcher isolation_ok=true, CapEff=0000000000000000 NoNewPrivs=true)`。
  - daemon UID 1001 の別プロセスからの攻撃: `PTRACE_ATTACH Chrome pid: errno=Some(1)`（EPERM）、`strace -p` は `exit=Some(1)`／`Operation not permitted`、`/proc/<pid>/environ`・`/proc/<pid>/mem` ともに `errno=Some(13)`（EACCES）。
  - check runner（db_guard namespace、`/proc/self/uid_map`=`1001 1001 1`）からの再実証でも同じ構成で **exit 0（1 passed、0.14 s）**。namespace 越しに subuid が `4294967295` と写る分だけ試験側の map 判定を修正済み（launcher 側の不具合ではない）。
  - 既存の daemon 所有 runtime 経路: `cargo test -p task-worker --test browser_runtime_isolated` → **exit 0（4 passed、1 ignored）**、壊れていない。
  - `cargo test --workspace && cargo clippy --workspace -- -D warnings` → exit 0（close-out run、下記参照）。
- 手順書・証跡リンク: host 準備は [docs/ops/browser-launcher-host-setup.md](ops/browser-launcher-host-setup.md)、試行錯誤と各 run の journal・eprintln 全文は [docs/progress/phase-browser-4.md](progress/phase-browser-4.md)（ADR-0115 launcher 節以降）。
- 未解決事項:
  - この host は LXC 内のため、試験 runner（UID 1001）の親 namespace map は初期 namespace の `0 0 4294967295` ではなく `0 100000 1001 / 1001 1001 1 / 1002 101002 64534 / 65536 165536 262144`。検証は db_guard namespace の外・この map のもとで行った。
  - Chrome stderr の `category=other-startup-error`（journal に 4 行）の中身（無害な起動時警告か）は未確認。
  - 機密能力（`CredentialInjection`・`IdentityRestore`）はこの task では解放していない。解放は後続 task `01M3VFQZ2TX3W0KTDQHKCAVJR6` / `01M3WV4BFJ71J9ZWJ020MP2Z4K` で判断する。

### 最新 main（7f3482a3）取り込み — 2026-10-02（work unit `land-main`）

`main` の `7f3482a3` を merge で取り込んだ（rebase なし）。衝突は 2 ファイル。
- `crates/task-worker/tests/browser_shared_cdp.rs`: main 側 5eb666f6（R7-12）の `ISOLATION`・`PREFLIGHT_TIMEOUT`・`probe.err` の excepthook・WebSocket frame の読み切り（`recv_exact`）を土台にした。そこへこのブランチの 97eb5194（接続から CDP 応答までを 1 試行とし、期限 55 秒の中で再試行する）を載せた。`find_browser`・`preflight`・`run_bounded` と、launcher 用の `userns: UsernsMode::Unshare` は自動 merge でそのまま残っている。
- `docs/PROGRESS.md`: main の R7-10・ADR-0117・repair 許可範囲の節を先に、この節を含む launcher 節を後に置き、どちらも残した。
- 証拠: `cargo build -p task-worker --bins` exit 0。`cargo fmt --all -- --check` exit 0。`cargo clippy --workspace -- -D warnings` exit 0。`cargo test --workspace` exit 0（3261 passed / 0 failed / 12 ignored）。`CELERIS_ISOLATION_TESTS=require cargo test -p task-worker --test browser_shared_cdp` を 3 回実行し、3 回とも exit 0（2 passed）。flaky は出なかった。launcher の実 host 試験は再実行していない（人の側で済み）。
