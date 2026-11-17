---
title: アーキ v4 B replay — 旧と新の repo-health の検査器で main の実 commit 10 件と模擬の判定を比べる
tasks: [01M7MZTKAZQY72EX2R4RQ1WVWF]
status: done
updated: 2026-11-17
completed: 2026-11-17
---

# replay（WorkUnit）

完了日: 2026-11-17

## 結論
- ADR [2026-11-17 repo-health-scc-edges-logical-modules](../../adr/2026-11-17-repo-health-scc-edges-logical-modules.md) D7 の実 commit 10 件では、旧（main `bc665168c9ca` の検査器）と新（提出時の HEAD の検査器。人の承認の後に operator が main に merge した版）の exit と new/worsened の finding が 10 件とも同じだった。Celeris repo の判定が fail に変わった commit は 0 件。
- 模擬 m1（既存の循環の中で組の無い向きの `use` を 1 行足す）は旧 pass・新 fail、模擬 m2（巨大 module の file を `#[path]` で別 dir に移して本体の行を足す）は旧 pass・新 fail で、期待どおりだった。
- 誤検出の候補を 1 件見つけた（模擬 m3）。本体の行を足さない純粋な移動でも、`#[path]` の宣言の行が `module_lines` に 1 行数えられ、閾値を超えた module では新が worsened（fail）にする。原因は `scripts/dev/repo-health/rules/size_growth.py` の `DECL_RE` が `#[path = …]` の行を宣言として除かないこと（旧の検査器と同じ式。新は `#[path]` の file を宣言した module に数えるので、別 dir への移動でこの 1 行が効くようになった）。この WorkUnit は policy の file を変えないので直さず、repair を申告する（「未解決」）。

## 方法
- 旧の検査器: `git archive bc665168c9ca scripts/dev/repo-health scripts/dev/source-size-report.py scripts/dev/source-size-report.toml` を `$TMPDIR/old` に展開した。新: 同じ path を提出時の HEAD から `$TMPDIR/new` に展開した（作業ツリーの `scripts/dev/repo-health` と差 0 を確認）。旧と新の差は `health/lang_deps.py`・`rules/deps.py`・`rules/size_growth.py` の 3 file だけ。
  - `scripts/dev/repo-health` だけを展開すると、検査器は `<検査器の dir>/../source-size-report.py` を読めず、3 rule とも `unsupported`（exit 2）になる。`source-size-report.py` も一緒に展開する（base と HEAD で差 0）。
- 実 commit: `git log --first-parent -n 10 bc665168c9ca -- 'crates/*.rs'` の 10 件（R1〜R10）。base は各 commit の `^1`。10 件とも main の祖先（`git merge-base --is-ancestor` で確認）。
- 両方の検査器を `python3 -I <検査器>/repo_health.py --repo . --base <c>^1 --head <c> --policy-version 1 --rules deps.cycle,deps.violation,size.responsibility-growth --out <json>` で当てた。profile は base の tree から解決される（旧新とも同じ `default`）。`policy.changed` は常に評価される（全件 pass）。
- 模擬: branch と作業ツリーを変えず、一時の index（`GIT_INDEX_FILE`）で `read-tree`・`hash-object -w`・`update-index`・`write-tree`・`commit-tree` を使い、提出時の HEAD を親にした一時 commit を作った（ref は作っていない）。base は提出時の HEAD。

## 実 commit 10 件の旧新の判定
列: 旧 exit・新 exit、new/worsened の finding（旧・新）、所要時間（旧/新、秒）。findings の数は全件で旧 111・新 111（deps.cycle 17・size 94。全部 existing）。

| R | commit | 件名（先頭） | 旧 exit | 新 exit | 旧の new/worsened | 新の new/worsened | 判定 | 秒（旧/新） |
|---|---|---|---|---|---|---|---|---|
| R1 | bcddc8ae | loc-reduction-v3 G: 重複した試験を統合する | 0 | 0 | 無し | 無し | 同じ | 30.65/31.31 |
| R2 | e4120872 | loc-reduction-v3 F: 消した /org/:id・/org/secretary を開く e2e 3 件を消し… | 0 | 0 | 無し | 無し | 同じ | 26.50/28.02 |
| R3 | 36082e8d | loc-reduction-v3 F: 撤去した Console・reports/notified・aider を現行として書いた注釈… | 0 | 0 | 無し | 無し | 同じ | 26.50/27.96 |
| R4 | e5f39ebd | loc-reduction-v3 F: 研究系 adapter の terminal_from_summary の写し 3 つを… | 1 | 1 | size fn_lines worsened 2 件（task-worker `local_deep_research::run_ldr` 325→328、`paperqa::run_paperqa` 484→487） | 旧と同じ 2 件 | 同じ | 30.67/31.33 |
| R5 | ce9a8a8b | loc-reduction-v3 F: CoS の messages 行の写しを legacy スレッドに入れず… | 0 | 0 | 無し | 無し | 同じ | 30.55/31.27 |
| R6 | affa5853 | loc-reduction-v3 F: cos_problem を chat_problem に 1 つにし… | 0 | 0 | 無し | 無し | 同じ | 31.18/32.82 |
| R7 | 1725b80d | loc-reduction-v3 F: 研究系 adapter のランナーの約束… | 0 | 0 | 無し | 無し | 同じ | 31.20/32.77 |
| R8 | 9a907113 | loc-reduction-v3 F: task-api の admin channel の送信と待ちの写し 13 か所を admin::call に… | 0 | 0 | 無し | 無し | 同じ | 26.00/27.53 |
| R9 | c03a0684 | loc-reduction-v3 F: task-worker の試験だけが呼ぶ本体の関数… | 0 | 0 | 無し | 無し | 同じ | 25.95/27.37 |
| R10 | 65035ffb | loc-reduction-v3 F: delivery::advance を伸ばさず通知を CoS チャットへ写し… | 0 | 0 | 無し | 無し | 同じ | 33.03/33.62 |

- 判定が変わった commit: 0 件。`cycle-edge:` の finding は 10 件とも出なかった（どの commit も既存の循環の内部に新しい組を足していない）。
- R4 の fail は旧の検査器でも同じ 2 件で、新の検査器による変化ではない（関数の行が閾値 200 を既に超えた 2 関数に 3 行ずつ足した）。
- existing の finding の中身は、10 件とも同じ形で旧新が違う（鍵が変わったもの 4 組と値が変わったもの 2 件）。どれも base と head を同じ版の検査器で数えるので delta は existing のままで、判定は変えない。理由は WorkUnit checker の進捗（[checker.md](checker.md) の「旧と新の比較」）と同じ:
  - deps.cycle: task-dispatch の dispatcher の循環が 3 単位→4 単位（`#[path]` の 8 file が dispatcher になり health_review と相互の辺で閉じた）。task-dispatch に project_controller・project_decider_ts の 2 単位の循環が加わった（`lib.rs` の `#[path]` で project_decider_ts が単位になった）。task-worker の adapter の 3 単位の循環と browser の 6 単位の循環が 12 単位の 1 つに併合した（`protocol/cos_chat.rs` の `super::super::cos_chat` の辺と、`browser_conformance.rs` の論理 module が browser になった辺）。
  - size: dispatcher の module_lines 31,124→32,347、`Dispatcher` の type_methods 352→377（health_review の `#[path]` の 8 file が dispatcher に入った）。fn_lines `run_registered`（517 行）の単位の鍵が `browser_launcher_run` から `browser` になった（論理 module）。

## 模擬
| 模擬 | head の変更（親は提出時の HEAD） | 旧 exit | 新 exit | 新の fail 級の finding | 期待 | 秒（旧/新） |
|---|---|---|---|---|---|---|
| m1 | `crates/task-api/src/releases.rs` に `use crate::knowledge::KnowledgeItem;` を 1 行足す（task-api の 49 単位の循環の内部の 2 単位。releases→knowledge の組は base に無い） | 0 | 1 | deps.cycle new: 「head の 49 単位の循環の内部に新しい組 releases -> knowledge（行 1 本）」。循環の finding は旧新とも existing（値 49） | 新 fail・旧 pass: 合う | 30.35/31.32 |
| m2 | `crates/task-dispatch/src/dispatcher/write_set_gate.rs`（122 行）を `health_review/write_set_gate.rs` に移し、`dispatcher.rs` の `mod write_set_gate;` を `#[path = "health_review/write_set_gate.rs"] mod write_set_gate;`（1 行）にし、移した file に本体 23 行の関数を足す | 0 | 1 | size worsened: dispatcher の module_lines 32,347→32,371 | 新 fail・旧 pass: 合う | 30.08/31.50 |
| m2x | m2 と同じで、宣言を rustfmt の形の 2 行（`#[path = "…"]` と `mod write_set_gate;`）にする | 1 | 1 | size worsened 2 件: dispatcher の module_lines 32,347→32,371、`dispatcher.rs` の file_lines 2,012→2,013 | 旧も fail（下の注） | 30.17/32.02 |
| m3 | m2 の移動だけ（本体の行を足さない。宣言は 1 行の `#[path]`） | 0 | 1 | size worsened: dispatcher の module_lines 32,347→32,348 | 誤検出の候補（「未解決」） | 31.20/32.75 |

- m1: 旧の検査器も循環の組の数は 261→262 と数えたが、値は節点の数（49 のまま）なので existing で pass した。新は組 releases→knowledge を base の循環の内部の組と突き合わせ、new（error）にした。
- m2: 旧の検査器では移した file の行が物理の単位 health_review に移り、dispatcher の module_lines は 31,124→31,043（−81）に下がって existing、health_review は閾値 8,000 の下のままで finding が出ず、pass した（すり抜け）。新は file を宣言した module（dispatcher）に数え、+24 行（足した 23 行と宣言の行 1 行）で worsened にした。deps.cycle は旧新とも pass（移した file の `use` は dispatcher の既存の辺だけ）。
- m2x（注）: `dispatcher.rs` は base で file_lines 2,012（閾値 2,000 超え）なので、`#[path]` の属性の行を 1 行足すだけで旧も file_lines の worsened で fail する。移動の効果だけを見るため、宣言を 1 行で書いた m2 を正の結果とした（rustc は 1 行の形も読み、新の `MOD_DECL_RE` も 1 行の形を読む）。最初の試行（m2x と同じ 2 行の形）の report は artifacts の `replay/mock-first-try/` にある。

## 誤検出の判断
- 実 commit 10 件: 判定の変化が 0 件なので、誤検出は無い。
- m3（誤検出の候補）: 本体の行を変えない `#[path]` の移動で、新は dispatcher の module_lines を 32,347→32,348 の worsened（fail）にする。増えた 1 行は `dispatcher.rs` の `#[path = "…"] mod write_set_gate;` の行。`rules/size_growth.py` の module_lines は「空行・注釈・use・mod 宣言を除く」と書き、`DECL_RE`（`^\s*(?:pub(...)\s+)?(?:use|mod)\s`）で宣言を除くが、`#[path` で始まる 1 行の宣言も、2 行の形の `#[path = "…"]` の行も除かない。
  - 旧の検査器も同じ `DECL_RE`（差 0）で、同じ dir の中の `#[path]` なら旧でも 1 行増える。新は別 dir の file を宣言した module に数える（D4）ので、別 dir への移動でもこの 1 行が効くようになった。
  - 影響: 閾値を超えた module（例 dispatcher 32,347、閾値 8,000）で `mod x;` を `#[path]` 付きに変えると、本体の行が増えていなくても fail になる。fail の文面は「module の本体の行の合計が閾値を超えた」で、実際には本体の行は増えていない。見逃しは増えない（数え過ぎの側）。
  - 直し方の候補（policy の file。この WorkUnit は変えない）: `size_growth.py` の宣言の判定で、`#[path = "…"]` だけの行と `#[path = "…"] mod x;` の行も宣言として除く。試験は `scripts/tests/test_repo_health_scc_edges.py` に「純粋な `#[path]` の移動 → pass」の対照を足す。

## 所要時間
- 1 回の検査（3 rule）は 25.95〜33.62 s（24 回の範囲。旧 25.95〜33.03 s、新 27.37〜33.62 s。新は同じ commit で旧より 0.6〜1.6 s 長い）。
- 壁時計: R1 は旧新を並べて 32 s、R2〜R10 は 2 commit ずつ（4 process）並べて 153 s、模擬 m1・m2・m2x は 6 process を並べて 32 s、m3 は 2 process で 33 s。

## 再現のコマンド
repo の作業ツリーで実行する（`$TMPDIR` は run の一時 dir。生の report は `$O` に書く）。同じ script を run の artifacts の `replay/scripts/` に置いた。

```sh
O=$TMPDIR/replay; mkdir -p "$O" "$TMPDIR/old" "$TMPDIR/new"
for v in old:bc665168c9ca new:HEAD; do
  git archive "${v#*:}" scripts/dev/repo-health scripts/dev/source-size-report.py scripts/dev/source-size-report.toml | tar -x -C "$TMPDIR/${v%%:*}"
done
run1() {  # run1 <label> <old|new> <base> <head>
  python3 -I "$TMPDIR/$2/scripts/dev/repo-health/repo_health.py" --repo . --base "$3" --head "$4" --policy-version 1 \
    --rules deps.cycle,deps.violation,size.responsibility-growth --out "$O/$1.$2.json" > "$O/$1.$2.stdout"; echo "$1 $2 exit=$?"
}
i=0; for c in $(git log --first-parent -n 10 --format=%H bc665168c9ca -- 'crates/*.rs'); do
  i=$((i+1)); run1 R$i old "$c^1" "$c" & run1 R$i new "$c^1" "$c" & wait
done
```

模擬（一時の index で作る。branch・作業ツリーは変えない）:

```sh
P=$(git rev-parse HEAD); export GIT_INDEX_FILE=$TMPDIR/idx
# m1
git read-tree "$P"; f=crates/task-api/src/releases.rs
git show "$P:$f" | sed '0,/^use crate::/s//use crate::knowledge::KnowledgeItem;\nuse crate::/' > "$TMPDIR/m1.rs"
git update-index --cacheinfo "100644,$(git hash-object -w "$TMPDIR/m1.rs"),$f"
M1=$(git commit-tree "$(git write-tree)" -p "$P" -m "replay mock m1")
# m2（m3 は extra を空にする。m2x は sed の置き換えで改行を入れて 2 行にする）
git read-tree "$P"; src=crates/task-dispatch/src/dispatcher/write_set_gate.rs; dst=crates/task-dispatch/src/health_review/write_set_gate.rs
{ git show "$P:$src"; printf '\n/// replay の模擬 m2: #[path] で移した file に足した本体の行。\nfn replay_mock_m2_extra(n: usize) -> usize {\n'
  for k in $(seq 1 20); do printf '    let v%d = n + %d;\n' $k $k; done
  printf '    v1 + v2 + v3 + v4 + v5 + v6 + v7 + v8 + v9 + v10 + v11 + v12 + v13 + v14 + v15 + v16 + v17 + v18 + v19 + v20\n}\n'; } > "$TMPDIR/m2.rs"
git update-index --force-remove "$src"; git update-index --add --cacheinfo "100644,$(git hash-object -w "$TMPDIR/m2.rs"),$dst"
d=crates/task-dispatch/src/dispatcher.rs
git show "$P:$d" | sed 's|^mod write_set_gate;$|#[path = "health_review/write_set_gate.rs"] mod write_set_gate;|' > "$TMPDIR/m2d.rs"
git update-index --cacheinfo "100644,$(git hash-object -w "$TMPDIR/m2d.rs"),$d"
M2=$(git commit-tree "$(git write-tree)" -p "$P" -m "replay mock m2"); unset GIT_INDEX_FILE
for w in old new; do run1 M1 $w "$P" "$M1" & run1 M2 $w "$P" "$M2" & done; wait
```

判定の比較は artifacts の `replay/scripts/compare.py`（`python3 -I compare.py <O> R1 … R10 M1 M2 M2x M3`）で、`compare.json` と `compare.md` を出した。

## 成果物（run の artifacts の `replay/`）
- `R1`〜`R10`・`M1`・`M2`・`M2x`・`M3` の `<label>.<old|new>.json`（report）・`.stdout`・`.stderr`・`.meta`（exit と秒）。
- `compare.json`・`compare.md`（旧新の表と、鍵・値の違い）、`mocks.txt`（模擬の一時 commit）、`scripts/`（`run1.sh`・`mocks.sh`・`mock_m3.sh`・`compare.py`・`commits.txt`）。

## 検査（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- 変えた file: この進捗 file だけ（`sh "$CELERIS_WU_SCOPE_PATHS"` の出力は `agent-docs/progress/2026-11-17-arch-v4-b/replay.md` の 1 行）。検査器・profile・crates は変えていない。
- 再現のコマンド: 模擬の節のコマンドを一時の dir で走らせ、m1・m2 の tree が上の表の模擬と同じ tree になることを確かめた。R の節のコマンドは `bash -n` で exit 0。
- `bash scripts/dev/test-affected.sh`: exit 0（約 20 s）。最初の check-health が pass（findings 140、new 0・worsened 0。差分が doc だけで deps.*・size・source-size は not_applicable）、「試験対象の変更なし（docs のみ等）」で終わった。
- `cargo clippy --workspace -- -D warnings`: exit 0（約 50 s）。`cargo fmt --all -- --check`: exit 0。
- `sh scripts/dev/progress-index.sh --check --base "$CELERIS_WU_BASE"`: exit 0（ok）。

## 未解決
- repair の申告: m3 の誤検出の候補（`#[path]` の宣言の行が module_lines に 1 行数えられ、純粋な移動でも閾値を超えた module が worsened になる）。`scripts/dev/repo-health/rules/size_growth.py` は policy の file で、この WorkUnit の範囲（この進捗 file だけ）の外なので直していない。直すなら policy の branch（人の差分承認と operator の main への merge）が要る。実 commit 10 件の判定には影響していない。

## 提案
- closeout が ADR の付記に、m3 の数え過ぎを既知の限界として書くか、policy の修正の task を人に提案する（「誤検出の判断」の直し方の候補）。
