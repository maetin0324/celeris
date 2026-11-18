---
title: アーキ v4 D adr — org の書き込みを共有 use-case（org_write.rs）へ集約する設計と before の値
tasks: [01M7MZTKB4RYM80S6CT3RST2N9]
status: done
updated: 2026-11-18
completed: 2026-11-18
---
# adr（WorkUnit）

完了日: 2026-11-18

ADR: [2026-11-18-org-write-shared-use-case.md](../../adr/2026-11-18-org-write-shared-use-case.md)。基準は `$CELERIS_WU_BASE`。
before の計測点は `git merge-base HEAD main`（件名「integrate wu/loc-fix (phase fix)」の commit。アーキ v4 C の統合の後）。

## 変更
- `arch-v4 D adr: …` — ADR（根本原因・決定・比べた案・撤去・不変条件と試験・計測の 6 節と実装前 brief の対応）と、この進捗を足した。
  `crates/`・`web/`・`scripts/` は変えていない。

## 決めたことの要点
- D1: `crates/task-api/src/org_write.rs` に `BrowserSettingsPatch`・`OrgWrite`・`load_org_node`・`plan_create`・`plan_patch`・`plan_browser_settings`・
  `plan_delete` と、呼び出し元の transaction の中で書く `apply(tx, write, actor) -> OrgWritten` を置く。依存は `task_core`・`crate::problem`・`crate::types` だけ
  （`problem`・`types` は task-api の SCC の外。C2 の JSON で確認した）。
- D2: HTTP（`handlers/org.rs`・`skills.rs`）は `require_admin` の後 `SqliteStore::write_tx` で `apply(.., "admin")` を呼ぶ。
- D3: CoS（`cos/ops/admin.rs`）は plan の失敗を `audit.reject`、適用を `audit.apply_checked` の tx の中の `apply(.., "cos")` で行う。
- D4: actor・response・problem の code・削除で行が無いときの扱い（HTTP は今と同じ、CoS は `org_node_not_found`）・認可を変えない。
- D5: 新しい型は `OrgWritten`（`Node`・`Deleted { id, existed }`）だけ。trait・Port・Facade・fallback・flag は足さない。
- 撤去: `org_commit`、org での `Applied` の使用、admin.rs の削除の inline plan、`SqliteStore::org_upsert_browser_settings`・`org_upsert_with_actor`・`org_delete`
  （試験は `org_delete_tx` へ移す）。

## before の値（A の §7 C2・C5 を同じ規則で当てた）

C2（循環の全部の辺。A の §7 C2 の snippet の検査器の dir を第 1 引数にした `raw/c2.py`）:

| 指標 | A（旧い検査器 × `bc665168`、この run で再実行して一致） | before（HEAD の検査器 × `git merge-base HEAD main`） |
|---|---|---|
| 循環の件数、単位・組・行の合計 | 17、192・742・1,731 | 17、198・756・1,827 |
| task-api の SCC の単位・組・行 | 49・261・628 | 49・261・632 |
| `handlers → cos` の行の辺 | 22 本・13 file（`handlers/org.rs` 3） | 22 本・13 file（`handlers/org.rs` 3） |
| `cos → handlers` の行の辺 | 48 本・9 file（`cos/ops/admin.rs` 11） | 52 本・9 file（`cos/ops/admin.rs` 15） |
| org の周りの辺（D が消す対象） | 10 行: `handlers/org.rs:99-101`、`cos/ops/admin.rs:313,339`、`skills.rs:412,431,446,451`、`schema.rs:119` | 同じ 10 行 |

- 数え方の違い: B の検査器は `super::` の path を解決する。`cos/ops/admin.rs:425,470,481,513` の `super::super::super::handlers::…` 4 行が辺になり、
  cos → handlers が 48 → 52、task-api の行が 628 → 632 になった（組は既存の cos→handlers なので 261 のまま）。値の違いは数え方だけによる。
- 旧い検査器 × merge-base は測れない: `profile_invalid: scripts/dev/repo-health/profiles/celeris.toml: review.high_risk[0] は class・paths だけ`
  （C が profile に `invariants` の欄を足したため）。したがって D の before と after は HEAD の検査器で比べ、A の値は並べて書くだけにする。

C5（`git grep` の 2 本。A の §7 C5 のまま）:

| 指標 | A（`bc665168`、この run で再実行して一致） | before（`git merge-base HEAD main`） |
|---|---|---|
| `crate::cos::` を書く handlers の行 | 22 行（13 file） | 22 行（13 file） |
| `(crate\|super(::super)*)::handlers` を書く cos の行 | 52 行（crate:: 48 ＋ super:: 4、9 file） | 52 行（crate:: 48 ＋ super:: 4、9 file） |

after の予想（約束ではない。`metrics` が測る）: C5 は 19 行 / 50 行、C2 の task-api の SCC の行は 632 → 622、単位 49・組 261 は変わらない見込み。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- C2: `python3 -I raw/c2.py <検査器の dir> <rev> <out.json>` を 3 通り（旧い検査器 × `bc665168`、旧い検査器 × merge-base、HEAD の検査器 × merge-base）。
  検査器は `git archive <rev> scripts/dev | tar -x -C $TMPDIR/<dir>` で取り出した。旧い検査器 × merge-base だけが上の `profile_invalid` で exit 1、他の 2 つは exit 0。
  出力と JSON は run の artifacts の `raw/`（`c2-*.txt`・`cycle-edges-*.json`・`c5-*.txt`）。
- C5: `git grep -nE 'crate::cos::' <rev> -- crates/task-api/src/handlers crates/task-api/src/handlers.rs | grep -vE "$X" | wc -l` と
  `git grep -nE '(crate|super(::super)*)::handlers' <rev> -- crates/task-api/src/cos | grep -vE "$X" | wc -l`（`$X` は A の §7 C4 の除外の式）→ 上の表。
- 範囲: `git diff --name-only "$CELERIS_WU_BASE" -- crates web` → 出力なし。

## 未解決
- C7b・C12 の before は `metrics` が after と同じ時に測る（この WorkUnit では測っていない）。

## 提案
- 無し。
