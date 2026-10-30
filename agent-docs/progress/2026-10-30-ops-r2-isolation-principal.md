---
title: ops-r2 の入力隔離・依頼主体・実験専用 operator session の設計（WU design）
tasks: [01M669QB069GND2ZKA1T97QPY9]
status: done
updated: 2026-10-30
---
# ops-r2 の入力隔離・依頼主体・実験専用 operator session の設計（WU design）

完了日: 2026-10-30

この WU は既存の仕組みを読み、ADR `agent-docs/adr/2026-10-30-ops-r2-isolation-principal.md` を書いた。コード（`crates/`・`scripts/`）は変えていない。

## 読んだもの

- `crates/campaign/src/isolation.rs`・`preflight.rs`・`ops_r1_intake.rs`・`operator_usage.rs`・`human_ops.rs`・`harness_exec.rs`（A の bwrap）
- `crates/task-worker/src/db_guard.rs`（worker・CoS run の namespace。DB と host 設定を ro にするだけで、他の host の path は見える）
- `crates/task-api/src/chat/mod.rs` の `post_relay`、`crates/task-core/src/chat/mod.rs` の `ChatRelayRequest`・`ChatRelay`
- `scripts/campaign/check-ops-r1-leak.py`、`docs/ops/ops-r1-leak-check.md`・`ops-r1-measure-daemon.md`・`ops-r1-intake.md`・`external-operator.md`・`operator-usage.md`
- `agent-docs/progress/2026-10-29-k4-ops-r1-pilot.md` と `actors.md`・`fix-leak.md`・`measure-e2e.md`・`auth-addendum.md`
- ADR 2026-10-27-relayed-instructions・2026-10-27-human-ops-actor-roles-request-ref・2026-10-29-stagnation-triage-notify-wiring

## 決めたこと（ADR の要点）

- 境界は 2 層。L1 は人の決定 sep-uid（評価器専用 uid `celeris-eval` の 0700 の dir に private grader を置く）。
  L2 は試行の主体（A の agent、B の測定 daemon の CoS と worker、実験専用 operator session）を、許可した path だけを mount した bwrap
  （`--unshare-pid` と新しい `/proc`）で動かす。旧解答・他試行の出力・git に残った private は L2 で塞ぐ。
- B は測定 daemon 全体を bwrap で起動し、試行ごとに `--state` を分ける。CoS run と worker run は daemon の子なので同じ namespace を継ぐ（CoS 直接遂行と task 経由の両方に効く）。
- deny_readable は主体ごとに、その主体を起動するのと同じ包み方の中から `probe-read` で確かめ、1 件でも読めれば preflight の `missing` に入れて正式試行を止める。
- 依頼は既存の `POST /chat/threads/{t}/relays`（`operator:ops-r1` principal）で入れる。`ops-r1-intake` の `--send`（人の token）を消して `--relay` に替え、
  intake 記録 `/2` に出典（本番の message・decision の id、`said_at`、承認票の依頼文 sha256）と decided_by・executed_by・approved_by・relayed_by を持たせる。新しい権限・恒久承認は作らない。
- 人の能動時間は `actor.kind=human` の、start と end がある区間の和集合だけ。人の op 行が 0 件の試行は null（`not_recorded`）。
- 実験専用 operator session は開始前に session id（`--session-id`）・新しい `CLAUDE_CONFIG_DIR`・初期 context・`session-map.base.json` を作る。
  poll と記録は script、LLM の介入は human-op の op 行に分け、最終の session-map は base と op 区間から決定的に作る。schema（human-ops /2、operator-usage /1）は変えない。
- 後続 unit（isolate・principal・session・probe・stall・close）の変えてよい file と、要る OS 権限・credential を ADR の表に書いた。

## 見つけた事実

- K2 と K4 の final の private.json が repo（`crates/campaign/fixtures/grader-private/final/`）に入っており、人の uid のすべての clone と git object から読める。
  L1 だけでは塞げない（ADR の「人に決めてもらう点」1）。
- B の測定 daemon に渡す account dir には過去の session の transcript が入る。測定専用の account dir が要る。
- K4 の measure-e2e の試験（`crates/celeris/tests/ops_r1_measure.rs`）は daemon.toml に人の token（`human:ops-r1-e2e`）を足している。principal unit で relay に替える。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

- `unshare -Ur true` → exit 0
- `bwrap ... --unshare-pid /usr/bin/unshare -Ur /usr/bin/true` → exit 0（bwrap の中で user namespace を作れる）
- 同じ bwrap で `cat /home/rmaeda/workspace/agent-platform/Cargo.toml` → `No such file or directory`
- `git diff --name-only "$CELERIS_WU_BASE" -- crates scripts` → 出力なし（受け入れ条件 2）
- 文書の検査の結果は報告本文に書く

## 未解決

- db_guard の mount 操作が bwrap の中の測定 daemon で通るかは未確認（probe unit で確かめる）。
- release dir に repo の source（fixtures）が入っていないことは未確認（close 段）。
- K4 証拠確定 task 01M669QAZE の人の能動時間の規則（cov-not-recorded）は未統合。統合後に D6 と揃っているかを probe unit が確かめる。

## 提案

- 人に決めてもらう点 2 件（K2 の private の扱い、承認した依頼文の relay）を decisions として出した（result.json）。
