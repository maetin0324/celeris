---
tasks: [01M7MZTKAZQY72EX2R4RQ1WVWF]
---
# ADR 2026-11-17: repo health の deps.cycle を既存の循環の中の新しい組に、Rust の単位を論理 module（#[path]・super::）に効かせる

- 日付: 2026-11-17
- 状態: Accepted（決定だけ。この ADR を書いた WorkUnit は code を変えていない。実装は同じ task の WorkUnit `checker` が policy の branch で行う）
- 付記先: [ADR 2026-11-07 repo health の契約](2026-11-07-repo-health-contract.md) D5（`deps.cycle`・`size.responsibility-growth` の意味）、
  [ADR 2026-11-07 health review integration](2026-11-07-health-review-integration.md) の付記（2026-11-08、dispatch の実装）の「行は `health_review` の module に数えられる」。
  両方の末尾に、この ADR を指す付記を足した。
- 関係: [ADR 2026-11-10 health generic fixtures](2026-11-10-health-generic-fixtures.md)（汎用の Git fixture）、
  [ADR 2026-11-16 許可の一覧の行の削除](2026-11-16-health-policy-allowlist-shrink.md)（policy の branch の先例）、
  [2026-10-11 破壊的変更を既定にする](2026-10-11-breaking-changes-by-default.md)。
- task: 01M7MZTKAZQY72EX2R4RQ1WVWF（アーキ v4 B。依頼キー celeris-architecture-scalability-v4-20261117、親 01M7MZT8KX547464YYS76QNP4G、依存 A 01M7MZTKAXM8GJGK8BPE13CQRM）。
  基準は main `bc665168c9caf1898adc4c611404bbee056daf9a`。before の値と計測の規則は A の baseline（run の artifacts の `baseline.md`）§6・§7。

## 背景
A の baseline（main bc665168）で次を確かめた。

- `deps.cycle` の値は循環の節点（単位）の数で、head の循環は節点を最も多く共有する base の循環と同じ fingerprint になる。
  既存の循環の中で、まだ辺の無い単位の組に辺を足しても節点の数は変わらないので、finding は `existing` のままで合格する（baseline §6 の 3）。
  Rust の循環は 14 件で、task-core は 55 単位・210 組（行の単位で 617 本）、task-api は 49 単位・261 組（628 本）（baseline §2 の #3〜#5）。新しい組を 1 本足しても検査は止めない。
- Rust の単位は物理の path（`<pkg>/src/<第 1 要素>`）で決まる（`health/lang_deps.py` の `rust_module_of`）。
  `#[path]` で別の dir に置いた file は、宣言した module でなく置き場所の module に数えられる。dispatcher の論理 module のうち 8 file・2,641 行が
  `health_review/` にあり、`size.responsibility-growth` の `module_lines` と `deps.*` の辺の元は `health_review` になる（baseline §3.2・§6 の 1）。
  file を `#[path]` 付きで別の dir に移すと、module の行の上限と循環の検査をすり抜けられる。
- `super::` で書いた参照は辺にならない（`lang_deps._rust` は `crate::` と package の lib 名だけを解決する）。
  task-api の `cos/ops/admin.rs` の 4 行（`super::super::super::handlers::accounts::…`）は handlers への辺として数えられていない（baseline §6 の 2）。

base の crates（試験の file と campaign・routing-eval・jev-classifier を除く）で、file の top level の `#[path = "…"]` は 27 行ある。
このうち宣言した file と読む file の単位（第 1 要素）が違い、D2 で単位が変わる見込みの file は 16 個（base の source を読んだ手の数え。WorkUnit `checker` が実装で確かめる）:

| package | file（物理） | 今の単位 | D2 の単位 |
|---|---|---|---|
| task-dispatch | `health_review/{design_gate,design_verdict,policy_review,inventory,check_item,plan_adoption,review_limits,worker_profile}.rs` | `health_review` | `dispatcher` |
| task-dispatch | `dispatcher/project_decider_ts.rs`（`lib.rs:47` の `pub mod project_decider_ts`） | `dispatcher` | `project_decider_ts` |
| task-core | `model/health_repeat.rs`（`lib.rs:192` の `pub mod health_repeat`） | `model` | `health_repeat` |
| task-core | `project_proposal_fixture.rs`（`project_proposal.rs:933`。adapters の種類が product の外なら数えない） | `project_proposal_fixture` | `project_proposal` |
| task-worker | `browser_{auth,conformance,launcher_run}.rs` | 各 file 名 | `browser` |
| task-worker | `browser_cdp_{credential_ipc,post_login}.rs` | 各 file 名 | `browser_cdp_sink` |

`cfg_attr(…, path = "…")` は base の crates に 0 件。

## 実装前 brief（7 項目）

### 1. 根本原因
- `deps.cycle` は循環を節点の集合だけで比べ、循環の内部の組（どの単位がどの単位に依存するか）を base と比べていない。症状は、既存の循環に新しい依存を足す変更が health の review を通ること（依頼の監査根拠「既存 SCC への新しい辺が node count を変えない場合の悪化を見逃す可能性」、A の baseline §2 の #3〜#5・§6 の 3）。
- `lang_deps` の Rust の単位が物理の path で、`mod` の宣言（`#[path]`）を読まない。`super::` の参照も辺にしない。症状は、file の移動・`#[path]` の付け替え・`super::` への書き換えで、`module_lines` と循環の検査の値が下がること。

### 2. 責務 owner
- 単位と辺: `scripts/dev/repo-health/health/lang_deps.py`（`rust_module_of`・`_module_file`・`_rust`）。
- 循環の判定: `scripts/dev/repo-health/rules/deps.py`（`cycle`・`_components`）。
- module の行: `scripts/dev/repo-health/rules/size_growth.py`（`evaluate` の Rust の単位）。
- base との突き合わせ（delta・fail 級）は `health/engine.py` の契約 D6 で、変えない。

### 3. 既存の再利用先
- `lang_deps.build`・`lang_deps.sccs`・`deps._components`（循環と内部の辺）、engine の fingerprint（`rule_id` と `key` の hash）と D6 の delta。
- `source_size.rust_masks`（`#[cfg(test)]` の範囲。試験の mod は今と同じく辺の元にしない）。
- profile の `[[rules."deps.cycle".exceptions]]`（期限付き例外。新しい組を人が認めるときに使う。新しい欄は足さない）。
- 試験の道具: `scripts/tests/repo_health_fixtures.py` の `git`・`write`・`commit`（import して使う。file は変えない）、`scripts/dev/repo-health/profiles/default.toml`。

### 4. 選択肢
- 採用: `deps.cycle` の中で、head の循環の内部の組ごとに finding を出す（D1）。どの組が新しいかを evidence に出せ、組の追加と削除が相殺しない。
- 採らなかった: 循環の finding の値を「組の数」にする。組を 1 つ消して別の組を 1 つ足すと値が変わらず見逃す。どの組が新しいかも出せない。
- 採らなかった: 新しい rule ID（例 `deps.cycle-edge`）を足す。schema の pattern・検証器・契約 D5 の一覧・profile の `required` を同時に変えることになり、D5（schema を変えない）に反する。
- 採用: `mod` の宣言を静的に読んで file を論理 module に割り当てる（D2）。base の tree にも同じ方法を当てられ、外部 tool が要らない（lang_deps の既存の方針）。
- 採らなかった: rustc の dep-info・`cargo metadata` で module を引く。base の tree を build する必要があり、health が数分遅くなる。A は dep-info を真偽の確認にだけ使った。
- 採らなかった: `#[path]` で dir をまたぐことを profile の規則で禁止する。Celeris 向けの新しい検査器になり、試験の file を分ける正当な使い方（多数ある）も止める。

### 5. 新しい状態・分岐・fallback・port
- `deps.cycle` の finding に鍵の形 `cycle-edge:<src_unit>-><dst_unit>` を 1 つ足す（rule ID・report の欄は同じ）。
- file から論理の単位を引く表を lang_deps に 1 つ置く。物理の単位を引く今の関数をこれに置き換えるので、正本は 1 つのまま（規約 4）。
- fallback・新旧切替の flag・config の欄・port は無し。

### 6. 撤去対象
- `rust_module_of` の物理の path だけで単位を決める版（論理の表に置き換え、呼び手の `size_growth.evaluate` と `lang_deps._rust` も同じ表を使う。物理の版を残さない）。
- `rules/deps.py` の docstring の「値は節点の数で、base より増えれば worsened。…existing」だけを書いた説明と、`health/lang_deps.py` の docstring の単位の説明（物理の path）。D1〜D3 の説明に書き換える。
- ADR health-review-integration の付記（2026-11-08）の「行は `health_review` の module に数えられる」と契約 D5 の `deps.cycle` の行の読み方: この ADR を指す付記を足した（ADR の本文は書き換えない）。

### 7. 不変条件と確かめる試験
- I1 base で既知の循環の内部の組と負債は fail にしない（`existing`）: `test_scc_edges_existing_internal_edge_kept_passes`・`test_logical_module_base_debt_is_existing`。
- I2 既存の循環の内部の新しい組は、節点の数が変わらなくても fail: `test_scc_edges_new_internal_edge_fails`・`test_scc_edges_super_path_new_internal_edge_fails`。
- I3 組の削除と循環の外への import は pass: `test_scc_edges_removed_edge_passes`・`test_scc_edges_legal_type_import_passes`。
- I4 `#[path]` での移動・付け替えで `module_lines` と循環の内部の組が減らない: `test_logical_module_path_move_size_fails`・`test_logical_module_path_move_deps_fails`。
- I5 report の schema と検証器は変えない: 既存の `scripts/tests/test_repo_health_contract.py` と `validate_report.py`。
- I6 repo 名・Celeris の名前で分岐しない: 新しい試験は汎用の package と `default.toml` だけを使う。既存の `test_repo_health_generic*.py` が通る。

## 決定

### D1. deps.cycle: 既存の循環の中の新しい組を finding にする
- head 側で、head の循環（大きさ 2 以上の強連結成分）の内部の組（`src_unit -> dst_unit`、両端が同じ head の循環に属し、head にその向きの辺がある）のうち、
  base の循環の内部の組の集合 P_base に無いものごとに finding を 1 件出す。
  P_base = {(s, d) | s と d が同じ base の循環に属し、base に s→d の辺がある}。辺は今の `_cycle_edges` の辺（`include_dev`・`include_type_only` と package をまたぐ use の除外は今と同じ）。
- 鍵は `cycle-edge:<src_unit>-><dst_unit>`、rule は `deps.cycle`。base 側（`tree.side == "base"`）では出さないので、engine の D6 で delta は `new` になり、
  severity が error（`deps.cycle` の既定）なら fail 級になる。base の tree が無いときは出さない（突き合わせる相手が無い）。
- finding の欄: scope は src_unit（crate の辺だけなら crate、それ以外は module。循環の finding と同じ決め方）、value はその組の行の本数、
  evidence はその組の行（`max_evidence` まで）、rationale は「head の N 単位の循環の内部に新しい組 src→dst（行 M 本）。base ではこの組は循環の内部に無かった」。
- finding にしないもの: 既存の組に行を足す・既存の組を保つ・組を消す・循環の外の単位への import（片端が循環の外）・循環の外の新しい辺。
- 新しい循環ができたとき・base の循環が 2 つ併合したときは、循環を閉じた辺だけでなく、base では循環の外にあった組も新しい組として出る。
  循環の内部にある組そのものを負債の単位とみなすので、これは意図どおり。
- 循環の finding（鍵 `cycle:…`、値は節点の数、節点の共有で base と突き合わせる）は今のまま残す。

### D2. lang_deps の Rust の単位を論理 module にする
- Rust の file ごとに論理 module の path（crate からの module 名の列）を決め、単位はその第 1 要素（`<pkg>/src/<第 1 要素>`）にする。crate root（`src/lib.rs`・`src/main.rs`・`src/bin/**`）は今と同じく単位にしない。
- `#[path]` の無い `mod x;` の file は今の規則と同じ（`x.rs`・`x/mod.rs`、非 mod.rs の file の子は `<file の stem>/x.rs`）。`#[path]` の無い repo では単位は今と変わらない。
- file の top level（inline の `mod` block の外）の `#[path = "P"] mod x;` で読む file は、P を宣言した file の dir からの相対で解決し、宣言した file の論理 module に属す（論理 path は「宣言した file の論理 path + x」）。
- `#[path]` で読んだ file の中の `#[path]` の無い `mod y;` は、rustc と同じく、その file と同じ dir の `y.rs`・`y/mod.rs`（`#[path]` の file は mod.rs と同じ扱い）。その子も同じ論理 module に属す。`#[path]` の連鎖（例 `health_review/design_gate.rs` → `design_verdict.rs`）もたどる。
- crate root の `#[path = "P"] mod x;` は x を単位にする（`<pkg>/src/x`）。`crate::x::…`・`<lib>::x::…` の解決（今の `_module_file`）も同じ表を使い、`#[path]` で読む module を root の再 export と誤らない。
- 同じ file を 2 つの宣言が読むときは、宣言した file の path の辞書順で先のものを採る（決定的にする）。どの宣言からも届かない file は既定の規則で単位を決める（rustc は build しないが、今の数え方と同じにする）。
- 拾わないもの（限界）: `#[cfg_attr(…, path = "…")]`（base の crates に 0 件）、inline の `mod` block の中の `#[path]`、macro が作る `mod`。`#[cfg(test)]` の範囲の `mod` は今と同じく辺の元にしない。
- 表は lang_deps の 1 か所で作り、`deps.violation`・`deps.cycle`・`size.responsibility-growth`・`lang_deps.build` を使う他の rule が同じ表を引く。

### D3. super:: の path を辺にする
- `super::` で始まる path（`use` の中と、式の中の参照）を辺の候補にする。式の中の参照は `super(::super)*::<名前>` の列を 1 つの path として読む（今の正規表現は 2 要素しか取らない）。
- 解決は、その file の論理 module の path（D2）から `super` の数だけ上がり、残りの要素をつなぐ。上がった先が crate root なら次の要素 x を `crate::x` と同じに解決する（module でなければ root の再 export とみなし辺を作らない）。上がりすぎ（論理 path より `super` が多い）は辺を作らない。自分の単位への辺は今と同じく作らない。
- 限界: inline の `mod x { … }` block の中の `super::` は file の論理 module から数える（block の深さを数えない）。単位は論理 path の第 1 要素なので、結果が違うのは crate root まで上がる path だけで、そのときは辺を作らない（見逃しになり、誤検出にはならない）。`self::` は自分の単位なので辺にしない。

### D4. size.responsibility-growth の単位を D2 にそろえる
- Rust の `module_lines` は D2 の単位で合計する。file の移動・`#[path]` の付け替え・分割では module の行は減らない。
- `type_methods`・`type_fields`・`fn_lines` の鍵と scope に使う Rust の単位も D2 の表から引く（物理と論理の 2 つの単位を持たない）。`file_lines` は file ごとのまま。
- 閾値（`default.toml`・`celeris.toml` の `module_lines` など）は変えない。単位の鍵が変わる finding（背景の表の 16 file を含む module）は、base と head を同じ版の検査器で数えるので同じ鍵で突き合う。`celeris.toml` の exceptions は `link.unlinked` と `migration.remnant` だけで、単位の鍵を書いた例外は無い。

### D5. 足さないもの・変えないもの
- 新しい config の欄・新旧切替の flag・Celeris 固有の設定・新しい検査器（rule ID・script）を置かない。物理の単位・`super::` を無視する数え方を残す分岐を置かない（規約 5・6）。
- report の schema（`schema/report-v1.schema.json`）・検証器・finding の欄・fingerprint の作り方を変えない。
- 変える file は `scripts/dev/repo-health/health/lang_deps.py`・`rules/deps.py`・`rules/size_growth.py` と新しい試験 1 file（D6）。`scripts/tests/repo_health_fixtures.py` は Rust の試験も使うので変えない。
- `scripts/dev/repo-health/**` は policy の file（`policy.changed`）。WorkUnit `checker` は policy の file・その試験・ADR・進捗だけを変える policy の branch で出し、人の差分の承認と operator の main への merge を待つ（ADR health-review-integration D6.4・D6.6、先例 task 01M7KATBGCRQRZ5NWXHXVWYPZ8）。自分の変更を自分で承認しない。

### D6. 試験と fixture
- file: `scripts/tests/test_repo_health_scc_edges.py`（unittest）。実行: `python3 -I -m unittest discover -s scripts/tests -p 'test_repo_health_scc_edges.py' -v`。
- fixture: 試験ごとに `tempfile` の一時 dir に git repo を作り、base と head の 2 commit を置く。汎用の Rust package（`Cargo.toml` の `[package] name = "demo"`、`src/lib.rs` に `pub mod a; pub mod b; pub mod c; pub mod types;`）。
  profile は `scripts/dev/repo-health/profiles/default.toml` を写し、size の試験だけ `module_lines` を小さく（例 40）する（base 側に置くので head では変えない）。
  本物の検査器 `scripts/dev/repo-health/repo_health.py --repo <一時 repo> --base <base> --head <head> --policy-version 1 --out <json>` を呼び、exit code と finding の鍵・delta を見る。
- base の循環は a→b→c→a（3 単位・3 組。a・b・c は `use crate::<x>::<型>;` で依存する）。`types` は何にも依存しない。

| 試験 | head の変更 | 期待 |
|---|---|---|
| `test_scc_edges_new_internal_edge_fails` | a.rs に `use crate::c::C;`（組 a→c を足す。節点は 3 のまま） | exit 1。`cycle-edge:…/a->…/c` が new、循環の finding は existing（値 3） |
| `test_scc_edges_existing_internal_edge_kept_passes` | a.rs に b の別の型の `use`（組 a→b は base にある） | exit 0。cycle-edge の finding 無し |
| `test_scc_edges_removed_edge_passes` | c.rs の a への `use` を消す（組を削除） | exit 0 |
| `test_scc_edges_legal_type_import_passes` | a.rs に `use crate::types::Id;`（循環の外の単位への型の import） | exit 0。cycle-edge の finding 無し |
| `test_scc_edges_super_path_new_internal_edge_fails` | a を `a/mod.rs`・`a/inner.rs` にし、inner.rs に `use super::super::c::C;` | exit 1。`cycle-edge:…/a->…/c` が new |
| `test_logical_module_path_move_size_fails` | base で b の行は閾値の下。head で b.rs の半分を `src/extra/b_impl.rs` に移し `#[path = "extra/b_impl.rs"] mod imp;` で読み、合計が閾値を超える | exit 1。`module_lines` の b が new。物理の単位では b・extra とも閾値の下 |
| `test_logical_module_path_move_deps_fails` | a.rs が `#[path = "misc/a_part.rs"] mod part;` で読む file に `use crate::c::C;` を書く | exit 1。`cycle-edge:…/a->…/c` が new（物理の単位なら misc→c で循環の外） |
| `test_logical_module_base_debt_is_existing` | base で既に上の 2 つ（閾値を超えた b の論理 module と、`#[path]` の file からの組 a→c）がある。head は無関係な file を変える | exit 0。`module_lines` の b は existing、cycle-edge の finding 無し |

- 守る不変条件は brief の I1〜I4。既存の試験（`test_repo_health_core.py`・`test_repo_health_structure.py` など）は循環の節点の数と物理の単位しか見ないので、これらを検出できない。

### D7. replay（旧と新の検査器の判定の比較）
- 対象 A: `git log --first-parent -n 10 bc665168c9ca -- 'crates/*.rs'` の 10 commit。base は各 commit の `^1`。base の時点では
  `bcddc8ae`・`e4120872`・`36082e8d`・`e5f39ebd`・`ce9a8a8b`・`affa5853`・`1725b80d`・`9a907113`・`c03a0684`・`65035ffb`（loc-reduction-v3 の F・G）。
- 対象 B（模擬 2 件）: `git commit-tree` で bc665168 の上に作る（作業ツリーと branch は変えない。一時の index で `hash-object -w`・`update-index`・`write-tree`）。
  - m1: task-core か task-api の循環の内部で、base に組の無い 2 単位を A の §7 C2 の JSON から選び、一方の production file に他方への `use crate::<単位>::<項目>;` を 1 行足す。期待: 旧 pass、新 fail（cycle-edge が new）。
  - m2: `crates/task-dispatch/src/dispatcher/` の production file を 1 つ `health_review/` に移し、元の宣言を `#[path = "../health_review/<file>"]` にして、本体の行を足す。期待: 旧は dispatcher の `module_lines` が減って pass、新は dispatcher が worsened で fail。
- 旧の検査器は `git archive bc665168c9ca scripts/dev/repo-health | tar -x -C $TMPDIR/old`、新は WorkUnit `checker` の HEAD の `scripts/dev/repo-health`。両方を同じ repo に
  `python3 <検査器>/repo_health.py --repo . --base <c>^1 --head <c> --policy-version 1 --out <json>` で当て、verdict・exit・fail 級の finding の鍵を並べる。
- 判定が変わった件は 1 件ずつ一覧にし、理由（新しい内部の組・論理 module の行・`super::` の辺）を書く。理由の無い変化は不具合として checker に戻す。
  1 回の health は約 1.5 分（A の §5.4）なので、24 回は 1 run に入らない。並べて回すか、run を分ける。

## 後続 WorkUnit への引き継ぎ
- `checker`: D1〜D5 を実装し、D6 の試験を足す。policy の branch で出す。背景の表の 16 file の単位を実装で確かめ、違えば進捗に書く。
- `metrics`: 共通受入条件 (1)〜(6) のうち B に関わる項目を A の §7 の command で before/after 計測する。(1) の循環の組は旧と新の検査器の両方で出す（単位の数え方が変わるため。A と違う数え方なので両方の値と理由を書く）。
- `replay`: D7。

## 付記 2026-11-17: 実装での違い（checker）
WorkUnit `checker` は D1〜D4 を既存の検査器 3 file（`health/lang_deps.py`・`rules/deps.py`・`rules/size_growth.py`）の修正で実装し、D6 の試験 8 件を足した。
決定の本文（D1〜D7）は書き換えず、実装で変えた 3 点をここに書く。詳細は WorkUnit の進捗 `agent-docs/progress/2026-11-17-arch-v4-b/checker.md` の「ADR との違い」。

- 人の承認: 人は 2026-11-17 に、この 3 点を含む policy の差分を承認した（運用 session の問いへの人の答えを operator が task のコメントで中継した）。
  operator は件名「integrate 01M7MZTKAZ checker: deps.cycle の循環の内部の新しい組を finding にし、Rust の単位を論理 module（#[path]・super::）にする（policy の branch、人の承認と operator の hand merge）」の no-ff の merge で main に入れた。WorkUnit は自分で承認していない。

1. D1 の組の finding（鍵 `cycle-edge:<src>-><dst>`）は、base の循環と突き合い節点の数が base を超えない head の循環（循環の finding が existing）にだけ出す。
   D1 の「新しい循環・併合した循環でも、base で循環の外にあった組を新しい組として出す」は実装していない。
   理由: その場合は循環の finding が new か worsened で既に fail 級になり、組の finding は同じ悪化の二重の報告になる。また既存の試験 `test_repo_health_structure.py` の 3 件
   （`test_new_python_cycle_fails_with_paths_and_lines`・`test_existing_cycle_that_grows_is_worsened`・`test_rust_module_cycle_ignores_crate_root_reexports`）は
   deps.cycle の finding がちょうど 1 件であることを確かめており、D1 のとおりだと落ちる。見逃しは増えない（節点が増えた循環は循環の finding が fail にする）。
2. D3 の `super::` の解決は、inline の `mod x { … }` block の深さも数える。D3 の限界の節は「block の深さを数えない」と書いた。
   理由: 数えないと、1 段の module の file の中の inline block にある `super::y`（本当は自分の単位の中）を `crate::y` と解決し、誤った辺を作る。
   D3 の「誤検出にはならない」はこの場合に成り立たない。block の範囲は rustfmt の字下げと `source_size.block_end` で決める。
3. 論理 module の表（`lang_deps` の `Graph.modules`）は、本体の file だけでなく、検査の対象の Rust の file 全部（試験・道具の file も）に作る。
   理由: `crate::x` が module かの判定は今まで本体の外の file も path の存在で見ていたので、表の lookup でも同じに扱うため。辺の元は今までどおり本体の file だけ。

既知の限界（WorkUnit `replay` が見つけた。直していない）:
- `#[path]` の宣言の行が `module_lines` に 1 行数えられる。`rules/size_growth.py` の `DECL_RE`（`^\s*(?:pub(...)\s+)?(?:use|mod)\s`）は、`#[path = "…"] mod x;` の 1 行の宣言も、
  2 行の形の `#[path = "…"]` の行も宣言として除かない。旧い検査器も同じ式だが、D4 で別 dir の `#[path]` の file を宣言した module に数えるので、別 dir への移動でもこの 1 行が効くようになった。
- 影響: 閾値を超えた module で `mod x;` を `#[path]` 付きに変えると、本体の行を変えない移動でも worsened（fail）になる。数え過ぎの側で、見逃しは無い。
- replay で当たった件数: main の実 commit 10 件では 0 件。模擬では m3（`crates/task-dispatch/src/dispatcher/write_set_gate.rs` を `health_review/` に移しただけ）の 1 件が、
  この 1 行だけで module `crates/task-dispatch/src/dispatcher` の `module_lines` 32,347→32,348 の worsened になった。m2・m2x の同じ module の増分（+24）にもこの 1 行が入っているが、
  m2・m2x は足した本体の 23 行でも fail になる。
- 扱い（2026-11-17 の決定。CoS の回答で、人の選択ではない）: この付記に既知の限界として書き、修正は別の policy の task（policy だけの branch、人の差分の承認、operator の merge）として提案する。起票は人が決める。
  アーキ v4 の後続 C〜H がこの限界で size の fail に当たったら、検査を外したり迂回したりせず、この決定を引いて人に聞く。
