---
title: P2-J 導入前の安全 gate・UI・手順書（最終）
tasks: [01M6N7XYA31CMJV6RR1KXSQMKH]
status: done
updated: 2026-11-05
completed: 2026-11-05
---
# P2-J 導入前の安全 gate・UI・手順書（最終）

完了日: 2026-11-05（コード・試験・手順書まで。本番の操作は人が行うので未実施）。

## 結論
- `GET /api/v1/projects/{id}/controller` に `summary` を足した。未達・達成（Outcome ごと）、待機の理由（規則・種類・詳細）、
  承認の範囲（生きている許可と `launch_root` の判定）、次の wake（timer・backoff・未処理の種類）、guard の上限、最後の判断の追跡を返す。
  案件の画面の「自動運営」はこれを出す（ADR [2026-11-05-phase3-p2j](../../adr/2026-11-05-phase3-p2j.md)）。
- 到達は判定と同じ関数（`recorded_outcomes` を task-ops へ移し、dispatcher と API の両方が呼ぶ）と `root_satisfies` で数える。
- `docs/ops/project-controller.md` の古い記述を直し、導入の checklist（release gate・schema migration の戻し方・CoS の KB skill の版・
  shadow→canary→enabled・rollback・未実施）を足した。
- 本番の操作（promote・systemctl・本番 config・本番 DB・人だけの API の承認）はしていない。main での full release gate
  （`scripts/selfdeploy/release.sh`）も回していない（`~/.local/celeris/releases` に書くので人の操作。checklist の 1 に未実施で載せた）。

## main と既存実装の照合
- 基準は `git merge-base HEAD main`。`agent-docs/progress/2026-11-04-long-horizon.md`・a1〜a9、`2026-11-05-a9-llm-pilot.md`、
  `2026-11-05-a9-fix-arrival.md`、ADR `2026-11-05-a9-llm-pilot-findings`、`docs/ops/project-controller.md`、兄弟の p0a〜p1i を読んだ。
- 既にあったもの（再実装しない）: GET の `state`・`pending_wakes`・`last_decision`・`budget_today`（P1-G）、画面の 6 欄（A9）、
  Roadmap・許可の panel（A5・A7）、`NotGit`・`NoCommits` を取り込み済みに数えない修正（A9 D1。手順書の該当の行は直っていた）。
- 足りなかったもの: Outcome ごとの達成・未達、待機の理由の規則、承認の範囲の要約、上限（使用量だけ出ていた）、追跡。
  手順書では、repo の無い root を「done で取り込み済み」と書いた古い行（実装は成果物が要る。I4）、領域の段に `canary` と
  command の引数が無い行、migration と SCHEMA_VERSION が 113 のままの行、自動の再開（P1-G）の前の「mode を置き直すまで判断しない」。
- 旧 milestones API（410）・Project と Task の意味論・Rust の state/safety と TS の decision の分離・既存の認可は変えていない。
  TS policy・bundle は変えていない（TS policy だけの変更の verify の規則 cargo 0・p50≤30 秒・p95≤60 秒の対象ではない。Rust の変更）。
  研究 K2/K4・IEEE Access の凍結評価には触れていない。migration は足していない（SCHEMA_VERSION 114 のまま）。

## 変更（commit 件名）
- `p2j: controller の GET と画面に未達・達成・待機の理由・承認の範囲・次の wake・上限・追跡を出す`
- `p2j: controller の手順書を実装に合わせ、導入の checklist と ADR を足す`
- `p2j: 進捗と地図を書く`

| 場所 | 内容 |
|---|---|
| `crates/task-core/src/project_wake.rs` | `ProjectControllerView.summary` と型 `ProjectControllerSummary`・`ControllerOutcomeStatus`・`ControllerWaitReason`・`ControllerAuthority`・`ControllerGrant`・`ControllerNextWake`・`ControllerBudgetLimits`・`ControllerTrace` |
| `crates/task-ops/src/project_controller_view.rs` | 新規。`summary`・`recorded_outcomes`（task-dispatch から移した）・`wait_kind`・`wait_details` |
| `crates/task-dispatch/src/dispatcher/project_decider_ts.rs` | `recorded_outcomes` の定義を消し、task-ops のものを呼ぶ |
| `crates/task-api/src/handlers/project_controller.rs`・`state.rs` | `controller_view` が `summary` を作る。`ControllerApiConfig.budget` と `ApiState::with_project_controller_budget` |
| `crates/celeris/src/daemon/api.rs` | `[project_controller.budget]` の実効値を API に渡す |
| `web/features/projects/project-controller-panel.tsx` | 欄「途中目標の到達」、待機の理由、承認の範囲、次の wake の詳細、使用量 / 上限、guard の上限、最後の判断 |
| `crates/task-api/tests/project_controller.rs` | 試験 `project_wake_api_summary_shows_outcomes_wait_authority_and_next_wake` |
| `crates/task-dispatch/src/dispatcher/tests/project_achievement.rs` | 試験 `project_controller_summary_shows_reached_unreached_and_wait_reason`・`project_controller_summary_all_reached_has_no_wait_reason` |
| `web/features/projects/project-controller-panel.test.tsx` | 試験 `P2-J: 未達・達成・待機の理由・…`・`P2-J: 待機なし・許可なし・…`。`web/e2e/parity/project-controller.spec.ts` の fixture と確認を足した（e2e は release gate だけで回る。この run では未実行） |
| `docs/ops/project-controller.md` | 古い記述の修正、節「GET の要約と画面（P2-J）」「導入の checklist（P2-J）」 |
| `docs/api/v1/api.md`・`docs/SPEC.md`・`docs/architecture-map.md` | `summary` の欄、画面の説明、地図の 1 行 |
| `agent-docs/adr/2026-11-05-phase3-p2j.md` | 決定 6 件と却下した案 |

## 試験が確かめること
- 達成と未達（dispatcher の本物の判定の後）: 3 Outcome のうち 2 件に根拠の root と記録がそろうと `reached` は o1・o2 だけ、
  `evidence_root_ids` はその root、o3 は `recorded=false`・根拠なし。待機の `kind=achievement_unverified`・規則 `pc/achieved-unverified:o3`、
  `details` に未達の id、`trace` と `waiting.decision_id` は最後の判断、`next_wake.at` は state の `next_wake_at`。全部達成した案件では全件
  `reached`、`waiting` なし。
- API: off の案件は未達 2 件・待機なし・許可なし・上限は `with_project_controller_budget` の値（`max_roots_per_day=7`・`max_tokens_per_day=1234`）・
  追跡なし。人が enabled にすると許可 1 件（`launch_root`・project・残り 10 回・期限）と `launch_root_allowed=true`。外部の wake の後は
  `pending_kinds=["external_released"]`。guard（日次予算）の判断を commit すると `kind=guard`・規則 `pc/guard/budget`・`details` に guard の名と
  問いの key・要約、`trace.effect_state=pending`。
- 画面: 達成・未達の印（`data-reached`）、根拠の root、待機の理由と規則、承認の範囲と残り回数・期限、次の wake の種類、使用量 / 上限、
  guard の上限、最後の判断。待機なし・許可なし（理由の code）・timer なし・backoff・block の誤りの出し分け。

## 証拠（提出時の HEAD で実行）
| command | exit | 要点 |
|---|---|---|
| `cargo nextest run -p task-dispatch -E 'test(project_controller_summary_) \| test(project_achievement_)'` | 0 | 16 tests run: 16 passed |
| `cargo nextest run -p task-api --test project_controller` | 0 | 6 tests run: 6 passed |
| `(cd web && npx vitest run features/projects/)` | 0 | 7 files、32 tests passed |
| `bash scripts/dev/test-affected.sh` | 0 | nextest 7694 passed（22 skipped）、web 104 files・711 tests passed、ts-verify 20 passed、script bundle SKIP 0 件。9 分 40 秒 |
| `cargo clippy --workspace -- -D warnings` | 0 | 警告 0 |
| `cargo fmt --all -- --check` | 0 | 差分 0 |
| `python3 scripts/dev/check-architecture-map.py` | 0 | 922 件のパス |
| `bash scripts/dev/check-doc-links.sh`・`check-doc-layout.sh scripts/dev/docs-layout.tsv`・`check-adr-numbers.sh` | 0 | ok |
| `sh scripts/dev/progress-index.sh --check` | 0 | ok |

生記録: test-affected の出力は run の成果物の dir（`logs/test-affected.log`）に置いた。test-affected・clippy は 2 つ目の commit の時点で回した。その後の 3 つ目の commit は文書（地図・進捗）だけで、文書の検査 4 本と progress-index は 3 つ目の commit の後に回した。

## delivery-repair（release 準備の中断）
- 症状: 配送の `prepare.log` は `step cargo-test`（16:04:23Z 開始）の途中で `Terminated` 3 行で終わり、result は `{"ok":false}`。
- 原因: `journalctl --user` で、16:15:34Z に release `4bc443db7cbd` の daemon が起動し、旧 daemon（release `b8dceaa7c48f`）が
  16:15:35Z に draining・16:15:36Z に「drained; exiting 0」（in_flight 0）で終了した。旧 daemon の unit の cgroup ごと準備の子が殺された。
  旧 release `b8dceaa7`（件名 `phase 1: release target の一時的な削除失敗を再試行する`）は `delivery::prepares_in_flight` を持たない
  （`git grep prepares_in_flight b8dceaa7c48f -- crates` が 0 件）。drain が準備を待つ修正（ADR 2026-11-05-drain-waits-for-release-prepare）は
  新しい release `4bc443db7cbd` と main（このブランチの HEAD）に入っている。
- 判断: この task の変更（controller の GET・画面・手順書）とは無関係で、原因は既に main で直っている。コードは変えない。
  次の準備は修正済みの daemon（4bc443db 以降）が動かすので、drain で殺されない。
- 確認: `cargo nextest run -p celeris prepare_child_counts_as_in_flight_until_it_exits` exit 0（1 passed）。
  main の取り込み: `git merge-base HEAD refs/heads/main` は HEAD と同じ（取り込む差分なし）。
- 本番の操作（準備の再実行・promote・systemctl）はしていない。準備の再実行は配送側が行う。

## 失敗した場合・未確認
- 最初の biome check が 2 件の整形で落ちた（`project-controller-panel.tsx` の JSX の改行）。`biome check --write` で直し、vitest を再実行した。
- web の e2e（`web/e2e/parity/project-controller.spec.ts`）は release gate だけで回るので、この run では実行していない。fixture に `summary` を足した。
- `summary` は GET のたびに snapshot（案件の全 root の events）を作る。案件の root が多いときの応答時間は測っていない（未確認。提案 1）。

## 未解決・次の gate（人の判断）
- 本番の導入（`docs/ops/project-controller.md` の「導入の checklist（P2-J）」1〜5）: 全部未実施。人が行う。
- P0-F（実 LLM の縮小 pilot の再試験）: 未実施。人の許可待ち（account・model・token と時間の上限）。P0-F の進捗は main に未取り込み。
- 24〜72 時間の非本番の実稼働と本番 RSS の off 比較（P1-I の段 3・4）: 未実施。人の承認待ち。

## 本番への移行
- config の欄は足していない・消していない。本番 config の書き換えは要らない（`[project_controller.budget]` は既存の欄で、GET はその値を出すだけ）。
- migration は無い。CoS の skill は変えていない（cos-operator 7、cos-inbox-triage 11 のまま）。

## 提案
1. `summary` の組み立ての時間を、root が数百件の案件で測る（遅ければ outcomes だけ別の endpoint にするか、判断の時の snapshot の hash で
   cache する）。
2. 待機の理由の `kind` を Rust の規則の id の定数から作る（今は接頭辞の文字列で判定している。規則の名を変えると `other`・`wait` に落ちる）。
