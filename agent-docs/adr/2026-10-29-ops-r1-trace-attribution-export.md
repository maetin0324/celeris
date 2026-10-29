---
tasks: [01M62VP61K6XHRJW0H2YVX7N4D]
---
# ADR 2026-10-29: ops-r1 B の trace と trial 別消費は測定 daemon の DB の写しから読み取り専用で出す

- 状態: 採用（設計。実装は同じ task の trace・attr の葉）
- 関係: ADR 2026-10-29-ops-r1-cos-intake-record（受付の記録 `ops-r1-intake/1`、task 01M62TJ5V2B65Y0C804HHQ4E3Q の branch にあり、この branch には未取り込み）、
  [2026-10-15-root-done-cos-summary](2026-10-15-root-done-cos-summary.md)（報告 message の経路）、
  ops-r1 棚卸しの g-b-trace-export・g-b-trial-attribution。仕様の本文は `docs/ops/ops-r1-trace-attribution.md`

## 1. 文脈
ops-r1 の B は「人が案件 CoS に頼み、CoS が起票し、task が走り、成果が確定し、CoS が報告する」経路を測る。
既存の `celerisctl campaign export` は `task.campaign` か campaign の活動窓で消費を結ぶが、CoS が起票した task は
campaign 識別を持たない。actor_trace は試験の中でしか照合していない。測定 daemon は本番昇格済み release
94f86928dc13 の binary のまま使うので、daemon・task-*・web のコードは変えられない。

## 2. 決定
- D1: `celeris-campaign ops-r1-trace` と `ops-r1-attribution` を足す。入力は DB の写し（`.backup` で作った単一ファイル）で、
  `mode=ro&immutable=1` の URI と `SQLITE_OPEN_READ_ONLY` で開く。`SqliteStore::open`（migration を走らせる）は使わない。
  `schema_migrations` の最大版が 102 でなければ止まる。
- D2: 6 段の取り元は `chat_messages`（受付・報告）、`cos_operations` の `task.create` と `events` の `cos_operation` 監査（起票）、
  `tasks.parent_id` の再帰と `runs`（task と run）、root の `transitioned` と reviewer/検査の event（成果確定）、
  `root_done_summaries` → `chat_runs` → `chat_messages`（報告）。欠けた段は null と固定の reason 語彙で出す。
- D3: cutoff は引数 `--cutoff` で必須にする。報告 message から決めると報告の欠けた試行で決まらず、実行時の時計を使うと
  byte 一致が崩れる。condition_id も DB に無いので `--condition-id` で渡す。
- D4: trial への帰属は DB の参照だけで決める。root 群（受付以後 cutoff 以前に `--thread` で applied の `task.create`）の
  子孫の run、`--thread` の CoS turn、統括 CoS（general・review thread）の turn は試行の task・案件を対象にした
  `cos_operations` か root_done の依頼を参照するときだけ帰属させる。結べない行は捨てずに `trial_id: null` と reason で出し、
  帰属行と未帰属行の合計が DB の候補行の全量になるようにする。
- D5: 監督 CoS（外部 operator・本番 daemon の CoS）の行は入れない。測定 daemon の DB に無く、別の入力を結ぶと
  1 つの DB からの byte 一致が成り立たない。B の条件の外の支援として operator_usage・human_ops で別に数える。
- D6: byte 一致のため、時刻は parse して比べ UTC ミリ秒の `Z` 表記で出す。並びは固定の key で決め、実行時刻・host・
  絶対 path は出さない。DB は写しの sha256 で同定する。

## 3. 検討した代案
- daemon に trial 識別を持たせる（CoS 起票 task に campaign を継承させる）: daemon の変更になり、測定対象の binary が
  94f86928dc13 でなくなる。DB の参照だけで足りるので採らない。
- `decision_log_usage` から消費を読む: 行は daemon の projection（書き込み）で作られ、CoS 起票 task は campaign を持たない。
  元の `runs.usage_json`・`chat_runs.usage_json` を直接読む。
- 統括 CoS の turn を時間窓だけで試行に入れる: 試行と無関係な turn を混ぜるので採らない（D4）。

## 4. 判定
verdict は feasible（daemon の変更は要らない）。統括 CoS が読むだけで操作をしない turn は試行に結べず未帰属に残る。
これは推測で埋めない（D4）。
