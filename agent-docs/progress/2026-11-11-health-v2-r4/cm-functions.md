---
title: "健全性v2 R4 cm-functions: 大きい 7 関数の判断と副作用の区間"
tasks: [01M74VAMWFCC4JGZY4ENA92HJJ]
status: completed
updated: 2026-11-11
---
# 健全性v2 R4 cm-functions: 大きい 7 関数の判断と副作用の区間

WorkUnit cm-functions の記録。コードは変えていない。書いたのは `agent-docs/progress/2026-11-11-health-v2-r4/change-map.md`（7 関数の節）とこの file だけ。

## 結果

`change-map.md` に `## 関数: <名>` の節を 7 つ書いた。各節に、位置・区間の表（区間・種類・読む状態・書く状態・呼ぶ helper）・判断と副作用が交互に入る所・純粋な入力→提案の案（型の名前）・owner 候補がある。行番号は base（`$CELERIS_WU_BASE`）の HEAD のもの。

| 関数 | 位置 | 行参照の数 | owner 候補（主） |
|---|---|---|---|
| finish_worker_result_with | `crates/task-dispatch/src/dispatcher/worker_finish.rs:337-1630` | 39 | run-supervisor |
| dispatch_one | `crates/task-dispatch/src/dispatcher/dispatch_run.rs:35-1145` | 62 | execution-coordination |
| spawn_review | `crates/task-dispatch/src/dispatcher/review_spawn.rs:347-1094` | 31 | execution-coordination |
| on_review_finished | `crates/task-dispatch/src/dispatcher/review_verdict.rs:20-613` | 33 | execution-coordination |
| reclaim_expired_leases | `crates/task-dispatch/src/dispatcher/leases.rs:217-436` | 22 | run-supervisor |
| route_state | `crates/task-dispatch/src/dispatcher/route_select.rs:757-978` | 14 | routing |
| resolve_quota_estimate | `crates/task-dispatch/src/dispatcher/quota_book.rs:119-332` | 20 | routing |

主な所見（後の unit・R4 の設計への材料）:

- 書き込みが複数の transaction に分かれている: finish_worker_result_with は WU 遷移と run index の終了を task 遷移より前に別々に書く。on_review_finished は delivery の行と run index を task 遷移より前に書く。どちらも task 遷移が `InvalidTransition` で落ちると、先の書き込みだけが残る。
- 書いた直後の DB を読み直して判断する: finish_worker_result_with の `settle_phase`（worker_finish.rs の 1395）、spawn_review の integration repair の解決（review_spawn.rs の 803）。
- 判断の helper が書き込みも持つ: `deferred_work_unit_trigger`・`wu_dispatch_gate`・`target_resync_exhausted`・`pick_reviewer`・`escalate_failed_children`。
- 同じ処理の写し: on_review_finished の「reviewer run の `WorkerFinished` と quota を追記し run index を閉じる」が 4 か所にある。
- quota group の event が 2 つの書き方に分かれる: resolve_quota_estimate は他 member の event を直接追記し、自分の event は呼び出し側の transaction で書く。
- 本番の関数の中の `#[cfg(test)]` 分岐: route_state の 964-972、finish_worker_result_with の 1324-1327。

## 証拠コマンドと結果（提出時の HEAD で実行）

- 節・表・行参照・案・owner の検査（python3 の一時 script。`## 関数: ` で分け、`crates/...rs:行` の参照の file の存在と行数を確かめる）: 節 7、各節の参照 14〜62 件、範囲外の参照 0、表・案・owner 候補はすべての節にある。7 桁以上の hex 語 0。
- 主な行参照 25 件を `sed -n <行>p` で読み、本文の記述と一致することを確かめた。
- `sh "$CELERIS_WU_SCOPE_PATHS"`: 変更 path は `agent-docs/progress/2026-11-11-health-v2-r4/` の下だけ。
- `sh scripts/dev/check-doc-links.sh`: exit 0（`check-doc-links: ok`）。
- `sh scripts/dev/progress-index.sh --check`・`bash scripts/dev/test-affected.sh`・`cargo fmt --all -- --check` の結果は報告本文（result.json）に書く。
- `cargo clippy --workspace -- -D warnings` は実行していない。Rust の file を変えていないので、結果は base と同じになる。

## commit

- `r4 cm-functions: record decision/side-effect intervals of 7 dispatcher functions`

## 未解決事項

- owner 候補 8 つの意味は change-map.md の冒頭に「この文書での読み」として書いた。cm-owners の field の割り当てと食い違えば、cm-owners を正として合わせる必要がある（cm-record で確かめる）。
- helper の副作用の有無は、helper の本体の `store.*` 呼び出しの grep と目視で決めた。`task_ops` の関数に `store` を渡して書く helper（`try_review_repair` など）は、log の文言とコメントから書き込みありと判断した。task_ops の中までは追っていない。

## 提案

- R4 の分割は、まず「transaction が分かれている所」（finish_worker_result_with の 1289-1387 と 1553、on_review_finished の 388・421 と 502-560）を 1 つの提案（`RunFinishProposal`・`ReviewVerdictProposal`）にまとめて 1 transaction で書く所から始めるとよい。純粋関数への切り出しと、遷移失敗時の部分書き込みの解消が同時にできる。
- on_review_finished の 4 か所の写しは、提案の `reviewer_run_end` 1 つにまとめる。
