---
title: BrowserAuthority・BrowserControl の導入と consumer 移行
tasks: [01M739TT2T291EJX9KGXFY08CM, 01M739GNWHA14P7T0KBS8WZW2J]
status: done
updated: 2026-11-10
completed: 2026-11-10
---
# BrowserAuthority・BrowserControl の導入と consumer 移行

実装日: 2026-11-10。完了日: 2026-11-10（task 01M739GNWHA14P7T0KBS8WZW2J の受け入れ条件 0〜4 を提出時の HEAD で確認）。

この完了は Browser の能力分離（[worker capability ports ADR](../../adr/2026-11-10-worker-capability-ports.md) の Browser 部分）だけを指す。ADR 全体と、親 R2 の WorkerAdapter の with_* の整理は完了扱いにしない。

## 変更

commit（件名で記す。基準は `git merge-base HEAD main`）:

- `R2 adr: ADR worker-capability-ports、計測台本 measure.py と基準値、R2 の進捗`（先行段。ADR と measure.py）
- `phase 1: separate browser authority and control capabilities`（実装と試験の移行）
- `phase 2: record browser port evidence`（この進捗の記録だけ。新しい実装は無い）

[worker capability ports ADR](../../adr/2026-11-10-worker-capability-ports.md) D1/D2/D4/D5 の Browser 部分を実装する。with_*・B0 の dup・DB/API schema は変更しない。

- EventSink の Browser 10 method と BrowserSink の7転送を撤去。BrowserAuthority は6 method、BrowserControl は4 method、どちらも Send + Sync で既定実装なし。BrowserPorts と両 trait を再公開する。
- StoreSink の既存本体を新 trait へ移し、run_worker は既存 requests_browser 述語を共有して browser run にだけ能力を渡す。BrowserPorts::new は同じ実装者の2能力を借りるだけで、状態や fallback を作らない。
- browser の5入口は Option を受け、既存の素通り判定の直後、policy・台帳・substrate・harness より前に未注入を拒否する。内部 consumer と run_registered は必須の能力を受ける。EventSinkLive と launcher login は control だけを受け、forward_events は通知と scrub 済み LiveEmitter だけを受ける。
- 開始時の BrowserRun 構築と browser_updated を running_browser にまとめ、daemon と launcher の両経路で control だけを使う。ReviewerSink・ChatRunSink・PrintSink・NullSink に能力は足さない。
- 試験 sink は未使用 method も明示実装する。RegisteredSink は fixture の owner・origin・credential 参照・trusted login を照合し、API の StoreSink は実 broker の describe_policy で再検証する。既定 true は残さない。
- owner 地図の sink 行に新 trait の owner・注入入口・拒否の不変条件を記載する。

## 実装前 brief

1. 根本原因: EventSink の既定実装が未注入を隠し、saved credential 再検証は既定 true だった。
2. 責務 owner: browser_policy・browser_live・browser 入口・StoreSink・run_worker。
3. 再利用先: 既存10 method の本体、requests_browser、既存の承認・credential・control 試験。
4. 選択肢: accessor や単一 trait は採らず、ADR D2 の2 trait と参照の組を採用。
5. 追加: 既定実装のない2 trait と注入引数。新しい状態・分岐 flag・fallback は無し。
6. 撤去対象: EventSink の10 method、BrowserSink の7転送と旧い既定実装。
7. 不変条件: secret は broker 内、承認の単回消費、stale credential の拒否、auth section と control gate の制約。既存試験を削除・弱化せず移行する。

## 試験の契約

既存の前提・policy・control の拒否試験は、すべて能力を注入して従来の拒否理由を通す。新規試験は既存試験が通らない注入境界を検証する。

- browser_port_missing_refuses_before_spawn: 5入口で正確な未注入エラー、adapter.run の呼出し0回、substrate の起動印なし、runtime/session file の新規作成なしを確認。
- browser_port_none_preserves_non_browser_non_execute_planner_and_review: 4種類の素通り条件を5入口で通し、adapter.run が各1回呼ばれることを確認。

## 証拠（task 01M739GNWHA14P7T0KBS8WZW2J、提出時の HEAD で実行）

基準は `git merge-base HEAD main`。検査はすべて最後の同期（統合 `integrate wu/browser-migration (phase migration)`）の後の提出時の HEAD で実行した。この記録の commit は文書だけを変える。影響試験と userns の build は記録 commit の直前（コードは同じで、差分はこの file だけ）で実行し、fmt・clippy・measure.py・browser_port_ の試験・check-health は記録 commit の後の HEAD で再実行して exit 0 だった。この WorkUnit 自身は main の取り込み・rebase をしていない。celeris が review 前に main へ同期した場合は、同期後の HEAD で同じ検査を再実行する。

影響試験は WU の基準（`CELERIS_WU_BASE`。HEAD と同じ tree）では対象が 0 crate になる。このため `env -u CELERIS_WU_BASE` を付け、基準を `git merge-base HEAD main` にして task 全体の変更を対象にした。

| 条件 | 実行したコマンド | 出力の要点 |
|---|---|---|
| 0 | `cargo fmt --all -- --check` | exit 0、差分なし |
| 0 | `cargo clippy --workspace -- -D warnings` | exit 0 |
| 1 | `env -u CELERIS_WU_BASE bash scripts/dev/test-affected.sh --list` | exit 0。celeris・celeris-campaign・celerisctl・llm-proxy・task-api・task-dispatch・task-worker の 7 crate と web typecheck+unit |
| 1 | `env -u CELERIS_WU_BASE bash scripts/dev/test-affected.sh`（2 回目） | exit 0。Rust 5862 件すべて合格・22 skipped。web は tsc -b 合格、Vitest 104 file・712 件合格、Node 113 件合格。doc test 合格。冒頭の health は verdict=pass、new=0・worsened=0・fail_grade=0。script bundle の SKIP 0 件 |
| 2 | `python3 agent-docs/progress/2026-11-10-health-v2-r2/measure.py --check-browser-split` | exit 0、`check-browser-split: ok` |
| 3 | `out=$(cargo test -p task-worker --lib browser_port_ 2>&1); echo "$out" \| tail -n 5; echo "$out" \| grep -qE 'test result: ok\. [1-9][0-9]* passed'` | cargo exit 0、grep exit 0。2 passed（browser_port_missing_refuses_before_spawn・browser_port_none_preserves_non_browser_non_execute_planner_and_review）、1192 filtered out |
| 4 | `bash scripts/dev/check-health.sh` | exit 0。検査器の自己試験 78 件合格、repo_health_gate の 7 項目合格 |
| userns | `cargo test -p task-api --test browser_e2e --test browser_h3_injection --no-run` | exit 0、2 binary を build（実行はしていない） |

### 既存試験を残したことの確認

`git merge-base HEAD main` から HEAD までに変わった .rs file について、`#[test]`・`#[tokio::test]` の付いた fn 名の集合を比べた（`$TMPDIR` の一時台本で `git show <rev>:<file>` を読んで比較）。exit 0。

- 削除 0 件、追加 2 件（browser_tests.rs の browser_port_ の 2 件だけ）。
- 件数（基準 → HEAD）: browser_e2e.rs 5→5、browser_h3_injection.rs 3→3、live_frame_tests.rs 6→6、browser_launcher_run_tests.rs 71→71、browser_live.rs 5→5、browser_policy.rs 7→7、browser_tests.rs 35→37。
- 承認の単回消費・saved credential の再検証・auth section・control gate の既存試験は、名前を変えず能力を注入する形で残り、上の影響試験で合格した。

### Browser を使わない sink に能力が無いことの確認

`grep -rnE 'impl[^{]*Browser(Authority|Control)[^{]*for' crates/` の結果、本番の実装者は `crates/task-dispatch/src/dispatcher/sinks.rs` の StoreSink（BrowserAuthority 398 行・BrowserControl 464 行）だけ。ほかは試験の sink（task-worker の browser_tests.rs・browser_launcher_run_tests.rs・live_frame_tests.rs、task-api の browser_e2e.rs・browser_h3_injection.rs の StoreSink）だけだった。

ReviewerSink（dispatcher/sinks.rs）・ChatRunSink（cos_chat/sink.rs）・PrintSink（celerisctl commands/worker.rs）・NullSink（task-worker adapter.rs）は EventSink だけを実装し、BrowserAuthority・BrowserControl を実装しない。

### 失敗の履歴と切り分け

- 影響試験の 1 回目は exit 100。5862 件中 5861 件合格、`task-dispatch dispatcher::tests::git_workspace::review_records_each_repositories_own_definition_and_worktree_results` が 1 件失敗した（4.48 秒、`each repository must have its own evidence`）。
- 同じ試験を `cargo nextest run -p task-dispatch --lib -E 'test(=dispatcher::tests::git_workspace::review_records_each_repositories_own_definition_and_worktree_results)'` で単独に 3 回実行し、3 回とも exit 0・1 passed（約 1.5 秒）。`git diff --stat $(git merge-base HEAD main) HEAD -- crates/task-dispatch/src/dispatcher/tests/git_workspace.rs crates/task-core crates/task-dispatch/src/dispatcher/review*` の出力は空で、この task は git_workspace.rs・task-core・review を変更していない。全体実行の負荷下で `run_until_idle(&mut d, 60)` の tick 数が足りずに終わった時間依存の失敗とみられる（未確認）。2 回目の全体実行は exit 0。
- 実装 run（task 01M739TT2T291EJX9KGXFY08CM）で失敗した `launch_notice_tests` の 3 件は、この run では `CODEX_HOME` 未設定・`$HOME` が rw の mount で、影響試験の中で合格した。fixture は変更していない。
- 実装 run の途中で 1 回失敗した `browser::post_login_tests::daemon_post_login_unconfirmed_keeps_observation_stopped_and_names_the_reason`（`target_changed`）は、この run の影響試験では合格した。

## 未解決

- userns・実 browser の opt-in 試験は実行していない。task-api の 2 target は `--no-run` の build だけを確かめた。実行は release gate（`scripts/selfdeploy/release.sh`）の担当。外部ネットワーク試験も実行していない。
- 親 R2 の WorkerAdapter の with_* 整理と ADR の残り（Browser 以外の能力）は、この記録の範囲外で未完了。
- 起動前通知の試験 fixture（`crates/task-worker/src/launch_notice_tests.rs`）は、継承した CODEX_HOME が read-only の環境では失敗する。この run の環境では再現しなかったが、fixture は直していない。

## 提案

- `launch_notice_tests.rs` の fixture に試験専用の書込み可能な CODEX_HOME を明示的に渡す修正を、owner の別 task で行う。失敗の無視・承認のない環境 fallback・本番 home の書換えは行わない。
- `review_records_each_repositories_own_definition_and_worktree_results` の負荷下の失敗は、tick 数の上限ではなく `RequiredChecksEvaluated` の出来事待ちにする修正を、task-dispatch の試験 owner の別 task で行う（agent-docs/guides/testing.md の決定的再現）。
- post-login の `target_changed` の時間依存は、この task と分けて owner の試験 fixture で決定的に再現して直す。
