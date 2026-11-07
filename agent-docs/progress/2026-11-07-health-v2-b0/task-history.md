---
title: 健全性v2 B0 task-history — 既存 task の全件確認・cancelled 2 件の中止理由と現行実装の照合・review 費用の基準
tasks: [01M6TYVJSPEYA2P8B7G56HSQ73]
status: done
updated: 2026-11-07
completed: 2026-11-07
---
# 健全性v2 B0 task-history

基準は着手時の `git merge-base HEAD main`（件名「repair: main 取り込みの衝突解消の import を整形する」）。SHA は計測 JSON（`/local/celeris/data/workspaces/01M6TYVJSPEYA2P8B7G56HSQ73/artifacts/b0/task-history.json` の `base_sha`）に記録した。判断材料の比較は同じ dir の `task-history.md` に置いた。

## 確認方法（件数照合）

一覧 API（100 件で切れることがある）は使わず、本番 DB を読み取り専用で開いて数えた（`sqlite3` の URI `file:/local/celeris/data/db/celeris.sqlite3?mode=ro`。書き込みなし）。

- 親 01M6TYV9DTZESAJJMXRQVGZ56V の直下の子（`tasks.parent_id = 親`）: 12 件。
- 親の子孫（`parent_id` を再帰でたどる）: 12 件。孫は 0 件。
- 親の objective が挙げる子の数（B0・G1〜G4・R1〜R6・V0）: 12 件。3 つの数が一致した。
- 題名に「健全性」を含む task は DB 全 2,234 件中 19 件（旧 6 件＋v2 の親 1・子 12）。旧 6 件はどれも子 task（`parent_id` / `root_id`）を持たない。分解は work unit で行われ、task は増えていない。

## task id と状態（2026-11-07 計測時）

| id | 題 | 状態 |
|---|---|---|
| 01M5KWPP2W6FZAGWXFDFBQJ5S0 | 健全性 1 source-size・architecture-map 差分ゲート | done（review_pass） |
| 01M5KWPP3YPZMZ71PZP6HAGNFD | 健全性 2 必須規則の正本と harness 配布 | done（review_fail→failed→reopen を経て review_pass、rereview も pass） |
| 01M5KWPW976PBWXG4B37TQ55XC | 健全性 3 TaskStore の consumer 別分割 | cancelled（dependency_failed） |
| 01M5KWQ3WME71Q0ATMPTJA7XE7 | 健全性 4 Dispatcher の判断と副作用の分離 | cancelled（dependency_failed） |
| 01M5KWPP519YW28Q6FQRNP5184 | 健全性 5 候補 SHA・基底 SHA と検査証跡の結び付け | done（failed→reopen を経て review_pass） |
| 01M5KYMKBGCZP4MXHEF13TYAZQ | 健全性1 残件 browser.rs 分離 | done（review_pass） |
| 01M6TYVJSPEYA2P8B7G56HSQ73 | v2 B0（この task） | running |
| 01M6TYVJSRM3SFQ3VHWQE4MZ9M ほか v2 の G1〜G4・R1〜R6・V0（11 件） | | すべて ready |

v2 の子 11 件の id は計測 JSON の `parent.children` にある。

## cancelled 2 件の中止理由と現行実装の照合

### 中止理由（events から確認）

- 2 件とも 2026-10-23T10:01:45Z に `ready → cancelled`、reason `dependency_failed`。人・CoS の cancel ではない。
- 同じ秒に依存先 01M5KWPP3YPZMZ71PZP6HAGNFD（健全性 2）が `reviewing → failed`（review_fail）になった。reviewer は基準 3（architecture-map の他節に関連試験・不変条件が無い）と基準 9 で落とした。
- 健全性 2 は 10:02:57Z に reopen され、14:20Z に done になった。健全性 3・4 は cancelled のまま残り、retry も後継 task も作られなかった（他 task の json で 2 件の id を参照するのは v2 の親・R3・R4 と日次の知識抽出 task だけ）。
- 中止は TaskStore・Dispatcher の分割の中身が原因ではない。依存先の一時的な failed による自動 cancel で、着手前（run 0 本）だった。

### 現行実装（基準 commit）の照合

1. 自動 cancel の仕組みは残っている。`crates/task-core/src/store/transition.rs` の `cascade_after_transition_tx` が、failed/cancelled への遷移で終端でない後続を `Trigger::DependencyFailed` で cancel する（ADR-0009 P-9、`agent-docs/adr/0009-proposal-adoption-and-phase-closure.md`）。同じ関数は `from.is_terminal()` なら何もしないので、依存先を reopen（failed→ready）しても後続は戻らない。後続を戻すのは `retry_task_tx`（`crates/task-core/src/store/tasks.rs`）が依存を新 task へ付け替える場合だけ（cancelled かつ最後の遷移理由が `dependency_failed` の後続）。reopen 経路での復帰は無い。
2. 健全性 3 の対象は解消していない。`crates/task-core/src/store/task_store.rs` の `pub trait TaskStore` は supertrait 16 個、4 字下げの `fn ` 宣言が 193 行。`Connection` の出現は 6 行。`crates/task-core/src/store/` の直下は 74 項目。
3. 健全性 4 の対象も解消していない。`crates/task-dispatch/src/dispatcher.rs` の `pub struct Dispatcher` は 4 字下げの field 行が 115 行（親 objective の 113 から増加。数え方は awk で `name:` の行を数えた概数）。`crates/task-dispatch/src/dispatcher/` の直下は 55 項目。
4. 健全性 1・2・5 の成果は main にある（`scripts/dev/check-source-size.py`、`crates/task-worker/src/required_rules.rs`、`agent-docs/adr/2026-10-23-required-rules-prompt-delivery.md`、`agent-docs/adr/2026-10-23-required-checks-bound-to-candidate.md`）。健全性 3・4 が前提にした「1・2・5 が main に入る」は満たされている。

### R3・R4 の scope への反映案

- R3（01M6TYVJT2HG9NY5FW9FMCZS9A）と R4（01M6TYVJT4S29ZMP6P0D4831JG）は健全性 3・4 の objective を残課題として引き継ぐ。旧 task に着手の成果は無いので、旧 run の成果物を探す前提を置かない。
- R3 の出発点は上の 193・16・6 を同じ数え方で再計測した値にする。R4 は 115 を出発点にする。
- R4 は旧健全性 4 と同じく R3 に依存する。R3 が review で一度 failed になると、今の P-9 で R4 も dependency_failed で cancel され、R3 を reopen しても戻らない。v2 の親は R3→R4 を depends_on でつなぐので同じ事が起こりうる。G3（通常の開発・統合ループへの強制）か R6 で「依存先の reopen で dependency_failed の後続を戻すか、cancel を知らせる」を扱うか、少なくとも親の closeout で cancelled の子を点検する手順を置くことを提案する（提案節）。

## review 費用の基準

出典は `runs.usage_json`（role=reviewer）。GPT 系（codex）の run は `cost_usd` が 0 で記録されるので、USD は Claude 系の run だけの値。

| task | review run（Claude / GPT） | Claude の USD | Claude の cache read＋creation tokens |
|---|---|---|---|
| 健全性 1 | 0 / 1 | 0 | 0 |
| 健全性 2 | 8 / 3 | 3.41 | 5,698,593 |
| 健全性 5 | 8 / 1 | 3.01 | 3,509,736 |
| 健全性1 残件 | 2 / 1 | 0.89 | 1,356,167 |
| 健全性 3・4 | 0 | 0 | 0 |

全 task の review run（2026-10-24T00:00Z〜2026-11-07T12:00Z）: Claude 517 本（usage あり 351、cost あり 326）、USD 合計 165.73、1 本の中央値 0.45、p90 0.92。tokens（4 種の和）の中央値 494,692、p90 1,347,122。GPT 34 本、tokens 中央値 633,586（USD 不明）。

## 証拠コマンド（提出時の HEAD で実行）

- `python3 <読み取り専用の集計 script>` → `/local/celeris/data/workspaces/01M6TYVJSPEYA2P8B7G56HSQ73/artifacts/b0/task-history.json`（exit 0）。script の本文は同じ dir の `task-history.md` に載せた。

## 未解決事項

- GPT 系 review run の USD 相当は記録が無く、未計測。
- usage の無い review run（Claude 166 本）の費用は未確認。

## 提案

- 依存先が reopen されたとき、`dependency_failed` で cancel された後続を戻すか、受信箱に知らせる仕組みを G3 か R6 の scope に入れる（今は `retry` だけが後続を付け替える）。
