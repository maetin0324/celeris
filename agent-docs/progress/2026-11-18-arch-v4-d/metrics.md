---
title: アーキ v4 D metrics — A と同じ計測で org の集約の before/after を記録する
tasks: [01M7MZTKB4RYM80S6CT3RST2N9]
status: done
updated: 2026-11-18
completed: 2026-11-18
---
# metrics（WorkUnit）

完了日: 2026-11-18

統合後の HEAD（`integrate wu/store-prune (phase impl)` の後）を after、`git merge-base HEAD main` を before とし、
A（[2026-11-17-arch-v4-a](../2026-11-17-arch-v4-a.md) と A の run の `artifacts/baseline.md` §6・§7）の計測コマンドで同じ規則で数えた。
A の基準 `bc665168` の値も並べた。source は変えていない（変更はこの file だけ）。検査は提出時の HEAD で実行した。

## 0. 結論

- C5 の 2 つの行数は before より少ない: handlers → cos は 22 → 19 行、cos → handlers は 52 → 50 行。
- C2: task-api の SCC は 49 単位・261 組のまま。行の辺は 632 → 620（−12）。handlers → cos は 22 本・13 file → 19 本・12 file、
  cos → handlers は 52 本・9 file → 50 本・9 file。新しい単位 `org_write` はどの SCC にも入っていない。
  handlers ⇄ cos の単位の循環は残る（org 以外の領域の辺が残るため）。
- 減らなかった・増えた項目:
  - production の LoC は ±0（375,763 → 375,763、Rust 331,072 → 331,072）。試験は +980 行。
  - C4 の `rusqlite` を書く file は 74 → 75、C6 の `&Transaction` の引数は task-api で 14 → 15 個（9 → 10 file）。どちらも `org_write::apply` の `tx` の引数が原因。
  - S1 の変更波及は file 3 → 4・単位 3 → 4・crate 2 → 2。C7b の直接 93 → 93、推移 365 → 365、package 12 → 12（減らない）。
  - `Applied` の形を残す file: 狭い command は 14 → 14、広い command は 19 → 18。
- 減った項目: SqliteStore の `pub` method 633 → 631、private 42 → 41。S1 の file が handlers ⇄ cos の循環に持つ辺は 5 行 → 0 行。

## 1. 計測の条件

- before = `git merge-base HEAD main`（以下 mb）、after = 提出時の HEAD、A = `bc665168c9caf1898adc4c611404bbee056daf9a`（以下 b0）。
- 検査器: HEAD の `scripts/dev/repo-health`（B の検査器。mb と HEAD で `scripts/dev` の差は 0 行）。A の値の再現には b0 の `scripts/dev`（旧い検査器）も使った。
  旧い検査器は mb・HEAD の profile を読めない（D の adr.md の「旧い検査器 × merge-base は測れない」）。したがって mb と HEAD は HEAD の検査器だけで比べる。
- 分類規則（source-size-report）: mb から `git show` で取り出した `source-size-report.py`・`.toml`。b0 の版と byte 単位で同じ（`cmp` で確認）。
- 生の出力は run の artifacts の `raw/`（`c2-*.txt`・`cycle-edges-*.json`・`c2-diff-mb-head.txt`・`c4-c6-*.txt`・`c6-pub.txt`・`c7b-*.txt`・
  `health-*.json`・`ssr-*.json`）。script は `raw/c2.py`（D の adr の `raw/c2.py` に org_write の確認の 2 行を足した）・`raw/c7b.py`（A の §7 C7b の検査器の dir を引数にした）・
  `raw/c4-c6.sh`（A の §7 C4〜C6 と `Applied` の 2 本の command）・`raw/c6-pub.py`（A の §7 C6 の snippet）。

## 2. (1) import・依存・循環の内部の辺（C1・C2）

| 指標 | A（旧い検査器 × b0） | HEAD の検査器 × b0 | before（mb） | after（HEAD） |
|---|---|---|---|---|
| `deps.cycle` の件数（C1） | 17 | 17（C2） | 17 | 17 |
| 循環の単位・組・行の合計（C2） | 192・742・1,731 | 198・756・1,827 | 198・756・1,827 | 198・756・1,815 |
| task-api の SCC の単位・組・行 | 49・261・628 | 49・261・632 | 49・261・632 | 49・261・620 |
| C1 の task-api の rationale | 49 単位の循環（辺 261 組） | — | 同じ | 同じ |
| handlers → cos の行の辺・file | 22 本・13 file | 22・13 | 22・13 | 19・12 |
| cos → handlers の行の辺・file | 48 本・9 file | 52・9 | 52・9 | 50・9 |
| `org_write` が SCC に入るか | （無い） | （無い） | （無い） | 入らない |

- A と HEAD の検査器の違いは数え方だけ: B の検査器は `super::` の path を解決するので `cos/ops/admin.rs` の `super::super::super::handlers` の 4 行が辺になる
  （D の adr.md と同じ）。HEAD の検査器は b0 と mb で同じ値を出した。
- 行の辺の差（`raw/c2-diff-mb-head.txt`）: task-api の SCC で 14 本が消え、2 本が増えた（−12）。
  - 消えた org の辺 10 本（D の adr.md が挙げた 10 行と同じ）: `handlers/org.rs` → cos 3（`Applied`・`OperationAudit` の参照）、
    `cos/ops/admin.rs` → handlers 2（`use crate::handlers::org;`・`load_org_node`）、`skills.rs` → handlers 4（`OrgWrite` 2・`org_commit` 2）、
    `schema.rs` → handlers 1（`BrowserSettingsPatch`）。
  - 書き方の変化だけの差（成果に数えない）: `cos/ops/admin.rs` → skills は完全修飾の 3 行が `use` の 1 行になった（−2）。
    `skills.rs` の `use crate::handlers::{…}` は `load_org_node` が抜けて文面が変わった（−1 ＋1）。
  - 組（単位の対）は 1 つも消えていない。org 以外の領域の辺が同じ組に残るため。
- `org_write` の単位の辺（HEAD の `deps.cycle` の辺の集合）: 入る辺は cos・handlers・schema・skills から、出る辺は problem・types へ。
  `problem`・`types` は SCC の外なので、`org_write` は循環に入らない。
- C1 のほかの rule の件数（mb → HEAD）: `size.responsibility-growth` 94 → 94、`dup.contract` 210 → 210、`arch-map.wrong_owner` 31 → 31（b0 も同じ）。

## 3. (2) SQLite を直接参照する production file（C4）

| 指標 | A（b0） | before（mb） | after（HEAD） |
|---|---|---|---|
| `rusqlite` を書く file（task-core/src/store/ を除く） | 74（task-core 42、task-api 16、task-ops 8、celeris 4、ほか 4） | 74（同じ内訳） | 75（task-api 17） |
| SQL 文字列を持つ file（同じ除外） | 50（task-core 38、task-api 4、task-ops 3、celeris 3、llm-proxy 1、celeris-mcp 1） | 50 | 50 |

- 増えた 1 file は `crates/task-api/src/org_write.rs`（8 行目 `use rusqlite::Transaction;`）。SQL 文字列は持たない。
  `apply(tx, write, actor)` は呼び出し側の tx を受けて `org_upsert_tx`・`org_delete_tx` を呼ぶ。CoS の `apply_checked` が業務の更新と操作の記録を
  同じ tx に入れるためにこの引数が要る（ADR 2026-11-18-org-write-shared-use-case）。before の `handlers/org.rs` の `org_commit` は
  `audit.apply_checked(store, …, |tx| …)` の closure の引数を型注釈なしで受けていた（mb の 126 行目）ので、grep に出なかった。

## 4. (3) HTTP と CoS の相互 import、SQL の公開面（C5・C6）

| 指標 | A（b0） | before（mb） | after（HEAD） |
|---|---|---|---|
| C5 `crate::cos::` を書く handlers の行 | 22 行（13 file） | 22 行（13 file） | **19 行（12 file）** |
| C5 `(crate\|super(::super)*)::handlers` を書く cos の行 | 52 行（`super::` 4、9 file） | 52 行（`super::` 4、9 file） | **50 行（`super::` 4、9 file）** |
| C6 `&Connection`/`&Transaction` の引数・欄（個数） | task-core 320、task-api 14、task-ops 7、llm-proxy 4、celeris 1 | 同じ | task-api 15（ほかは同じ） |
| 同上の file（`git grep -l`） | task-core 72、task-api 9、task-ops 2、llm-proxy 1、celeris 1 | 同じ | task-api 10 |
| C6 `Fn(&Connection)` の closure の引数 | task-core 13、task-ops 5、task-api 4 | 同じ | 同じ |
| C6 `impl SqliteStore` の block（path 付きを含む） | 102（105） | 102（105） | 102（105） |
| C6 SqliteStore の `pub`・`pub(crate)` 等・private（path 付きを含む `pub`） | 633・67・42（636） | 633・67・42（636） | **631**・67・**41**（634） |

- C5 の消えた行は usecase.md と同じ: handlers → cos は `handlers/org.rs` の 3 行、cos → handlers は `cos/ops/admin.rs` の 2 行。
  C5 の除外の式（A の §7 C4 の `$X`）は handlers/・cos/ では値を変えない。
- C6 の `&Transaction` の 1 個は `org_write.rs` の `tx: &Transaction<'_>`（§3 と同じ理由）。
- C6 の file の数は A の §7 に command が無いので `git grep -l` で数えた。A の表の「task-core 76 file」と b0 の 72 が違う。原因は未確認。個数（320）は A と一致した。
- SqliteStore の 3 method の削除（store-prune）: `pub` の `org_upsert_browser_settings`・`org_delete` と private の `org_upsert_with_actor`。

## 5. (4) S1（新しい CoS 業務操作）の変更波及（C7b）

S1 の file の組は before が A の §5.1 の 3 file、after が objective の 4 file（同じ意味の変更を after の HEAD で行うときに変える file）。

| 指標 | A（旧い検査器 × b0、3 file） | before（HEAD の検査器 × mb、3 file） | after（HEAD、4 file） | 参考: after の HEAD に 3 file の組を当てた値 |
|---|---|---|---|---|
| 変える production file | 3 | 3 | 4（`org_write.rs`・`cos/ops/admin.rs`・`handlers/org.rs`・`store/org.rs`） | 3 |
| 変える単位 | 3（handlers・cos・store） | 3 | 4（＋ `task-api/src/org_write`） | 3 |
| 変える crate | 2 | 2 | 2 | 2 |
| 単位の SCC の大きさ | 49・49・55 | 49・49・55 | 49・49・1・55（org_write は SCC の外） | 49・49・55 |
| 直接 import する単位（scenario の単位を除く） | 92 | 93 | 93 | 94 |
| 推移的に import する単位 / package | 369 / 12 | 365 / 12 | 365 / 12 | 366 / 12 |
| S1 の file が handlers ⇄ cos に持つ行の辺（C2） | 5（org.rs 3、admin.rs 2） | 5 | 0 | 0 |

- 数え方の違い: A の値（92・369）は旧い検査器、before と after（93・365）は B の検査器の `lang_deps`。HEAD の検査器を b0 に当てても 93・365・12 で、mb と同じだった。
  したがって before と after は HEAD の検査器の列で比べる。
- 結果: 変える file と単位は 1 つずつ増え、直接・推移の import の数は変わらない。S1 の変更波及は減らなかった。
  減ったのは、S1 で変える file が handlers ⇄ cos の循環に持つ辺（5 → 0）と、S1 の新しい単位が SCC の外に出たこと。
- 参考の列（3 file の組を after に当てた値）で直接が 94 になるのは、`org_write` が store を import する単位として数えに入るため。
- test-affected の `--list`・build・health の時間は WorkUnit `timing` が測る（この WorkUnit では測っていない）。

## 6. (5) `Applied` の形を残す file（D の ADR の付記の撤去の条件の追跡）

| command | A（b0） | before（mb） | after（HEAD） |
|---|---|---|---|
| `git grep -l 'Option<&OperationAudit>' crates/task-api/src`（狭い） | 14 | 14 | 14 |
| `git grep -lE "Option<&('[a-z_]+ )?(crate::cos::operations::)?OperationAudit>" crates/task-api/src`（広い） | 19 | 19 | 18 |
| `git grep -lE '\bApplied\b' crates/task-api/src` | 19 | 19 | 18 |

- 狭い command の 14 file（HEAD）: `approvals.rs`・`changes.rs`・`cos/operations.rs`（型の定義側）・`decisions.rs`・`docs.rs`・`handlers/projects.rs`・
  `handlers/task_actions.rs`・`handlers/tasks.rs`・`integrate_task.rs`・`knowledge.rs`・`lifecycle.rs`・`model_catalog.rs`・`repos.rs`・`skills.rs`。
- 広い command で消えた file は `handlers/org.rs`（`org_commit` の削除）。残る 18 file は狭い 14 と `cron_jobs.rs`・`execution.rs`・`handlers/clusters.rs`・`inbox_notifications.rs`。
- 狭い command が 14 のままなのは、`handlers/org.rs` が完全修飾の `Option<&crate::cos::operations::OperationAudit>` を書いていて、もともと数えに入らないため
  （ADR の付記の予想どおり）。`skills.rs` は `put_skill_op`・`delete_skill_op` が残る。

## 7. (6) LoC（C12、分類規則は mb の版に固定）

| 指標 | A（b0） | before（mb） | after（HEAD） | after − before |
|---|---|---|---|---|
| production 全言語（`views.product_only.lines`） | 375,099 | 375,763 | 375,763 | **±0** |
| production Rust（`views.product_only.rust_lines`） | 330,408 | 331,072 | 331,072 | **±0** |
| 試験（`external_test` ＋ `inline_test`） | 476,771（455,363 ＋ 21,408） | 479,132（457,713 ＋ 21,419） | 480,112（458,693 ＋ 21,419） | +980 |
| own_total_with_tests 全言語 / Rust | 1,025,677 / 879,344 | 1,028,966 / 881,891 | 1,029,946 / 882,871 | +980 / +980 |

- b0 → mb の差（production +664）は D より前に main に入った他の task の分で、D の成果ではない。
- production の file ごとの差（`git diff --numstat <mb> HEAD`、+389 −389）:
  `org_write.rs` +325、`handlers/org.rs` +20 −316、`cos/ops/admin.rs` +21 −22、`handlers.rs` +1 −9、`skills.rs` +8 −7、`schema.rs` +1 −1、`lib.rs` +2、
  `store/org.rs` +8 −33、`store/mod.rs` +3 −1。
- 試験の差: `cos_delivery_suite/org_write_contract.rs` +972、`cos_delivery_suite/main.rs` +1、`store/tests.rs` +12 −5。
- 判定: production は純減していない。`handlers/org.rs` から消えた 316 行の多くは `org_write.rs` へ移った行で、移動は成果に数えない。
  移動を除いて消えたもの（`org_commit` の direct・audited の 2 分岐、`admin.rs` の削除の inline plan、SqliteStore の 3 method）は、
  `OrgWritten`・`apply`・`plan_delete` の追加と相殺した。

## 8. 未達・要判断

- production の LoC: 純減していない（±0）。この task の受け入れ条件は純減を求めていないが、依存・影響範囲の削減を LoC では示せない。
- S1 の変更波及（C7b）: 減らなかった。file と単位は 3 → 4 に増え、直接 93・推移 365・package 12 は変わらない。
- handlers ⇄ cos の単位の循環: 解けていない。組の数（261）は変わらず、cos → handlers の 50 行と handlers → cos の 19 行が残る。
- C4 の `rusqlite` の file と C6 の `&Transaction` の引数: 1 ずつ増えた（`org_write::apply` の tx の引数。同じ tx の保証のために要る）。
- (5) 時間と費用: この WorkUnit では測っていない（WorkUnit `timing` が測る）。

## 9. 次の候補

- `cos/ops/admin.rs` に残る cos → handlers の 13 行: accounts 9 行（`pool_adapter`・`create_account_op`・`clear_stale_limit`・`is_implicit_source`・
  `set_account_limit_effect`・`resolve_pool_account`・`delete_account_effect`・`check_account_effect`。うち `super::` 3 行）、clusters 2 行（`put_local_settings_op`・
  `put_cluster_settings_op`。`Applied` の形）、reload 1 行（`reload_effect`）、system 1 行（`replay_effect`）。
- `Applied` の型を返す `repos.rs`・`skills.rs`・`model_catalog.rs` の操作を org_write と同じ形（plan と tx 内の apply を共有 module に置き、HTTP は `write_tx`、
  CoS は `reject`・`apply_checked`）へ移す。
- `cos/operations.rs` → handlers の HTTP の道具 3 行（30 行目 `use crate::handlers::{ApiResult, Params, json_response, no_query};`、632 行目 `read_json`、
  845 行目 `cluster_ids`）。HTTP の道具を handlers の外の SCC に入らない module に置けば、cos → handlers の辺が減る。

## 10. 再現コマンド（repo の root で）

```sh
R=<run の artifacts>/raw; B0=bc665168c9caf1898adc4c611404bbee056daf9a; MB=$(git merge-base HEAD main); H=$(git rev-parse HEAD)
mkdir -p $TMPDIR/ck-head $TMPDIR/ck-old
git archive HEAD scripts/dev | tar -x -C $TMPDIR/ck-head; git archive $B0 scripts/dev | tar -x -C $TMPDIR/ck-old
NEW=$TMPDIR/ck-head/scripts/dev/repo-health; OLD=$TMPDIR/ck-old/scripts/dev/repo-health
# C1
python3 scripts/dev/repo-health/repo_health.py --repo . --base $H --head $H --policy-version 1 --out $R/health-head.json   # mb も同じ
# C2（old × b0、new × b0・mb・HEAD）
python3 -I $R/c2.py $NEW $H $R/cycle-edges-new-head.json
# C4・C5・C6・Applied の形
bash $R/c4-c6.sh $H; python3 -I $R/c6-pub.py $H; python3 -I $R/c6-pub.py $H path
# C7b（S1 の after は 4 単位）
python3 -I $R/c7b.py $NEW $H S1after:crates/task-api/src/handlers,crates/task-api/src/cos,crates/task-api/src/org_write,crates/task-core/src/store
python3 -I $R/c7b.py $NEW $MB S1:crates/task-api/src/handlers,crates/task-api/src/cos,crates/task-core/src/store
# C12（分類規則は mb の版）
git show $MB:scripts/dev/source-size-report.py > $R/ssr.py; git show $MB:scripts/dev/source-size-report.toml > $R/ssr.toml
python3 -I $R/ssr.py --root . --config $R/ssr.toml --exclusive-classes --rev $H --format json --out $R/ssr-head.json
```

## 11. 検査（提出時の HEAD で実行）

- `bash scripts/dev/test-affected.sh`（基準 `$CELERIS_WU_BASE`）→ exit 0。「試験対象の変更なし（docs のみ等）」、
  repo-health `verdict=pass`（new=0・worsened=0・existing=697）、repo_health_gate の自己試験 pass。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- `sh scripts/dev/progress-index.sh --check` → exit 0（ok）。`bash scripts/dev/check-doc-links.sh` → exit 0（ok）。
- source の差分: `git diff --name-only "$CELERIS_WU_BASE" -- crates web scripts docs` → 出力なし。

## 未解決

- (5) 時間と費用は WorkUnit `timing` が測る。
- §8 の未達 4 項目は close が受け入れ条件ごとにまとめ、人に判断を聞く。

## 提案

- 次の pilot は §9 の accounts（cos → handlers 9 行）と `cos/operations.rs` の HTTP の道具 3 行。org の pilot では組の数が減らなかったので、
  handlers ⇄ cos の組を消すには、残る辺のある領域をまとめて移す必要がある。
