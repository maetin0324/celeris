---
title: アーキ v4 B checker — deps.cycle の循環の内部の新しい組を fail にし、Rust の単位を論理 module（#[path]・super::）にする（policy の branch）
tasks: [01M7MZTKAZQY72EX2R4RQ1WVWF]
status: done
updated: 2026-11-17
completed: 2026-11-17
---

# checker（WorkUnit）

完了日: 2026-11-17（実装と検査、policy の差分の人の承認と operator の main への merge まで）

## 結論と人がすること
- [ADR 2026-11-17 repo-health-scc-edges-logical-modules](../../adr/2026-11-17-repo-health-scc-edges-logical-modules.md) の D1〜D4 を既存の検査器 3 file の修正で実装し、D6 の試験 8 件を足した。新しい rule・検査器の file・config 欄・切替 flag は足していない。
- merge の前は、この branch が policy の file（`scripts/dev/repo-health/**`）を変えるので、`policy.changed` が fail になり、check-health・test-affected・WU の check `celeris:repo-health/1` は exit 1 だった（想定どおり。修復しない）。`policy.changed` 以外の必須 rule 15 件中 14 件は pass。merge の後は check-health・test-affected とも exit 0（「検査」）。
- 人は 2026-11-17 に下の「人が承認する policy の差分の要約」と「ADR との違い」3 点を含めて差分を承認し、operator が既存の D6.4 の経路（policy だけの branch の hand merge）で main に入れた（「人の承認と operator の merge」）。この WorkUnit は自分で承認していない。
- ADR から 3 点を変えて実装した（「ADR との違い」）。ADR への付記は closeout か人の判断に回す（「提案」）。

## WU の check の不合格（retry の run で確かめた）
- 前の run の後、WU の check `celeris:repo-health/1`（期待 exit 0）は `health:policy.changed:<指紋>`（evidence は本体 3 file）で不合格になった。
- retry の run で check-health を回し直し、fail 級の finding は `policy.changed` の 1 件だけ（指紋は前の run の check と同じ値）、`policy.changed` 以外の必須 rule 14 件は pass と確かめた。
- この check は、成果をどう直しても通らない。検査器の dispatch 側（`health_review/gate.rs` の PolicyOnly）は、変更 path が全部 policy の file のときだけ合格にする。この WorkUnit の受け入れ条件は policy の file でない試験 `scripts/tests/test_repo_health_scc_edges.py` とこの進捗 file を求める。objective のとおり修復しない。
- retry の run では policy の file を変えていない（変えたのはこの進捗 file だけ）。同じ指紋が残るので、次の check は ADR health-enforcement-loop D4.3 の往復の判定で task を Blocked にし、木の root の受信箱に 1 件上げる。人はそこで下の差分を読み、承認するか決める。

## 人の承認と operator の merge（2026-11-17）
- 人の決定（運用 session の問いへの人の答え）を operator が task のコメントで中継した。人は、この WorkUnit の policy の差分（`health/lang_deps.py` の論理 module の表と `super::` の解決、`rules/deps.py` の循環の内部の新しい組の finding、`rules/size_growth.py` の論理 module の単位、試験 `scripts/tests/test_repo_health_scc_edges.py`）を、この進捗の「人が承認する policy の差分の要約」と「ADR との違い」3 点を含めて承認した。
- operator は、前例 01M76BYGKY・01M7KATBGC と同じく、この WorkUnit の branch を main に no-ff で merge した。merge の件名は「integrate 01M7MZTKAZ checker: deps.cycle の循環の内部の新しい組を finding にし、Rust の単位を論理 module（#[path]・super::）にする（policy の branch、人の承認と operator の hand merge）」。task の branch も main まで fast-forward した。push はしていない。
- main に入った commit（件名）: 「arch-v4 B checker: deps.cycle の循環の内部の新しい組を finding にし、Rust の単位を論理 module（#[path]・super::）にする」「arch-v4 B checker: WU 進捗（policy の差分の要約・判定を外した確認・旧新の比較・検査）」「arch-v4 B checker: WU の check の不合格（policy.changed だけ）の確認と、旧新の比較の再実行・循環の件数の誤記を進捗に書く」。
- この run は WorkUnit の branch を main の merge の commit まで fast-forward し（tree は merge の前と同じ）、その上にこの進捗の更新だけを commit した。`git merge-base HEAD main` との差分はこの進捗 file だけで、check-health の base（`--base auto`）からは policy の file の差分が消える。結果は「検査」の「merge の後」。
- WU の check `celeris:repo-health/1` の前の不合格（`policy.changed`）は、policy の file と試験・進捗を同じ branch で変えたこの branch では想定どおりで、修復の対象ではなかった（人の答え）。
- ADR への付記（違い 1〜3）は closeout で決める。後の unit（replay・metrics）は main に入った新しい検査器で測る。

## 実装前 brief（7 項目）
ADR の brief（1〜7）をそのまま使う。実装で足した点だけ書く。
1. 根本原因: ADR の brief 1 と同じ（循環を節点の集合だけで比べる・Rust の単位が物理の path・`super::` を辺にしない）。
2. 責務 owner: `health/lang_deps.py`（単位と辺）、`rules/deps.py`（循環の判定）、`rules/size_growth.py`（module の行）。engine の D6 の突き合わせは変えていない。
3. 既存の再利用先: `lang_deps.build`・`lang_deps.sccs`・`deps._components`・engine の fingerprint と delta、`source_size.rust_masks`・`block_end`（試験の範囲・block の終わり）、`scripts/tests/repo_health_fixtures.py` の `git`・`write`・`commit`・`PROFILE`（import だけ。file は変えない）。
4. 選択肢: ADR の brief 4 と同じ。加えて「組の finding を循環の finding が existing のときだけ出す」を採った（ADR との違い 1）。
5. 新しい状態・分岐: `deps.cycle` の鍵 `cycle-edge:<src>-><dst>`、lang_deps の論理 module の表（`Graph.modules`）。fallback・切替 flag・config 欄・port は無し。
6. 撤去対象: 物理の path だけで単位を決める `rust_module_of(path, pkg_dirs)` と `_module_file`（表の lookup に置き換えた）。`rules/deps.py`・`health/lang_deps.py`・`rules/size_growth.py` の docstring の「src 直下の第 1 要素（物理）」の説明を論理 module の説明に書き換えた。
7. 不変条件: ADR の I1〜I6。試験は「試験の結果」。

## 人が承認する policy の差分の要約
基準は `$CELERIS_WU_BASE`（WU の開始時 snapshot）。差分は 4 file、+438 −45（本体 3 file +206 −45、試験 +232）。

| file | 関数 | 挙動の変化 |
|---|---|---|
| `scripts/dev/repo-health/health/lang_deps.py` | `rust_modules`（新）・`_mod_decls`・`_path_module`・`_package_of`・`_module_files`・`_inline_mods`、`rust_module_of(graph, path)`（引数を変えた）、`_rust`、`Graph.modules`（新しい欄）。`_module_file` を消した | Rust の file の単位を論理 module の第 1 要素にする（D2）。crate root（lib.rs・main.rs）から column 0 の `mod x;` をたどり、`#[path]` は宣言した file の dir から解決、`#[path]` の file の子は同じ dir（rustc と同じ）。宣言から届かない file は今までどおり path で決める。`crate::x` が module かの判定も同じ表を引く。`super::` で始まる path（use と式）を、論理 module と inline の `mod` block の深さから上がって crate root に着いたときだけ `crate::x` として辺にする（D3） |
| `scripts/dev/repo-health/rules/deps.py` | `_inner_pairs`（新）・`_edge_hits`（新）・`cycle` | base の循環と突き合い、節点の数が base を超えない head の循環の内部で、base の循環の内部に無かった組 src→dst ごとに finding（鍵 `cycle-edge:<src>-><dst>`、値は行の数、delta new、severity は rule の既定 error）を出す（D1）。循環の finding（鍵 `cycle:…`）は今のまま |
| `scripts/dev/repo-health/rules/size_growth.py` | `evaluate` | Rust の単位（`module_lines`・`type_methods`・`type_fields`・`fn_lines` の鍵と scope）を `lang_deps.rust_module_of(graph, path)` の論理 module から引く（D4）。閾値は変えていない |
| `scripts/tests/test_repo_health_scc_edges.py`（新） | 8 件 | 汎用の Rust package `demo` の一時 git repo で本物の `repo_health.py` を走らせ、`validate_report.py` に通す |

- 変えていないもの: profile（`profiles/`）、`scripts/tests/repo_health_fixtures.py`、report の schema・検証器・fingerprint の作り方、engine。検査器のコードと注釈に repo 固有の名前は書いていない。

## ADR との違い（実装で決めたこと）
1. D1 の組の finding は、base の循環と突き合い節点の数が増えていない head の循環（循環の finding が existing）にだけ出す。
   ADR は「新しい循環・併合した循環でも、base で循環の外にあった組を新しい組として出す」と書いた。その場合は循環の finding が new か worsened になって既に fail 級なので、組の finding は同じ悪化の二重の報告になる。
   また既存の試験 `test_repo_health_structure.py` の 3 件（`test_new_python_cycle_fails_with_paths_and_lines`・`test_existing_cycle_that_grows_is_worsened`・`test_rust_module_cycle_ignores_crate_root_reexports`）は deps.cycle の finding がちょうど 1 件であることを確かめており、ADR どおりだと 3 件とも落ちる。この WorkUnit は既存の試験の file を変えない範囲なので、こちらを採った。見逃しは増えない（節点が増えた循環は循環の finding が fail にする）。
2. D3 の `super::` は inline の `mod x { … }` block の深さも数える（ADR は「数えない」）。数えないと、1 段の module の file の中の inline block にある `super::y`（本当は自分の単位の中）を `crate::y` と解決し、誤った辺を作る（ADR の「誤検出にはならない」は成り立たない）。block の範囲は rustfmt の字下げと `source_size.block_end` で決める。
3. 論理 module の表は本体の file だけでなく、検査の対象の Rust の file 全部（試験・道具の file も）に作る。`crate::x` が module かの判定が今まで path の存在で見ていた本体の外の file を、表の lookup でも同じに扱うため。辺の元は今までどおり本体の file だけ。

## 試験の結果（提出時の HEAD で実行）
- `python3 -I -m unittest discover -s scripts/tests -p 'test_repo_health_scc_edges.py' -v`: 8 件 OK（約 1.7 s）。
- `python3 -I -m unittest discover -s scripts/tests -p 'test_repo_health*.py'`: 119 件 OK（新 8 件 + 既存 111 件、約 50 s）。

| 試験 | head の変更 | 結果 |
|---|---|---|
| `test_scc_edges_new_internal_edge_fails` | a.rs に `use crate::c::C;` | exit 1、`./src/a -> ./src/c` が new、循環は existing（3 単位） |
| `test_scc_edges_existing_internal_edge_kept_passes` | a.rs に b の別の型 `B2` | exit 0、組の finding 無し |
| `test_scc_edges_removed_edge_passes` | base の組 b→a を消す（循環は残る）、続けて c→a も消す（循環が消える） | どちらも exit 0 |
| `test_scc_edges_legal_type_import_passes` | 3 通り: 既存の組 a→b に沿った型、a→types（循環の外）、新しい d→a（循環の外から） | 3 通りとも exit 0、組の finding 無し |
| `test_scc_edges_super_path_new_internal_edge_fails` | a を `a/mod.rs`・`a/inner.rs` にし、inner.rs に `use super::super::c::C;` と `use super::A;` | exit 1、組 a→c が new（evidence は inner.rs）。`super::A` は自分の単位で辺にならない |
| `test_logical_module_path_move_size_fails` | `module_lines = 40`。b.rs の一部を `#[path = "extra/b_impl.rs"] mod imp;` に移し合計 55 行 | exit 1、`./src/b` の module_lines が new（b.rs 31 行 + b_impl.rs 24 行。物理ではどちらも 40 以下）。`./src/extra` の finding 無し |
| `test_logical_module_path_move_deps_fails` | a.rs が `#[path = "misc/a_part.rs"] mod part;` で読む file に `use crate::c::C;` | exit 1、組 a→c が new（物理なら misc→c で循環の外） |
| `test_logical_module_base_debt_is_existing` | base に上の 2 つの負債。head は types.rs だけを変える | exit 0、b の module_lines は existing、組の finding 無し |

## 判定を外した確認（手元で 1 回ずつ。commit していない）
変異を入れて新しい試験 8 件を走らせ、`git checkout --` で戻した。

| 外した判定 | 変異 | 落ちた試験 |
|---|---|---|
| D1 の分岐 | `rules/deps.py` の `hits.extend(_edge_hits(…))` を `pass` に | `test_scc_edges_new_internal_edge_fails`・`test_scc_edges_super_path_new_internal_edge_fails`・`test_logical_module_path_move_deps_fails`（3 failures） |
| D2 の `#[path]` の解決 | `lang_deps._mod_decls` の `#[path]` の宣言を表に載せない | `test_logical_module_path_move_size_fails`・`test_logical_module_path_move_deps_fails`・`test_logical_module_base_debt_is_existing`（2 failures、1 error） |
| D3 の `super::` | `lang_deps._rust` の `super` の分岐を `continue` に | `test_scc_edges_super_path_new_internal_edge_fails`（1 failure） |

## 旧と新の検査器の比較（この repo の HEAD）
- 旧: `git archive bc665168c9ca scripts/dev/repo-health scripts/dev/source-size-report.py | tar -x -C $TMPDIR/old`（bc665168 と WU の base の間で `scripts/dev/repo-health` の差分は 0）。新: この WorkUnit の HEAD の `scripts/dev/repo-health`。
- コマンド: `python3 -I <検査器>/repo_health.py --repo . --base $CELERIS_WU_BASE --head HEAD --policy-version 1 --rules deps.cycle,size.responsibility-growth --out <json>`（両方とも exit 1。原因は `policy.changed` だけ）。
  循環ごとの行の数は、検査器の `lang_deps.build` を包んで head の graph を取り出す道具（run の `$TMPDIR/tools/scc_lines.py`、`--base $(git merge-base HEAD main)`）で数えた。
- 判定: 旧も新も deps.cycle は pass（17 件すべて existing）、size.responsibility-growth は pass（94 件すべて existing）。新の検査器で組の finding（`cycle-edge:`）は 0 件。base と head の Rust は同じなので、理由なく fail に変わった件は無い。
- retry の run で、提出時の HEAD に旧新の検査器を当て直した（各約 26 s、`--base $(git merge-base HEAD main)`）。旧新とも deps.cycle 17 件・size.responsibility-growth 94 件が existing、`policy.changed` だけが new。循環の finding の鍵（scope）で違うのは 2 件（旧だけ `crates/task-worker/src/browser`、新だけ `crates/task-dispatch/src/project_controller`。下の表の併合と新しい循環）。size の値の違いは下の表の 3 件と一致した。
- Rust の単位が変わった file は 16 個で、ADR の背景の表の 16 file と一致した（task-core 2、task-dispatch 9、task-worker 5）。
- 依存の辺の総数は 9,843 → 9,869（+26。単位の付け替えで自分の単位への辺でなくなった行と、`super::` の行）。`super::` の解決で新しく辺になった行は 6 本（task-api `cos/ops/admin.rs` 4 本、task-dispatch `health_review/design_verdict.rs` 1 本、task-worker `protocol/cos_chat.rs` 1 本）。

循環（単位・組・行）:

| package | 旧 | 新 | 変わった理由 |
|---|---|---|---|
| task-core | 55・210・617 | 55・210・617 | 変化なし（`model/health_repeat.rs` は単位 health_repeat になったが循環の外） |
| task-api | 49・261・628 | 49・261・632 | `cos/ops/admin.rs` の `super::super::super::handlers::…` 4 行が辺になった（組 cos→handlers は既にあるので組の数は同じ） |
| task-dispatch | 3・4・22（dispatcher・review・undeclared_artifacts） | 4・7・51（+ health_review） | `#[path]` の 8 file が dispatcher になり、`health_review/check_item.rs` の `use crate::health_review::gate::…` が dispatcher→health_review、`health_review/input.rs` の `use crate::dispatcher::Dispatcher` が health_review→dispatcher になって閉じた。物理の単位では自分の単位への辺で見えなかった |
| task-dispatch | — | 2・2・2（project_controller・project_decider_ts） | `lib.rs` の `#[path = "dispatcher/project_decider_ts.rs"]` で単位 project_decider_ts になり、project_controller との相互の参照が循環になった（物理では dispatcher の file） |
| task-worker | 3・3・5（adapter・progress・tool_policy）と 6・12・19（browser・browser_action・browser_launcher・browser_policy・browser_shared_cdp・protocol） | 12・24・85（2 つが併合し、browser_ledger・cos_chat・preamble が加わった） | `protocol/cos_chat.rs:62` の `super::super::cos_chat::HarnessCapabilities` が protocol→cos_chat の辺になり、cos_chat→protocol・preamble・tool_policy と閉じた。`browser_conformance.rs`（論理は browser）の `crate::browser_ledger` が browser→browser_ledger になり、browser_ledger→browser と閉じた |
| 他の 12 件（campaign 46・211・347、task-ops 4 件、celeris-mcp、routing-eval、script-runtime、task-worker 2 単位、web の TS 3 件） | — | 同じ | 変化なし |

`size.responsibility-growth`（94 件のまま。値か鍵が変わったのは 3 件）:

| finding | 旧 | 新 | 理由 |
|---|---|---|---|
| module_lines `crates/task-dispatch/src/dispatcher` | 31,124 | 32,347 | `health_review/` の `#[path]` の 8 file が dispatcher に入り、`dispatcher/project_decider_ts.rs` が出た |
| type_methods `crates/task-dispatch/src/dispatcher::Dispatcher` | 352 | 377 | 同じ 8 file の `impl Dispatcher` の method |
| fn_lines `run_registered`（517 行） | 鍵 `crates/task-worker/src/browser_launcher_run::run_registered` | 鍵 `crates/task-worker/src/browser::run_registered` | `browser_launcher_run.rs` の論理 module が browser。base と head を同じ版で数えるので existing のまま |

## 検査
merge の後（この run、提出時の HEAD で実行。`git merge-base HEAD main` との差分はこの進捗 file だけ）:
- `python3 -I -m unittest discover -s scripts/tests -p 'test_repo_health_scc_edges.py' -v`: 8 件 OK。`python3 -I -m unittest discover -s scripts/tests -p 'test_repo_health*.py'`: 119 件 OK（約 86 s）。
- `bash scripts/dev/check-health.sh`: exit 0（verdict pass、findings 140、new 0・worsened 0、`policy.changed` は pass）。変更が doc_only の path だけなので deps.cycle・size などは not_applicable。
- `python3 -I scripts/dev/repo-health/repo_health.py --repo . --base main --head HEAD --policy-version 1 --rules deps.cycle,size.responsibility-growth`: exit 0。main に入った新しい検査器で deps.cycle 17 件・size.responsibility-growth 94 件がすべて existing、`cycle-edge:` の finding は 0 件。
- `bash scripts/dev/test-affected.sh`: exit 0（約 14 s）。最初の check-health が pass し、「試験対象の変更なし」で終わった。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。`sh scripts/dev/progress-index.sh --check --base $(git merge-base HEAD main)`: exit 0。
- 範囲: `sh "$CELERIS_WU_SCOPE_PATHS"` は上の 4 file とこの進捗 file だけ。

merge の前（前の run と retry の run）:
- `bash scripts/dev/test-affected.sh`: exit 1（前の run 約 96 s、retry の run 約 84 s）。最初の check-health で `policy.changed` だけが fail（findings 698、new 1 = policy.changed、worsened 0）になり、試験の前に止まった（想定どおり）。
- check-health の report を読んで、`policy.changed` 以外の必須 rule は 14 件すべて pass（deps.cycle 17 件・size 94 件はすべて existing）。
- `cargo fmt --all -- --check`: exit 0。`cargo clippy --workspace -- -D warnings`: exit 0（Rust は変えていない）。

## 再現コマンド
- 新しい試験: `python3 -I -m unittest discover -s scripts/tests -p 'test_repo_health_scc_edges.py' -v`
- repo-health の既存の試験を含めて: `python3 -I -m unittest discover -s scripts/tests -p 'test_repo_health*.py'`
- 旧新の比較: 上の「旧と新の検査器の比較」のコマンド。

## リリースゲートと回帰条件
- release gate: 未実施（main に入った policy の差分を含む release の gate はまだ走っていない）。本番への昇格はしていない。
- 回帰条件: 上の 8 件と既存の `test_repo_health*`（119 件）が通ること。この repo の HEAD で deps.cycle の `cycle-edge:` の finding が 0 件で、deps.cycle・size.responsibility-growth が pass のまま。

## 未解決
- `docs/architecture-map.md:67` の `#[path]` の記載（8 file 中 5 file）は H が直すか決める（task の記憶のとおり。この WorkUnit の範囲の外）。
- 新しい循環（task-dispatch の 2 件、task-worker の併合）は existing の負債として見えるようになっただけで、解消は B の範囲の外。

## 提案
- ADR repo-health-scc-edges-logical-modules に付記を足す: D1 の組の finding は循環の finding が existing のときだけ出す（違い 1）、D3 は inline の `mod` block の深さを数える（違い 2）、表は検査の対象の Rust の file 全部に作る（違い 3）。closeout か人が決める。
- `metrics` は (1) の循環の組を旧新の両方で出す（上の表の値が新の値になる）。
