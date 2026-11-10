---
title: BrowserAuthority・BrowserControl の導入と consumer 移行
tasks: [01M739TT2T291EJX9KGXFY08CM]
status: blocked
updated: 2026-11-10
---
# BrowserAuthority・BrowserControl の導入と consumer 移行

実装日: 2026-11-10。完了日: 未完了（受け入れ条件3の影響試験が範囲外 fixture で失敗）。

## 変更

- commit 件名: `phase 1: separate browser authority and control capabilities`。

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

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）

管理側が用意した作業状態から main 同期・rebase は行っていない。以下は最後のコード変更後の検査結果。進捗の更新後にも文書検査を再実行した。

| 条件 | 実行したコマンド | 出力の要点 |
|---|---|---|
| 0 | `python3 agent-docs/progress/2026-11-10-health-v2-r2/measure.py --check-browser-split` | exit 0、check-browser-split: ok |
| 1 | `cargo nextest run -p task-worker --lib -E 'test(browser_port_) \| test(browser::) \| test(browser_live::)'` | exit 0、125 passed、1069 skipped |
| 2 | `cargo test -p task-api --test browser_e2e --test browser_h3_injection --no-run` | exit 0、2 binary を build |
| 3 | `bash scripts/dev/test-affected.sh --list` | 7 crate と web typecheck+unit |
| 3 | `bash scripts/dev/test-affected.sh` | exit 100。Rust 5862件中5859 passed・3 failed・22 skipped。web tsc -b 合格、Vitest 104 file/712件合格、Node 113件合格。冒頭の health は exit 0、new=0/worsened=0/fail_grade=0。失敗で doc test の段には未到達 |
| 3 | `cargo clippy --workspace -- -D warnings` | exit 0。vendor の deno_core に既存 deprecated 警告あり |
| 3 | `cargo fmt --all -- --check` | exit 0、差分なし |
| 3 | `bash scripts/dev/check-health.sh` | exit 0、new=0/worsened=0/fail_grade=0。検査器の1件と78件、gate 7項目も合格 |
| 4 | `sh scripts/dev/check-doc-links.sh && sh scripts/dev/check-adr-numbers.sh && sh scripts/dev/progress-index.sh --check` | exit 0、リンク正常・ADR 415 file 正常・progress-index 正常 |

既存試験の削除は無し。試験名の集合を HEAD の変更前と比較し、5試験ファイルすべて removed=[]、追加は browser_port_ の2件だけだった。

### 失敗の履歴と切り分け

- Browser lib の途中の再検査では、既存の `browser::post_login_tests::daemon_post_login_unconfirmed_keeps_observation_stopped_and_names_the_reason` が `login submitted: "target_changed"` で1件失敗した。初回と最終コードの再実行は125件すべて合格。この試験の本体は変更していない。
- 影響試験の失敗は `launch_notice_tests::a_spawn_failure_keeps_the_notice`・`launch_notice_tests::a_failed_notice_prevents_the_spawn`・`launch_notice_tests::notice_is_called_before_the_process_is_spawned` の3件。Codex の before_spawn に届かず、通知回数0・順序ログ空となった。
- `cargo nextest run -p task-worker --lib -E 'test(launch_notice_tests::)'` → exit 100、同じ3件が単独実行でも失敗。
- `findmnt -T "$CODEX_HOME" -n -o TARGET,VFS-OPTIONS` → exit 0、`/home/rmaeda ro,nosuid,nodev,relatime`。継承 CODEX_HOME はこの mount 内。`codex.rs` の `run_attempt` は通知より前に `install_systemd_deny_rules` で CODEX_HOME/rules へ書くが、起動前通知の試験 fixture は専用 home を渡していない。
- `git diff --exit-code HEAD -- crates/task-worker/src/codex.rs crates/task-worker/src/launch_notice_tests.rs` → exit 0、どちらもこの変更の差分なし。EventSink の before_spawn の宣言と既定実装も変更なし。
- control-ts の optional bundle は offline install に失敗して渡されなかった。script は続行したが、Rust の3件の失敗で終了した。

## 未解決

受け入れ条件3は未達。`crates/task-worker/src/launch_notice_tests.rs` は expected_write_paths の外なので変更しない。ADR D4b の追加許可は browser 入口の直接試験が対象であり、この非 browser の fixture には適用しない。

userns・実 browser の opt-in 試験は実行していない。API の2 target は no-run build と、影響試験が既定で走らせる sandbox 内の試験だけを検証した。外部ネットワーク試験は実行していない。本番 account 設定・CODEX_HOME・host は変更していない。

## 提案

plan_issue: 起動前通知の試験 fixture の owner である `crates/task-worker/src/launch_notice_tests.rs` を許可範囲へ追加し、Codex adapter に試験専用の書込み可能な home を明示的に渡す。その後、影響試験を含む受け入れ条件3を再実行する。失敗の無視・承認のない環境 fallback・本番 home の書換えは行わない。

post-login の `target_changed` の時間依存は、この task と分けて owner の試験 fixture で決定的に再現して直すことを提案する。
