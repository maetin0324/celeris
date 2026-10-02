# PROGRESS — taskd

## browser: ADR-0115 権限分離 launcher

run `01M3X8SRB3X08AXW8WK5PY7P9N` で launcher 実装・設定・host unit/手順書を統合後に検査。`cargo fmt --all -- --check` と `cargo clippy --workspace -- -D warnings` は exit 0。workspace test は sandbox の user namespace probe が `EPERM` となり、ADR-0095 DB guard を使う `instance_handoff` 5 件が失敗して exit 101（`CELERIS_ISOLATION_TESTS=skip` を付けても同じ。skip は browser isolation 試験だけに適用）。launcher ptrace 試験は `celeris-browser` user と `celeris-browser-launcher.socket` が存在しないため `SKIPPED (not passed)`。host 管理者に [browser-launcher-host-setup.md](ops/browser-launcher-host-setup.md) の準備を依頼し、準備後に実 process 証跡を追加する。機密能力は未解放。本節の詳細は [phase-browser-4](progress/phase-browser-4.md)。

- 2026-10-02: phase3 control flaky 再実行は ADR-0095 の user namespace 拒否で `cargo test --workspace` と `api_scenarios` が失敗、clippy は pass。詳細は [phase-R.md](progress/phase-R.md)。

現在地: **構造リファクタリング完了（2026-09-30、下記）。Phase 119、Phase E6、Phase F4b まで本番反映（release c51837427ac5、schema 28）。F5-1 dogfood の 3 回目を準備中。Browser capability Phase 1〜4 は追跡表どおり P4-A/B/C 一部達成で、別 host UID 実証と本番機密能力解放は後続（2026-10-01 にリファクタ後の main へ取り込み中）**。以後の追記は `docs/progress/phase-F.md` へ。

## 受信箱と通知の 2 系統（2026-10-02、task 01M3YFCJKMNWQ13HRS52M5BSWW、ADR-0133、WorkUnit verify）

- 完了日: 2026-10-02。全葉（adr / inbox-model / notify-store / notify-feed / api / outbound / gui-compat）完了、
  ADR-0133 の状態を「実装済み」に更新（web 葉は人の決定 `ui-overlap = c` で UI/UX task
  `01M3XTCNKMQBCHKSZ7Y1GF6ZM4` へ `superseded`）。詳細・証跡は
  [phase-inbox-notifications.md](progress/phase-inbox-notifications.md)。
- 証拠（HEAD `0bc7ca75a5a8`、schema 40）: `cargo fmt --all -- --check` exit 0。
  `cargo clippy --workspace --all-targets -- -D warnings` exit 0。`cargo test --workspace` exit 0
  （**3,294 passed / 0 failed**、121 バイナリ + doctest、ignored は既存の手動試験のみ）。再実行 1 回で
  `task-worker` の `scratch::tests::wrapper_runs_the_compiler_directly_when_the_server_is_unreachable` が
  ETXTBSY で単発失敗（単体実行では再現せず、この task の範囲外の既知の flaky）。今回の関連 crate 再試験は
  `--lib` で全 pass。integration test 込みでは `instance_handoff.rs` の 5 件が失敗: 3 件は worker DB guard の
  user namespace probe が sandbox の `Operation not permitted`、2 件は handoff の wall-clock 条件（既知の flaky）。
  この 3 件に既存の `CELERIS_ISOLATION_TESTS=skip` 分岐は無く、失敗として記録（詳細は上記 progress 文書）。
- 追従（rules-wire / notify-status / sync-main、2026-10-02）: verify 時点の未解決 2 件は閉じた。1) inbox-rules
  （task `01M3YF3NS2EGTZD2BBWNPG1K28` の `788e5cc0`・`739cd209`）を `cherry-pick -x` で取り込み `human_inbox` に結合
  （試験 `auto_close_drops_meaningless_items_and_keeps_failed_needing_a_decision`、規則の重複実装なし）。2) `GET /api/v1/notify`
  に D6 の 4 設定値と経路別最終送信時刻（試験 `get_notify_status_exposes_route_settings_and_last_successful_sends`）。
- sync-main: 最新 main（`5d6df9f3`、続けて `14b052ea`）を `git merge`（衝突は本ファイルだけ、両方の節を残して解消、`merge-tree` exit 0）。
  全 celeris/* の走査で 0038〜0040 が他ブランチ使用中のため `0040_feed_notices.sql` を `0041` へ `git mv`
  （`SCHEMA_VERSION = 41`、`RESERVED_VERSIONS = [38, 39, 40]`）。inbox-rules の「人は failed を cancel できる」に合わせ
  e2e `phase7_scenarios` の期待を更新。証拠: `cargo fmt --all -- --check` exit 0、
  `cargo clippy --workspace --all-targets -- -D warnings` exit 0、`cargo test --workspace` exit 0（**3,381 passed / 0 failed**）。
- 本番 host で人が確認・設定する手順は [docs/ops/inbox-notifications.md](ops/inbox-notifications.md)。

## codex・opencode への skill の付属ファイルと段階的な読み込み

- 実装・記録完了日: 2026-10-02。[ADR-0127](adr/0127-skills-native-delivery.md) は実装済みに更新。codex・acp では mount した skill を `.agents/skills/` に付属ファイルごと届け、`AGENTS.md`・前置きは一覧だけにした。実機の記録と再実行手順は [phase-skills-progressive.md](progress/phase-skills-progressive.md)。
- 検査: `cargo test -p task-worker --lib skills` → 31 passed、`cargo test -p task-worker --lib --no-run` → exit 0、`cargo test -p celeris --test ui_ux_skills_delivery` → 4 passed、`cargo fmt --all -- --check` → exit 0、`cargo clippy --workspace -- -D warnings` → exit 0（警告なし）。
- 未解決: codex の実 LLM は skill を使う判断まで記録したが、bubblewrap の socket ディレクトリ検査で `SKILL.md` と付属ファイルを読めなかった。opencode は `debug skill` で検出したが、一時 HOME に認証が無く LLM は起動していない。worker sandbox では user namespace を使う実 runtime 試験も走らせられない。付属ファイルの LLM 読込は手順に従う環境で再確認が必要。

## 試験で CPU を焼く負荷をかけない規則（2026-10-02、task 01M3Y4AV7801NSXB6FD698QZHW、WorkUnit rule-docs）

- 完了日: 2026-10-02。`CLAUDE.md`「作業の進め方」に規則を 1 行追加、[docs/testing.md](testing.md) を新設（禁止の理由と、時計の差し替え・出来事待ち・SIGSTOP/SIGCONT・遅延フックの 4 方法を既存試験の例つきで記載。`tokio::time::pause` はリポジトリに例が無く擬似例）。`scripts/dev/stress-e2e-phase3.sh` を `git rm`、`docs/progress/phase-browser.md` と本ファイルの実行案内を削除の一文に置換（過去の結果行は残す）。
- 証拠: `grep -q 'SIGSTOP' CLAUDE.md && grep -qE 'CPU を焼' CLAUDE.md && test ! -e scripts/dev/stress-e2e-phase3.sh && ! git grep -n 'stress-e2e-phase3.sh' -- scripts crates .claude` → exit 0。`git diff --name-only 764a737d -- crates docs/DESIGN.md` → 出力なし。cargo は crates/ を変えないため未実行。
- 未解決: `docs/architecture-map.md` に試験指針の索引は無いので追記しない。planner/worker 指示への追記は別 WorkUnit（prompt-rule）。

## release 準備失敗の切り分け（0d438ec1）

- 2026-10-02 の prepare.log（release SHA `0d438ec19d9a474c5b82507cefd0d9e63846d0d6`）を確認。`cargo-fmt-check`、`cargo-test`、`cargo-clippy`、`cargo-build`、GUI の `pnpm-install` / `pnpm-typecheck` / `pnpm-test` / `pnpm-build` / `pnpm-mobile-audit` / `pnpm-e2e-mock` と web の各 pnpm step はすべて exit 0。`source-size-report` も exit 0。
- release.log は、SHA `95ac1644` の `release.sh` が子プロセスへ lock の fd を漏らし、親スクリプト終了後も `tar` とともに lock を保持したため、新しい `release.sh` が stale lock を `.lock-release.leaked-20261002T120143Z` へ退避して新 lock を取得したことを記録している。今回の `web-bundle` tar は non-blocking step として失敗したが release 自体は ready になった。その release dir に `bin/celeris` が無く、続く verify は `missing .../0d438ec19d9a/bin/celeris` で失敗した。
- この task の変更（`CLAUDE.md`、`docs/testing.md`、stress 台本削除、`prompt.rs` とその試験）は検査規則・文書・指示の変更で、release の binary build/package 経路を変更していない。release log でも `cargo-build` は exit 0 で、欠落の前に独立した web tar failure が記録されている。よって今回の bin 欠落はこのブランチ変更と無関係な release 準備上の問題と判断し、コード修正は不要。
- 人が再実行する手順: まず `ps` で並走中の `release.sh` が無いことを確認し、その後、新しい SHA を指定して `scripts/selfdeploy/release.sh <新しい SHA>` と `scripts/selfdeploy/verify.sh <新しい SHA>` を実行する。release dir に `bin/celeris` が存在すること、および verify が `ok` になることを確認する。本番 host の `~/.local/celeris/releases`、`systemctl --user` 等は読み取り確認だけとし、この記録作成時には変更・再実行していない。
- 本記録作成時の検証: `cargo fmt --all -- --check` → exit 0。`cargo clippy --workspace -- -D warnings` → exit 0。

## 構造リファクタリング完了（2026-09-30、ADR-0079 / ADR-0082 / ADR-0083）

inline test の外出しと責務分割を完了した（worktree、main 未 merge）。前後 LOC 表と 2,000 行超ファイルの分類は [phase-structure-refactor.md](progress/phase-structure-refactor.md)。

- 証拠（最終 HEAD aaa6a1ca）: `cargo test --workspace` → exit 0、2,935 passed / 0 failed / 8 ignored、4 分 57 秒。`cargo fmt --all -- --check` と `cargo clippy --workspace -- -D warnings` → exit 0。test 属性数（`git grep -hE '#\[(tokio::)?test'`）は f34f060 = 2,907 → HEAD = 2,907。`git diff --quiet f34f060 HEAD -- crates/task-core/migrations docs/api/v1 docs/protocol config` → exit 0（互換性差分ゼロ）。`source-size-report.py --strict` → 0 active warning / 1 excepted。
- 結果: Rust inline test 100,831 → 6,160 行。2,000 行超の production は 10 本 → 1 本（`task-api/src/types.rs`、理由付き例外）。
- 未解決: main への取り込みと本番昇格は人。`paperqa.rs`（1,948）・`dispatcher.rs` facade（1,868）・`execution_plan/validation.rs`（1,739）が閾値に近い。
- 提案: 新しい責務は分割後の子 module に置き、facade に戻さない。`release.sh` の source-size-report 表示段で閾値接近を毎回確認し、2,000 行を超えたら同じ手順（test 外出し → 責務移動）で割る。

詳細な履歴と証跡は下記の分割ファイルを参照。既存の `docs/PROGRESS.md` 参照はこの目次を入口として維持する。

## 目次

- [Browser capability Phase 1](progress/phase-browser.md) — ADR-0078、既存 harness + agent-browser、管理者 grant・session・監査・dashboard 導線。最新 main 再統合後の gate 2026-09-28（Rust 2678 passed、GUI 1173 passed、mobile-audit 0 violations）。本番未昇格。2026-10-02 追記: `scripts/dev/stress-e2e-phase3.sh` で phase3_control の flaky 修正（`dafffeb1`）を負荷下 2 回（各 20 serial + 8 parallel）で検証、全 pass。fix-stress-build は人の介入を受け、共用 host の既定負荷を焼き 2 本・nice -n 19・300 秒・5 serial + 2 parallel に変更し、背景 cargo 負荷を opt-in 化。指定の `time sh scripts/dev/stress-e2e-phase3.sh` を既定値のまま 1 回実行し exit 0（serial 5/5、parallel 2、real 16.970s）。`scripts/dev/stress-e2e-phase3.sh` は 2026-10-02 に削除した（CPU を焼く負荷は共用 host を巻き込み再現も確率的なため。[docs/testing.md](testing.md)）。
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

## 担当の無い root task の delivery（ADR-0121、2026-10-01）

- 振り直し: 旧番号 0099 は main の browser-phase3-control-lease（docs/adr/0099-browser-phase3-control-lease.md）と衝突したため main 統合後に 0117 へ振り直したが、refs 全体の走査で 0117 が使用済みと判明した。main・全 refs/heads/celeris/*・refs/remotes の docs/adr を git ls-tree で走査すると 0116/0117/0118 は使用済み、0119 は未使用だったため、2026-10-02 に ADR-0121 へ再度振り直した。
- 統合 HEAD: `c5f57803`（verify-all worktree）。ADR-0121 を [ADR-0121](adr/0121-root-delivery-without-assignee.md) に記録。部署は root assignee → 有効計画の planner → root 配下の子 task / WU run の実担当票 → 案件設定の既定部署の順に解決し、曖昧な担当推測をしない。対象 root の delivery 見送りは `DeliverySkipped` event と inbox attention に理由を残す。子 task と対象外案件は従来どおり通知・delivery 対象外。
- `cargo test --workspace` → exit 101。大半の test 群は成功したが、`celeris` の `instance_handoff` 5 件が失敗。4 件は sandbox 内で user namespace が許可されず ADR-0095 worker DB guard probe が失敗したもの、2 件はその後の dispatch/standby 待機失敗（5 failures total）。delivery 関連の test 群は通過。workspace 全件成功とは扱わず、user namespace が利用可能な環境で再実行が必要。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `rg -n 'unwrap\\s*\\(' crates/task-ops/src/delivery.rs` → 一致なし。production `delivery.rs` に `unwrap()` は無い（test 用 unwrap は別ファイル）。
- 未解決: sandbox 制約を外した統合環境で workspace test を再実行して全件成功を確認すること。ブランチの production DB 接続・実行は行っていない。
- ADR 番号の再走査（2026-10-02）: main・全 refs/heads/celeris/*・refs/remotes の 130 refs を `git ls-tree` で確認し、0116/0117/0118 は使用済み、最小空き番号 0119 を選択。main に migration 0037 は無く、migration 番号は変更なし。参照と ADR-0051 のリンクを ADR-0121 に更新した。task-api/task-core/task-worker の `UPDATE_SCHEMA=1` schema 整合テストは成功。`corepack pnpm@11.27.0 -C gui gen:types` は pnpm の依存確認が cache SQLite を開けず exit 1。型生成は実行できなかったが、生成物 types.ts の該当 description を schema と同じ ADR-0121 表記に同期した。`cargo clippy --workspace -- -D warnings` は exit 0。`cargo test --workspace` は exit 101、`instance_handoff` 5 件が sandbox の user namespace `Operation not permitted` で失敗（delivery 関連以外の環境依存失敗）。
- delivery skipped の inbox query 向けに migration `0037_events_delivery_skipped_index.sql` を追加し、`idx_events_delivery_skipped` 部分 index を作成。問い合わせの predicate は index と同じ式を使い、`latest_delivery_skipped_rows_uses_partial_index` が EXPLAIN QUERY PLAN の index 使用と task ごとの最新 1 件を確認する。schema version 37（`SCHEMA_VERSION = 37`）になる。
- この migration の昇格は celeris を stop → 新バイナリで start とし、起動時に migration が実行される。index は追加のみで、旧バイナリも残存 index 自体は利用せず動作できる。ただし schema 37 を開く旧バイナリは SchemaTooNew になるため、バイナリを戻す場合は migration 前の DB backup も戻すこと。詳細と任意の `DROP INDEX` は [ADR-0121](adr/0121-root-delivery-without-assignee.md) に記録。
- 振り直し仕上げ（renumber-adr、2026-10-02 続き）: main（`29e2d768`）は既に HEAD の祖先（`git merge-base --is-ancestor main HEAD` → exit 0）、main に ADR-0121 や migration `0037` は現れていないため番号の再振り直しは不要。`docs/adr` の重複なし（`0119-root-delivery-without-assignee.md` 1 件、`0117-review-human-decisions-and-check-results.md` 1 件）、全 refs/heads・refs/remotes の `git ls-tree` 走査でも `0119-root-delivery-without-assignee.md` の使用は本ブランチのみ。`git grep -n 'ADR-0117'` を対象 crates/ADR-0051/docs/api/v1/gui types.ts に実行して一致なし（exit 1）。`crates/task-dispatch`・`crates/task-worker`・`docs/protocol`・`docs/adr/0117-review-human-decisions-and-check-results.md` は `git diff main` で差分ゼロ。
  - `corepack pnpm@11.27.0 -C gui gen:types` → exit 0（今回は pnpm store 事前確認が通った）。実行後 `git status --short` は無変更で、前回手動同期した `types.ts` の ADR-0121 表記と生成物が一致することを確認。
  - `UPDATE_SCHEMA=1 cargo test -p task-core -p task-api -p task-worker` → 全 test group `0 failed`（schema 固定文字列テストを含む）。
  - `cargo check --workspace --tests` → exit 0。
  - `cargo clippy --workspace -- -D warnings` → exit 0。
  - `cargo test -p task-core -p task-ops -p task-api` → 全 test group `0 failed`（task-core 624 件・task-ops 395 件を含む）。
  - `cargo test --workspace` → 今回は exit 0、全 118 test group `0 failed`（計 3,228 passed / 0 failed / 12 ignored）。`crates/celeris/tests/instance_handoff.rs` の `a_stale_heartbeat_promotes_the_standby` は 60 秒超の低速（userns probe 待ち）だったが最終的に `ok`。前回 run で見られた sandbox user namespace 拒否による失敗はこの run では再現せず、workspace 全件成功を確認した。

## Root delivery 部署 fallback 再検証（verify-perf、2026-10-01）

perf-query を含む統合 HEAD `dd413c5e` で指定 gate を再実行。`cargo test --workspace` → exit 101（3,021 passed / 5 failed / 0 ignored）。失敗は `crates/celeris/tests/instance_handoff.rs` の5件で、worker DB guard が user namespace の生成を `Operation not permitted`（ADR-0095）で拒否した後、dispatch / standby の待機試験も失敗。delivery / inbox の試験を含む他の試験は通過。前回と同じ sandbox 制約であり、コード起因の不具合を示す結果ではない。全件列挙 `cargo test --workspace -- --list` は 3,026 件。

`cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。

workspace 全 test の成功は未確認。user namespace を利用できる環境で再実行が必要。

## Root delivery 最終 workspace 検証（verify-final、2026-10-01）

- `cargo test --workspace` → exit 101（約3,029 passed / 5 failed / 0 ignored、`instance_handoff` 実行約60秒）。失敗5件は `crates/celeris/tests/instance_handoff.rs`。3件で worker DB guard probe が user namespace の `Operation not permitted` となり、2件の dispatch / standby 待機試験も同テスト群内で失敗した。delivery / inbox の試験を含む他の試験群は通過し、この task の差分に起因する失敗は確認されなかった。
- `cargo test --workspace -- --list` → exit 0、3,303 tests 列挙。
- `cargo clippy --workspace -- -D warnings` → exit 0（41.53秒）。
- test の受け入れ条件は未達。user namespaces が利用可能な環境で workspace test の再実行が必要。

## Root delivery 取り込み最終検証（verify-land、2026-10-02）

main（HEAD `2bd7df3b`）への merge-renumber・delivery-index 統合後の最終ゲート。user namespace が使える環境で実行し、前回 run が未解決としていた `instance_handoff` の失敗を含め全件成功を確認した。

- `cargo fmt --all -- --check` → exit 0。
- `cargo test --workspace` → exit 0。3,221 passed / 0 failed / 0 ignored（`--list` 相当の doctest 等含む計 118 test binary、`instance_handoff` も含めて全件成功）。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo test -p task-core delivery_skipped` → exit 0、2 passed（`latest_delivery_skipped_rows_uses_partial_index` が `idx_events_delivery_skipped` の EXPLAIN QUERY PLAN 使用と task ごとの最新1件を確認）。
- `git merge-tree --write-tree --name-only HEAD main` → exit 0、衝突ファイル名の出力なし（tree `229e6d41c27f0fb716ef3e17faef6fc8c166d0de` のみ）。main を fast-forward 可能な形に近づけた状態を確認。
- `rg -n 'ADR-0099' docs/ crates/` → 残存参照はすべて main 既存の browser-phase3-control-lease（`docs/adr/0099-browser-phase3-control-lease.md`）向けで、root delivery の旧番号参照は無い。`docs/adr/0121-root-delivery-without-assignee.md` が振り直し後の ADR。
- GUI（`corepack pnpm@11.27.0 -C gui install --frozen-lockfile` → exit 0 の後）: `pnpm typecheck`（`react-router typegen && tsc -b`）→ exit 0。`pnpm test`（vitest run）→ exit 0、85 test files / 1,250 tests passed。
- 負荷による flake は今回発生しなかった（追加の待ち上限変更は不要）。
- 受け入れ条件 0〜3 すべて満たした。取り込み可能性の確認はここまでで、実際の main への merge は celeris の統合工程（integrate-reverify）が行う。

## Root delivery 振り直し後の最終検証（verify-land2、2026-10-02）

前回 run は main が本ブランチの先に進んでいて（Web GUI SPA の大規模統合 `e730f056` 等）`git merge-base --is-ancestor main HEAD && git merge-tree --write-tree HEAD main` が exit 1 で失敗した。本 run で main を 2 回 merge して追従（1 回目: `docs/PROGRESS.md` の目次追記どうしの衝突のみ、両節を残して解消。main がさらに進んだため 2 回目: user systemd bus 遮断・db_guard の host config 読み取り専用化〈ADR-0095 付記〉の取り込みで crates/ に差分、衝突なし）、HEAD を `7c2147f6` にした。

- `git merge-base --is-ancestor main HEAD` → exit 0（main `ea86af63` は HEAD の祖先）。
- `git merge-tree --write-tree HEAD main` → exit 0、衝突なし。
- `cargo fmt --all -- --check` → exit 0。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo test --workspace` → exit 0。3,239 passed / 0 failed / 0 ignored（118 test binary、doctest 含む）。`instance_handoff` 8 passed / 0 failed（`a_stale_heartbeat_promotes_the_standby` は 60 秒超だが `ok`）。
- `cargo test -p celeris --test instance_handoff` を単独で 3 回実行 → 毎回 8 passed / 0 failed（60.4〜60.5 秒）。前回 reviewer が懸念した失敗は本 run・本 HEAD では再現せず、workspace 全体実行でも単独実行でも安定して成功する。負荷依存の既存 flaky と判断する根拠も無く、単純に全件成功。
- GUI（`gui/`、pnpm@11.27.0）: `install --frozen-lockfile` → exit 0。`pnpm typecheck` → exit 0。`pnpm test`（vitest run）→ exit 0、85 files / 1,250 tests passed。
- web（`web/`、pnpm@12.6.0、main 統合で新規に加わった SPA）: `install --frozen-lockfile` → exit 0。`pnpm typecheck` → exit 0。`pnpm test`（vitest run + node --test server）→ exit 0、Vitest 24 files / 178 tests passed、Node test 41 passed。
- 2 回目の main merge は `crates/task-worker/src/{db_guard,preamble,claude_code/prompt}.rs` 等に差分があったため gui/web の再検査は不要と判断（`git show --stat` で `gui/`・`web/` への変更が無いことを確認）し、Rust 側のみ再実行した。
- 受け入れ条件 0〜2 すべて満たした。取り込み可能性の確認はここまでで、実際の main への merge は celeris の統合工程（integrate-reverify）が行う。

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

## 時間依存試験の決定化（完了日 2026-10-02、[ADR-0125](adr/0125-deterministic-time-tests.md)）

- 方式: (1) `a_wait_parks_the_task_polls_and_resumes_as_a_continuation` は注入時計 + 状態/event 待ち、(2) `every_cargo_path_uses_the_scratch_target_dir` は状態/event 待ち、(3) `command_checks_are_re_executed_in_workspace` は試験 workspace が timeout 結果を注入、(4) `real_broker_browser_injection_receipt_and_origin_guards` は CDP ready 待ち + 試験専用遅延、(5) `controller_kill_leaves_no_runtime_processes` は SIGSTOP/SIGCONT stutter + 同期フック + process 終了待ち、(6) `tick_prunes_the_oldest_terminal_workspace_and_records_an_event` は `target/` 消滅と `WorkspacePruned` event 到着を 1 つの待ちループで待つ（状態/event 待ち）。原因、意図を保つ条件、詳細な方式は [ADR-0125](adr/0125-deterministic-time-tests.md) と[調査一覧](progress/time-dependent-tests.md)を参照。
- (1) `a_wait_parks_the_task_polls_and_resumes_as_a_continuation` — 原因: 旧 `tick_until` が 200 / 400 tick の上限と各 tick 後の 20 ms sleep で、別 OS thread の偽 poller と worker の完了速度を仮定していた。poll 間隔は注入時計だが、結果の受信は実 scheduler に依存し、負荷で完了が遅れると wait 状態到達前に tick 数を尽きさせ「200 ticks で条件に届かない」にしていた。方式: (a)+(b) で `test_now` 注入時計を維持しつつ `Blocked`・`ClusterJobWaitPolled`・`Satisfied`・`Done` を保存状態・event の到着で待ち、tick 回数は失敗条件から外して壁時計 60 秒を保険にする。poll が 1 回だけ・event 重複なし・continuation が job 最終状態を前置きにして `Done` になる検査は元のまま残す。
- (2) `every_cargo_path_uses_the_scratch_target_dir` — 原因: 旧 `run_until(..., 800, Done)` が最大約 16 秒の tick 予算で並列 WU・checks・統合・reviewer を走らせ、レビューが `Reviewing` を経由する間に 800 tick に達すると `Reviewing` のまま失敗していた。tick 数は `TargetDirAdapter` の 50 ms 遅延や worktree・shell 起動の実時間を測れず、負荷で進捗と一致しなかった。方式: (b) で各 WU run・check・統合 check・review check の記録を追って最終 task の `Done` 状態を待ち、`run_until_state` の壁時計 60 秒を保険にする。`Reviewing` を成功扱いにせず、全 `CARGO_TARGET_DIR` が owner ごとの scratch target と一致する検査を保持する。
- (3) `command_checks_are_re_executed_in_workspace` — 原因: 実 `LocalWorkspace` で `test -f`・`exit 7`・`sleep 30` を 3 秒 timeout（timeout 時 6 秒で 1 回再試行）で順に実行していたため、負荷で短い shell の起動自体が遅れると存在するファイルや期待終了コードのケースまで `timed out` と誤判定する危険があった。方式: (d) で通常の pass/fail 判定は負荷で誤判定されない長さの timeout を保険として実 `LocalWorkspace` で検査し、timeout / 再試行の枝は `ScriptedWorkspace` が `sleep 30` に明示的な timeout 結果を注入して決定的にする。3 秒→6 秒の 1 回再試行・最終 `timed out` の文言・期待 exit 7・欠落ファイルの不合格は保持する。
- (4) `real_broker_browser_injection_receipt_and_origin_guards` — 原因: `IsolatedRuntime::launch` の直後に `Target.createTarget` を送っていたが、runtime ready（bwrap init と egress relay の準備）と browser の CDP pipe が応答できる時点は別であり、browser 起動が遅れると CDP 応答の 5 秒 poll timeout が `page target: SinkFailed` にまとまっていた。方式: (b)+(d) で `Browser.getVersion` の有効応答を CDP ready 条件として 60 秒の保険期限で待ってから page target を作り、応答ごとの timeout は試験用 feature のフックで 30 秒以上に差し替える。ready 後に `SinkFailed` が起きれば実障害として失敗を維持し、receipt に秘密が無いことと origin 不一致・cross-origin iframe・auth section 外の拒否を保持する。
- (5) `controller_kill_leaves_no_runtime_processes` — 原因: controller `SIGKILL` 後の PID 生存判定が `/proc/<pid>` 存在のみだと subreaper 配下で未 reap の zombie を生存と誤算するほか、bwrap の info-fd 報告から init が `PR_SET_PDEATHSIG` を設定するまでの窓で controller が死ぬ競合があり、scheduler 任せの順序では境界が再現できなかった。方式: (c)+(d)、終了判定は (b) で、試験フックで info-fd 報告直後に init を `SIGSTOP` し、subreaper も `SIGSTOP` したまま controller を `SIGKILL` してから `SIGCONT` する stutter で順序を固定する。生存判定は `Z` / `X` と starttime 不一致を除外し、同一 process の消滅・zombie 化を 60 秒の保険で待ち、継承した `SIGCHLD=SIG_IGN` による auto-reap 防ぐため試験本体・subreaper で SIGCHLD の既定値と mask を復元する。
- (6) `tick_prunes_the_oldest_terminal_workspace_and_records_an_event` — 原因: `prune_one_workspace` は削除を別スレッドに逃がしてから `WorkspacePruned` event を追記するが、旧試験は `tick()` 後に `target/` の消滅だけを `for _ in 0..100`（20 ms sleep、約 2 秒）の固定回数で待って直後に event を読んでいた。負荷では unlink が終わっても event の sqlite 書き込みが未完了の窓があり、`cleanup_and_disk.rs:97` の assert が先に落ちていた。方式: (b) で固定回数ループを削除し、`target/` の消滅と `store.events_for` に期待の `WorkspacePruned { removed: ["repos/benchfs/target"] }` が現れることの両方を `wait_for_prune` の 1 つの待ちループで確認する。60 秒の `STATE_WAIT_GUARD` は超えたら現在の `events` と `target_dir.exists()` を出して `panic!` する保険で、`target/` 消滅・repo dir 残存・`removed` 中身の assert は元のまま保持する。
- 2026-10-02 単独再実行: `cargo test -p task-dispatch --lib cluster_job_wait` 3回、各 exit 0 / 5 passed。`cargo test -p task-dispatch --lib every_cargo_path_uses_the_scratch_target_dir` 3回、各 exit 0 / 1 passed。`cargo test -p task-dispatch --lib command_checks_are_re_executed_in_workspace` 3回、各 exit 0 / 1 passed。
- browser 試験: `unshare -U -r true` は exit 1（uid_map の `Operation not permitted`）。`cargo test -p task-worker --test browser_injection_wire` を3回実行、各 exit 101（各回 inner + delayed scripted の2 passed、real browser の `real_broker_browser_injection_receipt_and_origin_guards` は unshare の `Operation not permitted` で失敗）。`cargo test -p task-worker --test browser_runtime_isolated controller_kill_leaves_no_runtime_processes -- --exact` は exit 101（helper 起動時 `NoChildPid`、launch-info 前に終了し stutter 本体に未到達）。SIGSTOP stutter 条件での今回の実行も userns 不可のため未実施。skip 扱い・成功扱いにはしていない。過去の [kill 再実行記録](progress/time-dependent-tests-kill.md)には userns が許可された環境での stutter 5/5 pass がある。
- (5) final review の bwrap zombie 証拠失敗は、継承した `SIGCHLD=SIG_IGN` による auto-reap と整合する。試験本体・subreaper で SIGCHLD の既定値と mask を復元し、reaper の停止と zombie の PPid も検証するよう修正した。[再現・修正記録](progress/time-dependent-tests-kill.md)。
- `trap '' CHLD` 下の修正前バイナリは exit 101（親の `ECHILD`）。修正後の独立した zombie/reap 回帰試験、fmt、workspace clippy は exit 0。final review のコマンド列は dispatch 3 件と worker build まで通り、browser injection が userns 不許可で exit 101。実 runtime の反復は未確認。
- 統合検査: `cargo test --workspace` は exit 101、停止した最初の target `celeris --test instance_handoff` は 8 件中 3 passed / 5 failed（worker DB guard の user namespace `Operation not permitted` が3件、handoff/standby 状態待ち失敗が2件: `a_newer_release_takes_over_while_the_old_one_finishes_its_run`、`a_stale_heartbeat_promotes_the_standby`）。残り3失敗名は `normal_mode_does_not_inject_the_smoke_builtins`、`verify_mode_never_dispatches_and_never_touches_daemon_instances`、`starting_the_same_release_twice_exits_three`。cargo は後続 target を実行せず workspace 全体の件数は未確定。`cargo clippy --workspace -- -D warnings` は exit 0（警告なし）。`crates/` に差分なし。
- 未修正の同種試験: ファイル・試験名・待機の形は[調査一覧「その他の一覧」](progress/time-dependent-tests.md#その他の一覧)を参照。cluster wait の human/cancel/unknown-cluster/leaf wait、build cache の parallel target-dir、orphan takeover、tree approval/gate/replan、work-unit、browser startup reap、task-api stream / browser H3、credentiald injection IPC、process-group kill、browser egress、scratch-cache wait などを含む。
- 未解決事項: user namespace を許可する環境で実 browser injection と controller kill を各3回再実行し、controller kill は今回未実施の SIGSTOP stutter 条件でも再確認する。workspace test は userns 制限下で失敗し、別途2件の handoff/standby 状態待ちも発生したため、許可環境で再実行して全 workspace 件数を記録する。この記録 unit（verify-record）自体はコード変更なし。ブランチ全体では `crates/task-dispatch/src/dispatcher/tests/cleanup_and_disk.rs`（(6) の修正、後述）を含め `crates/` の試験を変更している。

### (6) `tick_prunes_the_oldest_terminal_workspace_and_records_an_event`（完了日 2026-10-02）

- 原因・方式: [`docs/progress/time-dependent-tests.md`](progress/time-dependent-tests.md#tick_prunes_the_oldest_terminal_workspace_and_records_an_event) の該当節を参照。`for _ in 0..100` の固定回数ループ（約2秒）を削除し、`target/` の消滅と `store.events_for` の期待 `WorkspacePruned { removed: ["repos/benchfs/target"] }` 到着を 1 つの待ちループ（`wait_for_prune`）で待つ。保険の [`STATE_WAIT_GUARD`]（60 秒）を超えたら現在の `events` と `target_dir.exists()` を出して `panic!` する。`target/` 消滅・repo dir 残存・`removed` の中身の assert は元のまま。同 file の `workspace_prune_after_secs_zero_disables_pruning` の 50ms sleep は直さない（`workspace_prune_after_secs == 0` は削除スレッドを立てずに即 return するため、待っても届かない非同期処理が無い）。
- 検証: `cargo test -p task-dispatch --lib cleanup_and_disk` を3回単独実行、各 exit 0 / 3 passed。`tick_prunes_the_oldest_terminal_workspace_and_records_an_event` 単体を SIGSTOP 2ms / SIGCONT 1ms の stutter 下で3回実行、3/3 `test result: ok`（stutter 無しの 0.03s に対し 0.07〜0.10s、崩れず通過）。`cargo fmt --all -- --check` exit 0、`cargo clippy --workspace --all-targets -- -D warnings` exit 0。本体（`crates/task-dispatch/src/dispatcher/housekeeping.rs` の `prune_one_workspace` 等、非試験コード）は変更していない。

### 人が実行する手順（userns が使える host で、最終コード向け）

- 対象 SHA: main（`448891a2`）を取り込んだ merge commit `a46b7423`。この leaf（merge-main）の HEAD と、統合後の task branch の HEAD は crates/ の tree がこれと同じになる。確かめるコマンド: `git diff --stat a46b7423 <使う SHA> -- crates` の出力が空であること。この merge では `crates/task-worker/src/browser_runtime.rs` の init 待ちで衝突が出た。main の `NoChildPid(rt.failed_stderr())` を残し、このブランチの `test_hook::stop_init_after_info(rt.inner_pid)` は inner_pid が決まった直後に置いた。
- 前回の人の実環境確認は `57576a5a`（SIGCHLD 継承の修正 `e64043be` と main merge の前）。最終コードでは人の手ではまだ確かめていない。
- 前提: `unshare -U -r true` が exit 0。`CARGO_TARGET_DIR` はローカルを使う。CPU を焼く負荷はかけない。

1. (4) browser injection を単独で 3 回実行する:
   ```
   for i in 1 2 3; do cargo test -p task-worker --test browser_injection_wire -- --exact real_broker_browser_injection_receipt_and_origin_guards; echo "exit=$?"; done
   ```
2. (5) controller kill を単独で 3 回実行する。試験は自分で subreaper と init を SIGSTOP し、controller の SIGKILL 後に SIGCONT する（[kill 記録](progress/time-dependent-tests-kill.md)の表、「修正後」行の stutter 条件）:
   ```
   for i in 1 2 3; do cargo test -p task-worker --test browser_runtime_isolated -- --exact controller_kill_leaves_no_runtime_processes; echo "exit=$?"; done
   ```
3. (5) を SIGCHLD 無視が継承される状態で 3 回実行する（final review で bwrap zombie の assert が落ちた条件）:
   ```
   for i in 1 2 3; do sh -c "trap '' CHLD; exec cargo test -p task-worker --test browser_runtime_isolated -- --exact controller_kill_leaves_no_runtime_processes"; echo "exit=$?"; done
   ```
4. 外からの SIGSTOP stutter（停止 2ms・再開 1ms、sleep だけで CPU は焼かない）の下で (4)(5) を各 3 回、(5) は SIGCHLD 無視の下でも 3 回実行する。(5) は `STUTTER_SCOPE=pid` で試験 process だけを止める。process group 全体を止めると外からの SIGCONT が、試験が止めた subreaper を起こしてしまい、試験の前提が壊れる。この worker で group 指定にすると bwrap の zombie assert が 6/6 落ちたが、これは試験側の不具合ではない。
   ```
   cargo test -p task-worker --test browser_injection_wire --no-run && cargo test -p task-worker --test browser_runtime_isolated --no-run
   D=$CARGO_TARGET_DIR/debug   # 未設定なら target/debug
   B4=$(ls -t $D/deps/browser_injection_wire-* | grep -v '\.d$' | head -1)
   B5=$(ls -t $D/deps/browser_runtime_isolated-* | grep -v '\.d$' | head -1)
   cat > /tmp/stutter.sh <<'EOF'
   #!/bin/sh
   # usage: [STUTTER_SCOPE=pid|group] [BIN_DIR=<target>/debug] stutter.sh <test-binary> <test-name>
   d=${BIN_DIR:-/nonexistent}
   # dash は名前に '-' を含む env を落とすので、(4) が実行時に読む CARGO_BIN_EXE_* は env(1) で渡す
   setsid env "CARGO_BIN_EXE_celeris-browser-sandboxd=$d/celeris-browser-sandboxd" \
     "CARGO_BIN_EXE_celeris-browser-egress=$d/celeris-browser-egress" \
     "$1" --exact "$2" --test-threads=1 &
   pid=$!
   t=$pid; [ "${STUTTER_SCOPE:-pid}" = group ] && t=-$pid
   while kill -0 "$pid" 2>/dev/null; do
     kill -STOP -- "$t" 2>/dev/null; sleep 0.002
     kill -CONT -- "$t" 2>/dev/null; sleep 0.001
   done
   wait "$pid"
   EOF
   chmod +x /tmp/stutter.sh
   for i in 1 2 3; do BIN_DIR=$D STUTTER_SCOPE=group /tmp/stutter.sh $B4 real_broker_browser_injection_receipt_and_origin_guards; echo "exit=$?"; done
   for i in 1 2 3; do STUTTER_SCOPE=pid /tmp/stutter.sh $B5 controller_kill_leaves_no_runtime_processes; echo "exit=$?"; done
   for i in 1 2 3; do sh -c "trap '' CHLD; STUTTER_SCOPE=pid exec /tmp/stutter.sh $B5 controller_kill_leaves_no_runtime_processes"; echo "exit=$?"; done
   ```
5. 合格の見分け方:
   - 各回 `exit=0` で、末尾が `test result: ok. 1 passed`（`FAILED` ではない）。
   - (5) の出力に `runtime survived`、`bwrap must be an unreaped zombie`、`panicked` が無い。
   - (4) の出力に `SinkFailed` と `panicked` が無い。
   - `CELERIS_ISOLATION_TESTS=skip` を設定していないのに `SKIPPED` と出たら、環境の不備（bwrap や browser が無い）で、合格ではない。
- 結果は `docs/progress/time-dependent-tests-injection.md` と `docs/progress/time-dependent-tests-kill.md` に追記する。
- 2026-10-02 の merge-main worker で上の 1〜4 を `a46b7423` で実行した（この sandbox では `unshare -Ur true` が exit 0、load average 約 31）。結果:
  - 手順 1〜3: 各 3/3 `test result: ok`。`SinkFailed`、`runtime survived`、`SKIPPED` は出なかった。
  - 手順 4: (4) は pid 指定・group 指定とも 3/3 ok。(5) は pid 指定で 3/3 ok、SIGCHLD 無視下で 3/3 ok。
  - 手順 4 の限界: 外からの stutter で実行時間はほとんど変わらなかった（(4) 0.43〜0.48s、(5) 0.08〜0.10s）。止められるのは試験 process とその process group だけで、自分で session を作る runtime の中までは届かないと見られる。競合点への負荷の再現は、試験の中にある stutter（(5)）と遅延（(4) `delayed_cdp_page_target_response`）が担う。
  - これは人の host での確認の代わりではない。

### 実環境での確認（2026-10-02、ADR-0079 D7 人の回答）

人が上記の手順を userns の使える host（host `home-dev`）で実行した。`unshare -U -r true` は exit 0。load average 18.5（CPU を焼く負荷はかけていない）。task branch `57576a5a` を `/var/tmp` の worktree に取り出し、`CARGO_TARGET_DIR` はローカルを使用。

- (4) `cargo test -p task-worker --test browser_injection_wire -- --exact real_broker_browser_injection_receipt_and_origin_guards` を3回単独実行: 3/3 `test result: ok`（1 passed、各回 0.45〜0.49s）。`SinkFailed`・`SKIPPED` の出力なし。
- (5) `cargo test -p task-worker --test browser_runtime_isolated -- --exact controller_kill_leaves_no_runtime_processes` を3回単独実行: 3/3 `test result: ok`（1 passed、各回 0.08s、`helper_reaper` ok）。`runtime survived`・`SKIPPED` の出力なし。
- SIGSTOP stutter の修正前後比較（コードを一時的に外して再現させる手順）は、今回この人の実行では行っていない。[`docs/progress/time-dependent-tests-kill.md`](progress/time-dependent-tests-kill.md) にある、userns が許可された環境での過去の stutter 5/5 pass の記録を採用する。

未解決事項: 上記の (4)(5) は3回とも合格し、未解決の失敗はない。SIGSTOP stutter の修正前後比較（本来の手順3番目）は今回の人の実行では未実施（worker sandbox では userns が使えず自動実行できず、今回人が実行した際も改めてはやらず、過去の [kill 記録](progress/time-dependent-tests-kill.md)の stutter 5/5 pass を根拠として採用したため）。

この記録は task branch `57576a5a`（SIGCHLD 継承の修正 `e64043be` と main merge の前）に対するもので、最終コードでの確認は下の「実環境での確認（最終コード）」に置き換わる。

### 実環境での確認（最終コード、2026-10-02、ADR-0079 D7 real-env-2 人の回答）

人が上記「人が実行する手順」の手順1〜4を、merge-main 完了後の最終 SHA `ab1914e629d9`（HEAD。`e64043be` の SIGCHLD 継承修正と main merge `a46b7423` を含む）で実行した。host `home-dev`、`unshare -U -r true` は exit 0、load average 14〜18（CPU を焼く負荷なし）。`ab1914e629d9` を `/var/tmp` の worktree に取り出し、`CARGO_TARGET_DIR` はローカル、`CELERIS_USERNS_TESTS=1` で実行した。`git diff --stat a46b7423 ab1914e629d9 -- crates` は空（crates の tree は `a46b7423` と同一）。

- 手順1 (4) browser injection 単独 ×3: 3/3 `test result: ok`（各 0.43〜0.52s）。
- 手順2 (5) controller kill 単独 ×3: 3/3 `test result: ok`（各 0.08s、`helper_reaper` 0.10s）。
- 手順3 (5) を `trap '' CHLD` 下で ×3: 3/3 `test result: ok`。
- 手順4 SIGSTOP stutter: (4) `STUTTER_SCOPE=group` ×3 で 3/3 ok（1.65s・1.73s・7.03s、stutter による遅延が効いている）。(5) `STUTTER_SCOPE=pid` ×3 で 3/3 ok。(5) SIGCHLD 無視 + pid stutter ×3 で 3/3 ok。
- 全回（合計18回）で `SinkFailed`・`runtime survived`・`bwrap must be an unreaped zombie`・`panicked`・`SKIPPED` のいずれの出力もなかった。

結果: 全件合格（推奨どおり）。未解決の失敗なし。最終コードでの (4)(5) の実環境確認・SIGSTOP stutter 条件（SIGCHLD 無視下を含む）は、ここで完了したものとして記録する。コードの変更はこの記録には含まれない。

## Web GUI dogfood（開始 2026-10-01、release bf54b41ad627）

- 状態: 本番 daemon 向けの web gateway `127.0.0.1:7720` と LAN 入口 `192.168.1.103:7721` を起動。gui/ :7700 は継続稼働。PC 1440px・スマホ 390px の読み取り確認は合格。
- H6: 期間・合格条件・判定日は人の決定待ち。決まるまで cutover しない。H9: 通知方針は人の決定待ち。H10: release `bf54b41ad627` の staging verify exit 0、読み取り parity 3 passed。
- 配置上の問題（2026-10-01）: 本番 release パスが前 run の staging 成果物を指す symlink。参照先を dogfood 中に削除しない。NFS 実体コピーは途中で中止。再起動時は web unit と LAN socket を手動で start する。恒久化の対応・再確認結果は未記入。
- 恒久化（2026-10-02、web ADR-W3 / ADR-0081 付記 (C)(D)）: `scripts/selfdeploy/web-follow.sh <new> <old>` を新設し、`promote.sh` が昇格後に呼ぶ（旧 `celeris-web@<old>` が active かつ新 release の `gate.json` `web.ok=true`・`web/app/server/index.js` ありのときだけ新へ切替。失敗は warning、exit 0、`celeris@` には触れない）。`celeris-web@.service` から `Wants=celeris@%i.service` を除去。証拠: `bash scripts/selfdeploy/tests/promote_web_follows_release.sh` exit 0（(a)(b)(b2)(c)(d)(e) 全 ok）。本番の override.conf 撤去・unit の置き直し・web 再起動は人の手順（[docs/selfdeploy.md §4e](selfdeploy.md#4e-web-の追従web-followsh)）。未実施。
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

### ADR 番号振り直し後の最終検証 — 2026-10-02（work unit `verify-land2`）

前回 reviewer の指摘（ADR-0117 の番号衝突、`instance_handoff` 失敗未確認のまま `cargo test --workspace` が中断）に対応。`docs/adr/` は振り直し済みで、root delivery の ADR は `0119-root-delivery-without-assignee.md`、`0117-review-human-decisions-and-check-results.md` と重複なし（`0078` の重複はこの task 以前から main に存在する無関係な既存衝突で、範囲外のため変更していない）。main（`188fa27409f11305414b21d048f0f0e260c6efdb`）は既に HEAD の祖先。

- `cargo fmt --all -- --check` → exit 0（差分なし）。
- `cargo test --workspace` → exit 0（`instance_handoff` 8 件を含め全テストバイナリで `test result: ok`、失敗 0。`a_stale_heartbeat_promotes_the_standby` が 60 秒超過の警告を出すが結果は ok）。
- `cargo test -p celeris --test instance_handoff` を単独で 3 回連続実行 → いずれも exit 0（8 passed / 0 failed、各約 60.5 秒）。前回 reviewer が見た失敗は本ブランチの変更が原因ではなく、再現しなかった（負荷依存の既存 flaky として記録。今回は user namespace 制限も再現せず）。
- `cargo clippy --workspace -- -D warnings` → exit 0（警告なし）。
- `gui`: `pnpm install --frozen-lockfile` → exit 0、`pnpm run typecheck`（`react-router typegen && tsc -b`）→ exit 0、`pnpm run test`（vitest）→ exit 0（85 files / 1250 tests passed）。
- `git merge-tree --write-tree main HEAD` → 衝突なしで tree を生成（exit 0）。`git merge-base --is-ancestor main HEAD` → exit 0。
- `git status` / `git diff --stat` ともに本行追記以外の変更なし。本番 DB・本番 host は操作していない。

### 取り込み前の最終確認 — 2026-10-02（work unit `land-final`）

main（`ea86af6307f87bf8bd3a9d2069ec45f75325fc68`）は HEAD (`14bf01edb90b42135c488198ace804aab14023db`) の祖先（`git merge-base --is-ancestor main HEAD` → exit 0）で、追加 merge は不要だった。`git merge-tree --write-tree main HEAD` は exit 0、tree `57e25288479c7e08a33a30e3b0a4a9384cb3a8b4` を生成し、衝突なし。

- ADR-0121 `docs/adr/0121-root-delivery-without-assignee.md` と ADR-0051 の付記、コード、生成 schema、PROGRESS の参照は一致。`git ls-tree -r` で main と refs/heads/celeris・refs/remotes/celeris の全 129 refs を走査し、ADR-0121 は本ブランチと関連する統合ブランチの 2 refs のみで使用。main に同番号の決定はなく、内容の異なる ADR 番号衝突はない。
- `cargo fmt --all -- --check` → exit 0。
- `cargo test --workspace` → exit 0（全 workspace 成功、`instance_handoff` を含む）。失敗試験なし。前回レビューでの `instance_handoff` 失敗は今回の再実行で再現せず、`verify-land2` で同 test binary を単独 3 回実行した結果も全て 8/8 pass のため、本ブランチ起因ではない負荷依存 flaky と判断。
- `cargo clippy --workspace -- -D warnings` → exit 0（警告なし）。
- GUI（`gui`）: `pnpm install --frozen-lockfile` → exit 0、`pnpm run typecheck` → exit 0、`pnpm run test` → exit 0（85 files / 1250 tests）。
- 本番 DB・本番 host は操作していない。

### reland-main — 2026-10-02

- main `7f3482a307f7221d7e74d88af123e26b92aac867` を取り込む前に `git merge-tree --write-tree HEAD main` を実行し、`docs/progress/phase-R.md` に両側の追記競合を確認。通常の `git merge main` が phase-R の両方の節を保持して完了した。統合後 `git merge-base --is-ancestor main HEAD` → exit 0。
- ADR 番号を main と全 `celeris/*` refs の `git ls-tree` で確認。0119 は既存の root-delivery ADR を持つ refs があり、0120 も既存 ADR に使用済み。0121 は main と走査した全 refs のどちらにも ADR 文書がなく最小の空き番号だったため、root-delivery ADR を `docs/adr/0121-root-delivery-without-assignee.md` に変更。ADR-0051、実装・migration コメント、schema、GUI types、PROGRESS の参照も ADR-0121 に揃えた。
- `cargo fmt --all -- --check` → exit 0。
- `cargo test --workspace` → exit 101。`instance_handoff` は 8 件中 3 件成功、5 件失敗。`normal_mode_does_not_inject_the_smoke_builtins`、`verify_mode_never_dispatches_and_never_touches_daemon_instances`、`starting_the_same_release_twice_exits_three` は worker DB guard の user namespace `Operation not permitted` が原因。`a_newer_release_takes_over_while_the_old_one_finishes_its_run` と `a_stale_heartbeat_promotes_the_standby` も失敗。
- `cargo test -p celeris --test instance_handoff` を 3 回単独再実行 → 3 回とも 3 passed / 5 failed、同じ 5 件が失敗。userns sandbox 制限を含む既存の `instance_handoff` failures と判断する。指定どおり sandbox 制約を修正理由にはしていないが、workspace test 成功とは扱わない。
- `cargo clippy --workspace -- -D warnings` → exit 0（警告なし）。
- `git merge-tree --write-tree main HEAD` → exit 0（作業前 tree `c0a8f60f0e346a020fecd806054c9588f9671fc3`、記録 commit 後 tree `13cadac91414f7cd69f0d9d25ff279d9b5de7ff7`、いずれも衝突なし）。
- 検証記録を commit `0ec72fdd` に保存。作業後 worktree は clean。
- 本番 DB・本番 host は操作していない。

### rerun-flaky: 高負荷起因の失敗の単独再実行 — 2026-10-02

段 reverify の統合検査で `browser_injection_wire`（2 試験）と `browser_runtime_isolated::controller_kill_leaves_no_runtime_processes` が失敗した件は、別 task の負荷試験で host の load average が上がったことによる環境起因と人が判断した（負荷は 09:15 に停止済み）。コードは変更していない。

- `uptime` → `09:19:49 up 1 day, 11:26,  6 users,  load average: 23.67, 28.81, 31.48`（再実行開始時点。負荷停止直後で 1 分平均はまだ下降中）。再実行完了時点の `uptime` → `09:22:01 up 1 day, 11:28,  6 users,  load average: 19.53, 26.02, 30.14`。
- `unshare -U -r true` → 成功（user namespace 作成は拒否されていない）。
- `cargo build --workspace --bins` → exit 0。
- `cargo test -p task-worker --test browser_injection_wire` → exit 0、`test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`（`inner_injection_wire`、`real_broker_browser_injection_receipt_and_origin_guards` とも ok）。
- `cargo test -p task-worker --test browser_runtime_isolated controller_kill_leaves_no_runtime_processes -- --exact` → exit 0、`test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 4 filtered out`。
- 2 回連続で単独実行し、どちらも同じ結果を再確認した。
- 人の判断のとおり、環境（host 負荷）起因の flaky と確認できた。コード変更なし。

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

#### 統合検査 flaky の単独再実行（tick_prunes）

- 対象: `dispatcher::tests::cleanup_and_disk::tick_prunes_the_oldest_terminal_workspace_and_records_an_event`。各回の直前に `/proc/loadavg` を読み、`cargo test -p task-dispatch --lib tick_prunes_the_oldest_terminal_workspace_and_records_an_event` を個別に foreground 実行した。
- 1回目: loadavg `24.22 24.05 24.86 28/1583 3`、exit 0、`1 passed; 0 failed`（498 filtered out）。
- 2回目: loadavg `24.33 25.07 25.22 3/1496 3`、exit 0、`1 passed; 0 failed`（498 filtered out）。
- 3回目: loadavg `21.29 24.39 25.00 2/1427 3`、exit 0、`1 passed; 0 failed`（498 filtered out）。
- 先行する `cargo test --workspace` は load 25 前後で同じ試験が失敗し、`498 passed; 1 failed` だった。人は環境（host 高負荷）起因の tick 依存 flaky と判断した。今回の3回はすべて pass。指示どおりコードは変更せず、修正は別 task `01M3Y4AV5Z` が担当する。

### 最新 main（7f3482a3）取り込み — 2026-10-02（work unit `land-main`）

`main` の `7f3482a3` を merge で取り込んだ（rebase なし）。衝突は 2 ファイル。
- `crates/task-worker/tests/browser_shared_cdp.rs`: main 側 5eb666f6（R7-12）の `ISOLATION`・`PREFLIGHT_TIMEOUT`・`probe.err` の excepthook・WebSocket frame の読み切り（`recv_exact`）を土台にした。そこへこのブランチの 97eb5194（接続から CDP 応答までを 1 試行とし、期限 55 秒の中で再試行する）を載せた。`find_browser`・`preflight`・`run_bounded` と、launcher 用の `userns: UsernsMode::Unshare` は自動 merge でそのまま残っている。
- `docs/PROGRESS.md`: main の R7-10・ADR-0117・repair 許可範囲の節を先に、この節を含む launcher 節を後に置き、どちらも残した。
- 証拠: `cargo build -p task-worker --bins` exit 0。`cargo fmt --all -- --check` exit 0。`cargo clippy --workspace -- -D warnings` exit 0。`cargo test --workspace` exit 0（3261 passed / 0 failed / 12 ignored）。`CELERIS_ISOLATION_TESTS=require cargo test -p task-worker --test browser_shared_cdp` を 3 回実行し、3 回とも exit 0（2 passed）。flaky は出なかった。launcher の実 host 試験は再実行していない（人の側で済み）。

### main（764a737d）取り込み — 2026-10-02（work unit `land-main2`）

`main` の `764a737d` を merge で取り込んだ（rebase なし）。衝突は 2 ファイル。
- `crates/task-worker/src/browser_runtime.rs`: main 側 c11ffd35 の `RuntimeError::InitNotReady` と、`IsolatedRuntime::launch` で `--info-fd` の後に pid ns init の starttime を記録して `/proc/<pid>/wchan` が `do_wait` になるまで待ち、未完了なら本人確認のうえ init を SIGKILL する処理を残した。このブランチ側の `RelayNotReady(String)`・`NoChildPid(failed_stderr)` の診断、launcher 経路（`UsernsMode::Fd`、`/tmp/celeris-session` の bind、relay 診断、Chrome lifecycle 診断）もそのまま残した。
- launcher 経路での init 待ち: launcher は bwrap の親として host の pid ns にいるので `--info-fd` の pid は host pid。init の userns は launcher（euid celeris-browser）が owner の userns の子孫なので、launcher は init の `/proc/<pid>/stat`・`wchan` を読め、SIGKILL も送れる。このため launcher 経路の扱いは変えていない（理由をコードのコメントにも書いた）。unit（`deploy/systemd/celeris-browser-launcher.service`）に `ProtectProc`・`PrivatePIDs` は無く、他 UID の `/proc` は見える。
- `tests/browser_runtime_isolated.rs` の main 側変更は自動 merge で残り、launcher 用の `userns: UsernsMode::Unshare` も残っている。
- `docs/PROGRESS.md`: main の verify-land2・land-final・reland-main・rerun-flaky などの節を先に、launcher 節を後に置き、どちらも残した。
- 証拠: `cargo build -p task-worker --bins` exit 0。`cargo fmt --all -- --check` exit 0。`cargo clippy --workspace -- -D warnings` exit 0。`cargo test --workspace` exit 0（3279 passed / 0 failed / 12 ignored）。`cargo test -p task-worker --test browser_runtime_isolated` exit 0（5 passed、1 ignored）。flaky は出なかった。
- **launcher の binary に効く変更あり**: `browser_runtime.rs` の launch（init 待ち）は launcher・sandboxd・egress の binary に入る。host の binary は 86ce1a88 のビルドのままなので、反映には人による入れ替えが要る。
- main 取り込み後の launcher binary の実 host 再試験は未実施（任意で人が require 試験を再実行）。手順は `CELERIS_LAUNCHER_TESTS=require cargo test -p task-worker --test browser_launcher_ptrace -- --nocapture`（binary の入れ替えは `sudo /usr/local/sbin/celeris-browser-launcher-update`）。

### prompt-rule: planner の check 指針に重い負荷台本禁止を追記 — 2026-10-02

`crates/task-worker/src/claude_code/prompt.rs` の `PLANNER_CHECK_GUIDANCE`（check の書き方の箇条、1639〜1663 行）の末尾に次の 1 行を追加した: 「Do not run CPU-burning load scripts (busy loops, a CPU-burning load generator tool, parallel cargo load) in checks or acceptance; reproduce timing bugs deterministically (paused or injected clock, event waits, SIGSTOP/SIGCONT, test-only delay hooks; see docs/testing.md).」。同文字列を `crates/task-worker/src/claude_code/tests.rs` の `planner_prompt_has_the_check_writing_section` の needles 配列にも追加した。(注記: 本節の英文引用は負荷生成ツールの固有名を言い換えている。実コード中の文字列そのものはこのタスクの差分に含まれないため変更していない。)

worker（非 planner）向け指示と `task-dispatch` の要否確認:
- `git grep -n "check の書き方\|CHECK_GUIDANCE" crates/task-dispatch crates/task-worker` → `PLANNER_CHECK_GUIDANCE` 定数は `crates/task-worker/src/claude_code/prompt.rs` にのみ存在し、`task-dispatch` に check 作成の指針テキストは無い。
- worker（非 planner）実行の前置きは `crates/task-worker/src/preamble.rs`（`render`/`mode_section`/`repos_note` など）にあるが、worker は checks/acceptance を**書く**側ではなく既存の check を実行・満たす側なので、「check を書くときの注意」を worker 向けに追記する対象がない。worker 向けの指示には変更不要と判断した（追記しない理由として記録）。
- 試験の prompt snapshot/hash 試験は存在しない（`grep -n "sha256\|snapshot\|hash" crates/task-worker/src/claude_code/tests.rs` に prompt 関連の一致なし）ため、他に更新箇所はない。

検証:
- `cargo test -p task-worker --lib claude_code::` → exit 0、91 passed（`planner_prompt_has_the_check_writing_section` を含む）、0 failed。
- `cargo clippy --workspace -- -D warnings` → exit 0（警告なし）。

## ui-ux 外部 skill の org 種更新と結合試験（work unit `e2e-verify`）

完了日 2026-10-02。ADR-0122 の木（深さ1: ui-ux 課への 4 外部 skill 登録、深さ2: このtask）の最終段。
`config/org.example.toml` の `ui-ux` に `skills_mounts = ["frontend-design", "shadcn", "web-design",
"ui-ux-quality-gate"]`（license: none で除外した skill は無いため 4 件とも）と、依存方針（shadcn 以外の
新規ライブラリは提案に留める、外部ネットワークに出ない）の policy 1 行を追加した（routing 用の
`profile.skills` は不変）。`crates/celeris/tests/ui_ux_skills_delivery.rs` を新規に追加し、
`config/skills/` を一時 KB に取り込み → `org.example.toml` から `ui-ux` の実効 profile を解決 →
`task-worker` の配送関数（`deliver_claude_code` / `deliver_agents_md`）で実際に materialize するところまでを
結合して確認した（LLM 呼び出しなし、外部ネットワークなし）。詳細は
`docs/progress/ui-ux-skills.md` の「org 種の更新と結合試験」節。

- 証拠: `cargo build --workspace --bins` → exit 0。`cargo test --workspace ui_ux_skills` → 4 試験バイナリ
  （celeris / celerisctl / task-dispatch / task-ops）で計 12 passed / 0 failed。
  `cargo fmt --all -- --check` → exit 0。`cargo clippy --workspace -- -D warnings` → exit 0。
  `cargo test --workspace` → exit 0（120 試験バイナリすべて `test result: ok`、合計 3236 passed / 0 failed、
  失敗・flake 無し）。
- 既存の routing 試験（`example_org_routes_ui_work_to_ui_ux_and_api_work_to_software_engineering` 等）も
  上記のフルスイートに含まれ通過を確認済み。
- 未解決事項: なし。

## ADR-0122 完了（ui-ux 外部 skills）

完了日 2026-10-02。ADR-0122（外部 agent skill の vendoring・KB への取り込み・ui-ux への mount・quality gate の
reviewer 配布）の D1〜D6 を実装し、`docs/adr/0122-ui-ux-external-skills.md` の状態欄を「採用・実装済み」に更新した。
実装 commit: vendor-skills `d39733d8`、vet-skills `3cfdf69a`、adr `7ce72c5c`、skill-import `41e6324b`、
review-skills `dcf7aefd`、e2e-verify `6d95a306`、runbook `99a529c6`。

- 証拠コマンド: `cargo test -p celeris --test ui_ux_skills_delivery`（結果の詳細は
  `docs/progress/ui-ux-skills.md` を参照。config/skills → 一時 KB → ui-ux 実効 profile → worker 配送の結合試験が
  全件 pass、routing 回帰試験も同じフルスイートで通過を確認済み）。
- 未解決事項:
  - 本番の KB 取り込み・ui-ux への mount は worker からは行わない。人が `docs/ops/ui-ux-external-skills.md` の
    手順で実行する（ADR-0095 付記 D-d）。
  - `web-design` の LICENSE 判断（LICENSE ファイルが無く README の License 節に拠っている点）は、より厳しい
    基準を採るかどうかを人が判断する（ADR-0122 D6、`docs/progress/ui-ux-skills.md`）。
- release/verify（work unit release-report、2026-10-02、HEAD `a58f68b5551b` = adr-status 統合後）:
  - release.sh: sha12 a58f68b5551b exit 0（`CELERIS_STATE_DIR` を scratch に、`SD_USE_CALLER_CARGO_TARGET=1`
    `SD_RELEASE_PRUNE=0`。worker sandbox から本番の `~/.local/celeris/releases` は読み取り専用なので、既定の
    state dir では lock を作れず exit 1。gate.json ok=true: fmt / cargo-test（nextest 3236 passed・11 skipped・
    doctest ok）/ clippy / source-size-report / build --release / pnpm install・typecheck・build / web の
    install・typecheck・test・release がすべて exit 0。gui/ に変更が無いので pnpm-test・mobile-audit・e2e:mock は
    skipped（base ea86af6307f8））。
  - verify.sh: exit 0（verify.json ok=true live_ok=true。検査 1〜6 すべて true）。1 回目は worktree の
    gui/ に devDependencies が無く検査 4b（gui-e2e）だけ「@playwright/test not found」で exit 1。
    `pnpm install --offline --frozen-lockfile` 後の再実行で 4b も pass。本番の daemon・DB・config・systemd には
    触れていない（DB は `mode=ro` の `.backup` を読むだけ）。
  - `unshare -U -r true` → exit 0（この run の sandbox では user namespace を作れた）。

### final review 失敗の再実行（work unit `rerun-dispatch`）

2026-10-02 に、final review の `cargo test --workspace` で失敗した 2 件を単独で各 3 回実行し、続けて
`cargo test -p task-dispatch --lib` を実行した。各試験の直前に取得した `uptime` の load average（1/5/15 分）も併記する。

| 試験 | 回 | exit | passed | load average (1/5/15 分) |
| --- | ---: | ---: | ---: | --- |
| `cluster_job_wait::a_wait_parks_the_task_polls_and_resumes_as_a_continuation` | 1 | 0 | 1 | 20.82 / 23.28 / 20.88 |
| 同上 | 2 | 0 | 1 | 21.93 / 23.41 / 20.99 |
| 同上 | 3 | 0 | 1 | 19.55 / 22.85 / 20.84 |
| `every_cargo_path_uses_the_scratch_target_dir` | 1 | 0 | 1 | 16.57 / 22.03 / 20.60 |
| 同上 | 2 | 0 | 1 | 19.71 / 22.07 / 20.66 |
| 同上 | 3 | 0 | 1 | 21.56 / 22.42 / 20.82 |
| `cargo test -p task-dispatch --lib` | — | 0 | 503 | 15.65 / 20.14 / 20.15 |

各単独実行はすべて 1 passed / 0 failed、lib 全体は 503 passed / 0 failed / 0 ignored。再現しなかったため、
この再実行では `plan_issue` は発生していない。作業ブランチの起点 `6b49a92dd5d7` から HEAD までの
`git diff --name-only` は空で、`review.rs`・review tests・`review_spawn` 周辺の skill 配布差分も無い。
したがって、その変更は対象 2 試験の経路に触れていない。

### land-main: 最新 main の統合と最終検査 — 2026-10-02

main `95ac16442f92` を merge し、`docs/PROGRESS.md` の衝突を解消した。ui-ux external skills の記録と CPU 負荷規則・planner 指針の記録を両方保持した。全ターゲット clippy で main 由来の `ui_ux_skills.rs` に型複雑度と不要な let-return の lint が見つかったため、型 alias と直接 return に整えた。

- `git merge-base --is-ancestor 95ac16442f92 HEAD` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- `cargo clippy --workspace --all-targets -- -D warnings` → exit 0（初回は上記2 lint で失敗、修正後 pass）。
- `cargo test -p task-worker --lib planner_prompt_has_the_check_writing_section` → exit 0（1 passed、0 failed）。

## provider / LLM source 分離の設定例（2026-10-02、config-docs）

ADR-0132 D2・D7 に合わせ、設定例の provider ID を source 非依存の名前にし、
PaperQA の設定 JSON を `config/paperqa.proxy.example.json` に改名した。
PaperQA は `celeris/standard`、LDR・LangMem・opencode は `celeris/cheap` を参照する。
Qwen の tier 写像は cheap のみ。人が実行する本番移行・確認・戻し方は
`docs/ops/provider-llm-source-migration.md` に記した。

`cargo test -p celeris --lib example` の既存 `loads_research_example_config` は
`[adapters.paperqa].settings` の旧ローカルパスを文字列一致で固定しているため、
設定例を `settings/proxy` にした後はそのアサーションだけ失敗する。
設定の `Config::load` と `validate` は成功し、他の example 試験は成功した。
この WorkUnit の指示に従って試験コードは変更していない。
`celerisctl config to-harnesses --config <file>` による全 10 件の
`config/celeris*.example.toml` の読み込みと設定検査は全件 exit 0。

## provider の LLM source / adapter 分離と cheap 専用 Qwen — 全体検証（2026-10-02、verify）

完了日 2026-10-02。ADR-0132（`docs/adr/0132-provider-llm-source-split-and-cheap-qwen.md`）の D1〜D7
（provider を LLM source と adapter / harness に分ける設定・API・schema、llm-proxy の Qwen tier 写像を
cheap だけにする fallback、paperqa・langmem・ldr・opencode の Qwen 前提除去、gui/ と web/ の providers 画面、
設定例と本番移行手順）の統合後 HEAD（`ecafb5a4` 「integrate wu/worker-tools (phase tools)」）に対して
全体の fmt・clippy・ビルド・対象試験を実行した。ADR-0132 の状態欄を「実装済み」に更新した。実装そのものの
修正はこの WorkUnit では行っていない（指示どおり）。

- 証拠コマンドと結果:
  - `cargo fmt --all -- --check` → exit 0（差分なし）。
  - `cargo clippy --workspace -- -D warnings` → exit 0（warning ゼロ）。
  - `cargo test --workspace --no-run` → exit 0（全クレート・全試験バイナリのビルドに成功。実行はしていない
    — userns が要る browser 系試験が worker sandbox で落ちるため、全体の実行は daemon 側の workspace check
    に任せる）。
  - `cargo test -p llm-proxy` → exit 0。unit 39 passed、`tests/proxy_integration.rs` 27 passed、doctest 0
    （cheap のみの Qwen 優先・`celeris/frontier`・`celeris/standard` が Qwen に倒れないこと・Qwen 生存時は
    Qwen 優先・不達時は Claude/GPT cheap へ倒れることを固定する `cheap_only_*` 試験を含む）。
  - `cargo test -p celeris config::` → exit 0、86 passed（設定の読み込み・検証・互換 provider 行・
    knowledge fallback 設定を含む）。続けて `cargo test -p celeris --lib` 全体も exit 0、217 passed
    （`config::tests::loads_research_example_config` を含め、settings/proxy 化後の設定例試験もすべて pass。
    config-docs 段で期待パスの更新が済んでいるため、以前この節に記録していた既知の失敗は解消済み）。
  - `cargo test -p task-api --test providers_admin` → exit 0、13 passed（`provider_kind_*` を含む provider
    の kind・llm_source の作成・更新・互換検証）。
  - `cargo test -p task-api --test llm_sources` → exit 0、3 passed（`GET /api/v1/llm/sources` の読者ビュー）。
  - `cargo test -p task-dispatch --lib knowledge` → exit 0、9 passed（knowledge_fallback: cheap-only の
    Qwen 生存時・不達時の両経路、langmem 到達可否、probe キャッシュを含む）。
  - `cargo test -p task-dispatch --lib`（全体）→ exit 0、506 passed / 0 failed（最終レビュー受け入れ条件 0
    の `cargo test -p task-dispatch --lib` に一致）。
- 未解決事項:
  - 本番の `~/.config/celeris/config.toml` の移行はこの WorkUnit では実行していない。人が
    `docs/ops/provider-llm-source-migration.md` の手順（控え・ID 対応表・`models.qwen` の非 cheap キー削除・
    `celeris/<tier>` への切替・opencode の cheap 制限・検証・画面確認・戻し方）に従って行う。
  - browser 系など userns を要る試験（`crates/celeris/tests/browser_*` ほか）はこの worker sandbox では
    実行できない。daemon 側の workspace check（本番相当の権限を持つ環境）の `cargo test --workspace` に
    全体実行を委ねる。
  - `cargo test --workspace` のフル実行はこの run では行っていない（上記の userns 制約のため、対象クレート・
    対象試験に絞って実行した）。
- 提案: 次に本番 config.toml を移行する際は、移行前後で `GET /api/v1/providers` と `GET /api/v1/llm/sources`
  の応答を保存し比較すると、`kind` / `llm_source` の推定結果が意図どおりか目視確認しやすい。
- ACP の Qwen 直指定（`OPENCODE_CONFIG` 無し・`openai_compatible` の Qwen source）も cheap だけに絞る修正（acp-cheap）。

### land-main3: 最新 main の統合 — 2026-10-02

main `0d438ec19d9a` を merge し、`docs/PROGRESS.md` の両側の節を保持した。main の ADR-0122 完了・ui-ux 外部 skill の結合試験・planner 指針・最終検査記録に加え、browser launcher の実 process 証跡、tick_prunes の単独再実行、過去の land-main/land-main2 記録も残した。

- main 由来の launcher 関連差分を確認: `crates/task-worker/src/browser_runtime.rs` は main 側の init 待ち変更を含み、launcher/sandboxd/egress の起動経路に効く。この変更は既に land-main2 の記録に記載済みで、host の binary 入れ替えと require 試験の再実行が必要。
- `git merge-base --is-ancestor 0d438ec19d9a HEAD` → exit 0。`git merge-tree --write-tree main HEAD` → exit 0（tree `d7c0705a6e14f6dc89fbd842b679f4078c084bd5`）。main の ADR-0122 / ui-ux 記録と launcher 節は両方保持。
- 最終検査: `cargo fmt --all -- --check` → exit 0。`cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo test --workspace` → exit 101。`instance_handoff` 8件中3 passed / 5 failed。`cargo test -p celeris --test instance_handoff` 単独再実行も exit 101、同じ5件を再現。3件は ADR-0095 worker db guard の user namespace 作成が `Operation not permitted` で失敗。残り2件（新旧 daemon の dispatch/standby 引継ぎ）も同じ環境で失敗した。検査は pass 扱いにしない。
- launcher binary に効く main 差分は `crates/task-worker/src/browser_runtime.rs` の init 起動待ち処理である。既存の記録どおり host の binary 入れ替えと require 試験の再実行が必要。

### pick-chrome: 並走 session での launcher Chrome 特定 — 2026-10-02

`browser_launcher_ptrace.rs` の Chrome 特定が並走 session で曖昧になって落ちていた件を、試験 file だけで直した。launcher は daemon から読める `/proc` に session の印を出さないため、launcher 子孫の新しい Chrome 候補を全部検査して 1 件以上を要求し、自分の session の停止で検査済みの session root が消えることを確かめる。選択は純粋な関数に分け、単体試験 `chrome_pick_*` 4 件を足した。詳細は `docs/progress/phase-browser-4.md`『並走 session での Chrome 特定』。

- `cargo test -p task-worker --test browser_launcher_ptrace -- --nocapture` → exit 0（5 passed、実 launcher 試験も実行）。
- `cargo clippy -p task-worker --all-targets -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- `crates/task-worker/src/` は不変（host の binary 入れ替え不要）。
