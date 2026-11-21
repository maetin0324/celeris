---
title: 戦略層 v1 子 2 contract-design — 本番の release の確認と、Strategy/Hypothesis/Evidence 契約・Project.request の prompt 配送の ADR
tasks: [01M7XQ9Q9EB3CH59TSNYM9HBC8]
status: done
updated: 2026-11-21
completed: 2026-11-21
---
# contract-design（WorkUnit。子 2 の段 design）

完了日: 2026-11-21。設計の記録は [ADR 2026-11-21-strategy-evidence-contract](../../adr/2026-11-21-strategy-evidence-contract.md)。
source（`crates/`・`web/`）は変えていない。

## 結論

- 本番の release は `53a88490a5d9` で、子 1 の統合 commit「integrate wu/gate (phase close)」（2026-11-21T12:35Z）そのものである。祖先の検査は exit 0。
  子 1 の修正は本番で動いている。設計に進んだ。
- ADR は次を決めた: schema `celeris.strategy/1` の欄と入力長の上限（数）、idempotency の key と衝突の規則、版の鎖（`supersedes` は前の版の成果物の参照）、
  承認を文書に書かず人の決定から導く規則、反証を次の版で消せない規則、置き場所（task の成果物）、承認（既存の決定）、照合（Project.request の
  sha256・承認済み Roadmap の版）、接続点（`/cos/operations` の `task.create` の任意の欄 `strategy`）、U3 の経路（`TaskLineageContext.project`）、
  試験の prefix と 4 規則の ablation の形。
- 新しい DB 表・event の種類・migration は足さない。各欄・照会を既存の何で引くかを ADR の D3.4 の表に書き、本番 DB で 3 つの照会の速さを測った。

## 本番の release と子 1 の統合 commit の祖先関係

実行した command（提出時の作業ツリーで実行）と出力の要点:

| command | 出力の要点 |
|---|---|
| `curl -s -m 5 http://127.0.0.1:7710/api/v1/health` | exit 0。`"release":"53a88490a5d9"`、`"schema_version":126`、`"role":"active"`、`started_at` 2026-11-21T14:05:28Z |
| `ls -l /local/celeris/state/current` | `releases/53a88490a5d9` への symlink（health と一致） |
| `git log --all --oneline --grep='integrate wu/gate (phase close)'` | 2 件。子 1 のものは 2026-11-21T12:35Z の merge（parents は「integrate wu/e2e-protocol (phase fix)」と「strategy-v1 1 gate: 4 回目の run の提出時の検査の結果を親の進捗に書く」）。もう 1 件は 2026-10-03 の別の task のもの |
| `git merge-base --is-ancestor <子 1 の統合 commit> 53a88490a5d9` | exit 0（祖先。release の commit が子 1 の統合 commit そのもの） |
| `git merge-base --is-ancestor <子 1 の統合 commit> main` | exit 0 |
| `git merge-base --is-ancestor 147c84783273 main` | exit 0（昇格の前の release も main の祖先） |
| `cat /local/celeris/state/releases/53a88490a5d9/promoted.json` | `promoted_at` 2026-11-21T14:05:55Z、`from` 147c84783273、含む task は子 1（01M7XQ9Q4P4ZQ38Y509J9XYAQ0） |
| `python3 -c '…gate.json…'` | `ok: True`、`failed_step: ''` |

人の決定 prod-release の答え（a: 昇格を済ませてから答える）と一致する。

## 既存の表で引けるかの測定（本番 DB、読み取り専用、2026-11-21）

DB は `file:/local/celeris/data/db/celeris.sqlite3?mode=ro` で開いた。

| 照会 | 件数 | 時間 |
|---|---|---|
| `cos_operations` の全行 / `action = 'task.create'` | 3,911 / 272 | 8 ms |
| `cos_operations` を `action`・`state`・`target_id` で引く（task がどの戦略を実行するか） | — | 9 ms |
| `decisions` の全行を走査（より新しい承認済みの版の有無） | 473 行 | 6 ms |
| 1 task の `artifact_produced`（`task_id` の索引） | — | 4 ms |
| 案件 01M2WTS3DKNZBSZ2JMVB4CZMBW のすべての `artifact_produced`（案件の全版の一覧） | 5,442（task 1,715、events 全体 984,449） | 1.65 s |
| `decision_answered` の `by` の内訳 | `human` 375・`operator:claude-code` 40・`cos` 11・`human:web` 7・`cos:<thread>` 6 | — |

gate は最後の 1.65 秒の照会を使わない設計にした（ADR D3.4）。

## WorkUnit の受入条件と根拠

- 条件 0（ADR の内容）: `agent-docs/adr/2026-11-21-strategy-evidence-contract.md` の D1（欄・上限・idempotency・版）、D2（discussion と approved・承認・反証）、
  D3（置き場所・承認・照合）、D4（接続点）、D5（U3）、D6（prefix と ablation）。
- 条件 1（新しい表・event の種類を足さない理由）: ADR D3.4 の表と、上の測定。
- 条件 2（release の確認）: 上の「本番の release と子 1 の統合 commit の祖先関係」。
- 条件 3（`crates/` と `web/` に差分が無い）: `git diff --stat "$CELERIS_WU_BASE" -- crates web` の出力が空（提出時の HEAD で実行）。

## 親の目的への寄与

- 改善 B（仮説・支持・反証・未確定を版と引用つきで共有）: 契約を決めた（確認済み: ADR に書いた）。動く実装と試験は後続の WU（contract-core・strategy-gate）で、
  この WU では未実装。
- 改善 A・C（戦略の比較と pivot）: 戦略候補・判定方法・再評価条件の欄と、反証を消せない規則を決めた。pivot が起きるかは子 5・6 の shadow ケースで確かめる（未確認）。
- observability: Project.request の全文と sha256 を run の prompt に出す経路を決めた（U3。実装は WU request-prompt、未実装）。
- LoC: source の変更 0 行（文書だけ）。

## 検査（提出時の HEAD で実行）

- `bash scripts/dev/test-affected.sh` → exit 0（「試験対象の変更なし（docs のみ等）」。repo-health は verdict=pass、new=0・worsened=0）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- `sh scripts/dev/progress-index.sh --check` → exit 0（ok）
- `sh "$CELERIS_WU_SCOPE_PATHS"` → 変更 path は ADR と この進捗 file の 2 つだけ

## commit

- `strategy-v1 2 contract-design: 戦略・仮説・証拠の契約と Project.request の prompt 配送の ADR、本番 release の確認を書く`

## 未解決

- operator が人の代わりに答える決定は、戦略の承認に数えない（ADR D2.2 の 4）。人が Web の受信箱で答える必要がある。
- `commit` の参照の存在は gate が確かめない（reviewer が確かめる）。
- 子 0 の研究ケース（BenchFS ISC27 task C）の内部利用の人の許可は未確認（子 0・1 から引き継ぐ）。

## 提案

1. `crates/task-dispatch/src/dispatcher/project_wake.rs` の局所の `sha256_hex` は `task_core::project_proposal::sha256_hex` と同じ働きの重複である。
   この task の WU の範囲には入れない。別の task で 1 つにまとめる。
