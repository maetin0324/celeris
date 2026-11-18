---
title: アーキ v4 D adr-forms — ADR 2026-10-09 D5 への org の形の付記と、D の ADR への使い分け・撤去の条件の付記
tasks: [01M7MZTKB4RYM80S6CT3RST2N9]
status: done
updated: 2026-11-18
completed: 2026-11-18
---
# adr-forms（WorkUnit）

完了日: 2026-11-18

基準は `$CELERIS_WU_BASE`（この WorkUnit の base。件名「arch-v4 D adr: …」の commit を含む）。`crates/`・`web/`・`scripts/` は変えていない。

## 変更
- `arch-v4 D adr-forms: …` — 次の 2 つの ADR に付記を足し、この進捗を書いた。
  - [ADR 2026-10-09-cos-operations-all-mutations](../../adr/2026-10-09-cos-operations-all-mutations.md): D5 の「`Applied::Audited` を返す」段落の直後に
    1 行の参照、末尾に「付記: org の書き込みの形（2026-11-18、task 01M7MZTKB4RYM80S6CT3RST2N9）」。
  - [ADR 2026-11-18-org-write-shared-use-case](../../adr/2026-11-18-org-write-shared-use-case.md): 末尾に「付記: 2 つの形の使い分けと撤去の条件」。

## 付記の要点
- ADR 2026-10-09 の付記: org は `org_write.rs` の plan と tx 内の apply を、HTTP が `SqliteStore::write_tx`、CoS が `OperationAudit::apply_checked`（plan の拒否は
  `reject`）で包む形に決めた（実装は WorkUnit `usecase`・`store-prune`。付記を書いた時点では未実装と明記）。D5 の `Applied::Direct`／`Applied::Audited` は
  org に当てはまらない。新しい CoS 業務操作は既定でこの形で書く（共有の操作は `handlers`・`cos` に依存しない module に置き、`Option<&OperationAudit>` と
  `Applied` を受けさせない）。D5 の同一 transaction の条件と外部副作用の扱いは変えない。
- D の ADR の付記: 新しい操作の既定は org_write の形。`Applied` の形を残す file の一覧（下）。撤去の条件は「その領域の共有の操作を変える task が同じ変更で移す。
  全部が移ったら `Applied` 型・`Option<&OperationAudit>` の引数・ADR 2026-10-09 D5 の `Applied` の記述を消す」。期限は親 01M7MZT8KX547464YYS76QNP4G の子 H の
  完了報告（残数を数え、領域ごとに移行の task を提案するか残す理由を書く）。責任者は、移行がその領域を変える task の担当、残数の確認と提案が子 H、起票の判断が人。
  D が全部を移さない理由（依頼が org と admin の周辺の狭い pilot に限った）。

## 数えた file（この WorkUnit の HEAD。`usecase` の前）
- objective の command `git grep -l 'Option<&OperationAudit>' crates/task-api/src` → 14 file:
  `approvals.rs`・`changes.rs`・`cos/operations.rs`（型の定義側）・`decisions.rs`・`docs.rs`・`handlers/projects.rs`・`handlers/task_actions.rs`・
  `handlers/tasks.rs`・`integrate_task.rs`・`knowledge.rs`・`lifecycle.rs`・`model_catalog.rs`・`repos.rs`・`skills.rs`（すべて `crates/task-api/src/` の下）。
- 数え方の違い: 上の command は完全修飾の `Option<&crate::cos::operations::OperationAudit>` と lifetime 付きの別名 `Option<&'a OperationAudit>` を拾わない。
  `git grep -lE "Option<&('[a-z_]+ )?(crate::cos::operations::)?OperationAudit>" crates/task-api/src` → 19 file（上の 14 に `cron_jobs.rs`・`execution.rs`・
  `handlers/clusters.rs`・`handlers/org.rs`・`inbox_notifications.rs` が加わる）。`git grep -lE '\bApplied\b' crates/task-api/src` も同じ 19 file。
  両方の値を ADR の付記に書いた。関数ごとの一覧も ADR の付記の表にある。
- `usecase` の後の予想（約束ではない）: 狭い command 14 のまま、広い command 19 → 18（`handlers/org.rs` が消える）。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- 上の 3 本の `git grep` → 14・19・19 file（一覧は上）。
- 範囲: `git diff --name-only "$CELERIS_WU_BASE" -- crates web` → 出力なし（crates/・web/ の差分 0）。
- `sh "$CELERIS_WU_SCOPE_PATHS"` → 変更 path は `agent-docs/adr/` の 2 file と `agent-docs/progress/2026-11-18-arch-v4-d/adr-forms.md` だけ。
- `sh scripts/dev/progress-index.sh --check` → exit 0。
- cargo の検査（test-affected・clippy・fmt）は走らせていない。source を変えていないため。

## 未解決
- 無し。付記の「org はこの形にした」は D の決定を書いたもので、実装（`usecase`・`store-prune`）の後に `close` が実装と一致しているかを確かめる。

## 提案
- 子 H の完了報告で残数を数えるとき、objective の狭い command だけでは 5 file が漏れる。H は広い command の値を主に使い、狭い command の値を並べて書くとよい。
- 子 F が `knowledge.rs` の `put_page_op`・`record_op`・`accept_op`・`reject_op` を変えるなら、撤去の条件によりその操作は同じ変更で org_write の形へ移す対象になる。
