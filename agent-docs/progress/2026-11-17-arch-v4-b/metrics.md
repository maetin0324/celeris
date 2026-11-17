---
title: アーキ v4 B metrics — 共通受入条件 (1)〜(6) を A の baseline の規則で before/after 計測する
tasks: [01M7MZTKAZQY72EX2R4RQ1WVWF]
status: done
updated: 2026-11-17
completed: 2026-11-17
---

# metrics（WorkUnit）

完了日: 2026-11-17。計測は 2026-11-17T20:48Z〜21:10Z（host home-dev、TZ は UTC）。
before は A の baseline（A の run の artifacts の `baseline.md`）の値と、base の commit から取り出した旧い検査器（main `bc665168c9ca` の
`scripts/dev`）。after は提出時の HEAD（件名「integrate wu/checker (phase implement)」。B の検査器は件名
「integrate 01M7MZTKAZ checker: …」の merge で main に入った）の新しい検査器。生の出力は run の artifacts の `raw/` にある。

## 結論と人が決めること
- (1) 循環: 件数は旧・新とも 17 件（Rust 14・web 3）。新しい検査器は `#[path]` と `super::` を数えるので、単位の合計 192 → 198、
  組 742 → 756、行の本数 1,731 → 1,827 になった。source は変わっていない（crates・web の差分 0）。値が変わった 4 件の循環と理由は §1。
- (5) health の時間は悪化した。新しい検査器の中央値は S1 92.81 s・S2 88.68 s で、A の before（S1 86.56 s・S2 81.92 s）より
  +6.25 s（+7.2%）・+6.76 s（+8.3%）。同じ時刻・同じ条件で測った旧い検査器（S1 82.08 s・S2 82.35 s）より +10.73 s（+13.1%）・+6.33 s（+7.7%）。
  user の CPU 時間は +1.5 s 前後（約 +2%）、max RSS は +28 MB。wall の差が CPU の差より大きい理由は未確認。判定（verdict・new・worsened）は旧と同じ。
- test-affected・cargo の build・review の時間と費用は再計測していない（理由は §5）。この 3 項目は「未達/要判断」。
- (6) LoC: production 0・production Rust 0・own_total_with_tests +393（tooling +161、external_test +232）。
- (2)(3)(4): B は crates・web を変えていない（`git diff --quiet bc665168c9ca HEAD -- crates web Cargo.toml Cargo.lock` が exit 0）。
  path で数える値は A と同じ。repo-health の単位で数える値（handlers⇄cos の単位の辺、§7 C7b）は数え方の変化で変わった（§3・§4）。
- 人が決めること: health の時間の悪化（+6〜11 s、+7〜13%）を受け入れるか。決めるのは closeout と G（反復速度の計測）で、この WorkUnit では直していない。

## 計測の条件
- A の §1 と同じ host・同じ env。Python 3.13.3、git 2.48.1。検査器は `git archive <rev> scripts/dev | tar -x -C $TMPDIR/{old,new}` で取り出し、
  `python3 -I $TMPDIR/<old|new>/scripts/dev/repo-health/repo_health.py --repo . …` で走らせた（新旧切替 flag は使わない）。
  `scripts/dev/repo-health` だけを取り出すと `source-size-report.py` と `check-architecture-map.py` が無く、rule が unsupported・error になる。
  `scripts/dev` を丸ごと取り出す（1 回目の試みの失敗。値は使っていない）。
- (1) の C1・C2 は 4 通り（旧・新 × bc665168・HEAD）を並列で走らせた（20:50:19Z〜20:51:52Z、開始時の 1 分 load 3.81）。値を見るだけで、時間は比べない。
- (5) の health は 1 回ずつ直列で、各回の直前に 1 分 load average が 4 以下になるまで待った（A の §7 C8 の waitload。待ち時間はどの回も 0 s）。

## 1. 循環（A の §7 C1・C2）

C1（`repo_health.py --base <rev> --head <rev> --policy-version 1`）の結果は 4 通りとも verdict=pass・exit 0・findings 697（new 0・worsened 0）。
rule ごとの件数は 4 通りで全部同じ（deps.cycle 17、size.responsibility-growth 94、dup.contract 210、arch-map.wrong_owner 31。A の §2 と一致）。
旧×bc665168 と旧×HEAD、新×bc665168 と新×HEAD はそれぞれ全部の値が同じだった（HEAD と bc665168 の source は同じ）。

C2（循環の全部の辺。`raw/cycle-edges-{old,new}-{b0,head}.json`）:

| 循環（先頭の単位） | 旧（= A の before）単位・組・行 | 新 単位・組・行 | 違いの理由 |
|---|---|---|---|
| task-core（approval ほか） | 55・210・617 | 55・210・617 | 同じ |
| task-api（approvals ほか） | 49・261・628 | 49・261・632 | `super::` の 4 行（`cos/ops/admin.rs:425,470,481,513` の `super::super::super::handlers`）が辺になった。組は既存の cos→handlers |
| campaign | 46・211・347 | 46・211・347 | 同じ |
| task-dispatch dispatcher | 3・4・22（dispatcher・review・undeclared_artifacts） | 4・7・51（＋health_review） | `#[path]` で health_review/ に置いた 8 file が dispatcher の単位になった。dispatcher→health_review 16 行（例 `health_review/check_item.rs:32`）と、本来の health_review の file からの health_review→dispatcher 3 行（例 `health_review/input.rs:54`）・health_review→review 3 行で health_review が循環に入った |
| task-worker | adapter 3・3・5 と browser 6・12・19 の 2 件 | adapter 12・24・85 の 1 件 | `protocol/cos_chat.rs:62` の `super::super::cos_chat` が protocol→cos_chat の辺になり、2 件がつながった。ほかに browser⇄browser_ledger（例 `browser_conformance.rs:51`、`browser_ledger.rs:16`）、cos_chat・preamble・tool_policy の辺が入った |
| task-dispatch project_controller | 無し | 2・2・2（project_controller・project_decider_ts） | `crates/task-dispatch/src/lib.rs:47` の `#[path = "dispatcher/project_decider_ts.rs"]` の file が論理の単位 project_decider_ts になり、project_controller と相互に参照する（`project_controller.rs:129`、`dispatcher/project_decider_ts.rs:38`） |
| ほか 11 件（task-ops comment 9・19・44 ほか、web 3 件） | — | 同じ | — |
| 合計 | 17 件（Rust 14・web 3）、単位 192・組 742・行 1,731 | 17 件（Rust 14・web 3）、単位 198・組 756・行 1,827 | |

- 新しい検査器が base と head の両方に同じ数え方を当てるので、base で既知の循環（新しく見えた 4 件の変化を含む）は existing のままで fail にならない
  （新×bc665168・新×HEAD とも new 0・worsened 0）。新しい組の finding（`cycle-edge:`）は base=head では 0 件（想定どおり）。
- size.responsibility-growth は件数 94 のまま、1 件の単位が変わった: `crates/task-worker/src/browser_launcher_run` → `crates/task-worker/src/browser`
  （`run_registered` 517 行。`#[path]` の file が論理の単位に移った）。
- A と違う値（task-api 628→632 行、dispatcher、task-worker、project_controller）は、B の ADR（2026-11-17 repo-health-scc-edges-logical-modules）の
  D2（論理 module の単位）と `super::` の解決で数え方が変わったためで、source の変化ではない。C〜H は A の値と新しい値を並べて書く。

## 2. 共有状態・SQLite の直接参照（A の §7 C3・C4）

B は crates を変えていないので値は A と同じ。HEAD で同じ command を当てて確かめた（`raw/c3-c6-head.txt`）:
dispatcher/ の非試験 69 file、self.store 44 file・633、self.config 35 file・218。`rusqlite` を書く production file 74、SQL 文字列を持つ file 50。
論理 scope（A の §6 の 1）は 76 file・self.store 48/661・self.config 39/224（A の値。file は変わっていない）。

## 3. HTTP・CoS の相互の import と SQL の公開面（A の §7 C5・C6）

- `git grep`（C5）: handlers→cos 22 行、cos→handlers 52 行（crate:: 48 ＋ super:: 4）。A と同じ（HEAD で実行）。
- repo-health の単位の辺（C2 の task-api の SCC）: handlers→cos は旧・新とも 22 本・13 file。cos→handlers は旧 48 本・9 file、新 52 本・9 file。
  新しい検査器は A の §6 の 2 が別に数えていた `super::` の 4 行を辺に含むので、単位の辺の数が `git grep` の数（52）と一致した。
- SQL の公開面（C6）: `impl SqliteStore` 102 個（HEAD で実行）。ほかの値（`&Connection` の引数、pub method 633 ほか）は crates が同じなので A と同じ。

## 4. 変更の波及（A の §7 C7b・C10、§5.2）

- 変える file・module・crate の数、再 build の範囲、test-affected の対象（A の §5.2）と C10（修正の波及 194 件・46.9%）は、source が同じなので A と同じ。
  bc665168..HEAD の first-parent の commit は A の進捗の commit と B の 2 つの merge で、変えた path は agent-docs と `scripts/dev/repo-health/`・
  `scripts/tests/` だけ（base の分類で production ではない。§6）。C10 は再実行していない。
- C7b（変える単位を import する単位。repo-health の単位で数える）:

| | 旧×bc665168（= A） | 新×HEAD | 理由 |
|---|---|---|---|
| S1 direct / transitive / packages | 92 / 369 / 12 | 93 / 365 / 12 | 単位の数え方の変化（`#[path]` の file が論理の単位に移り、`super::` の辺が入った）。source は同じ |
| S2 direct / transitive / packages | 6 / 14 / 4 | 6 / 14 / 4 | 同じ |

## 5. 時間と費用（A の §5.5 の health。test-affected・build・review は再計測していない）

health の手順は A の §5.5・§7 C8 と同じ。scenario の各 file の末尾に `SCENARIO_PROBE_<k>` の const を足し（毎回違う値）、
`repo_health.py --repo . --base <HEAD の sha> --head worktree --policy-version 1` の wall を `/usr/bin/time` で測り、測った後に `git checkout -- <files>` で戻した
（最後の `git status --porcelain` は 0 行）。A は `--base bc665168` だった。after は HEAD を base にして、base と head の差を probe だけにした
（A の head は A の作業ツリーに probe を足した状態で、bc665168 との差は A の進捗 file と probe だった）。S1・S2 を交互に走らせた（`raw/health_time.sh`、log は `raw/health-timing.log`）。

| | run 1 | run 2 | run 3 | 中央値 | user（中央値） | max RSS | 開始時の 1 分 load | verdict |
|---|---|---|---|---|---|---|---|---|
| before（A、旧い検査器） S1 | 87.82 s | 86.56 s | 81.48 s | 86.56 s | 69.45 s | 611 MB | 0.25〜1.28（S1・S2 を通して） | fail: new 3・worsened 1 |
| before（A、旧い検査器） S2 | 81.76 s | 81.92 s | 85.96 s | 81.92 s | 69.46 s | 612 MB | 同上 | fail: new 5・worsened 2 |
| after（新しい検査器） S1 | 93.84 s | 88.88 s | 92.81 s | 92.81 s | 71.04 s | 639 MB | 1.97 / 1.59 / 2.50 | fail: new 3・worsened 1 |
| after（新しい検査器） S2 | 88.68 s | 85.14 s | 88.74 s | 88.68 s | 71.23 s | 640 MB | 1.87 / 2.13 / 2.00 | fail: new 5・worsened 2 |
| 対照（旧い検査器、同じ時刻・同じ手順） S1 | 82.08 s | 81.78 s | 86.93 s | 82.08 s | 69.57 s | 611 MB | 1.99 / 1.75 / 1.73 | fail: new 3・worsened 1 |
| 対照（旧い検査器、同じ時刻・同じ手順） S2 | 81.81 s | 88.38 s | 82.35 s | 82.35 s | 69.67 s | 611 MB | 1.78 / 1.95 / 1.96 | fail: new 5・worsened 2 |

- 時刻: after 20:52:57Z〜21:01:56Z、対照 21:02:05Z〜21:10:28Z。
- 判定: health の時間は悪化した（A 比 S1 +7.2%・S2 +8.3%、対照比 S1 +13.1%・S2 +7.7%）。user の CPU は +1.5 s 前後で、wall の差の大部分は
  CPU の差で説明できない。原因（lang_deps の論理 module の表を base と head・rule ごとに作り直しているか等）は未確認。
- 判定（findings・new・worsened）は旧と新で同じ。scenario の probe では新しい組の finding は出ない（既存の単位に const を足すだけで、use を足さない）。

再計測していない項目（「未達/要判断」。達成とは書かない）:

| 項目 | before（A） | 再計測しない理由 |
|---|---|---|
| test-affected の時間（S1 893.41 s・S2 655.59 s） | A の §5.5 | B は crates を変えないので、cargo の build と試験の範囲・件数は同じ。ただし test-affected は最初に check-health を走らせるので、上の health の差（+6〜11 s）だけ長くなる見込み（未計測） |
| cargo check・test build の cold・warm | A の §5.3 | crates・Cargo.toml・Cargo.lock の差分 0。build の入力が同じ |
| review の時間と費用（reviewer run の wall 中央値 1,137.5 s・$2.047） | A の §5.4 | B は reviewer の prompt・profile の `[review]`・crates を変えていない。C11 は期間の実績の集計なので、B の前後を切り分けられない |

## 6. LoC（A の §7 C12。分類規則は base に固定）

script と toml は task の base `f549b50b7ccc` から `git show` で取り出した（bc665168 の版と `cmp` で同じ）。
`python3 -I $TMPDIR/ssr.py --root . --config $TMPDIR/ssr.toml --exclusive-classes --rev <rev> --format json`（`raw/ssr-{f549b50b,HEAD}.json`）:

| | base f549b50b | HEAD | 差 |
|---|---|---|---|
| production 全言語（`views.product_only.lines`） | 375,099 | 375,099 | 0 |
| production Rust（`views.product_only.rust_lines`） | 330,408 | 330,408 | 0 |
| own_total_with_tests（`views.own_total_with_tests.lines`） | 1,025,677 | 1,026,070 | +393 |
| own_total_with_tests の Rust | 879,344 | 879,344 | 0 |
| 試験（external_test + inline_test） | 455,363 + 21,408 | 455,595 + 21,408 | +232 |
| tooling | 25,253 | 25,414 | +161 |

HEAD は main の他の変更を含まない（f549b50b..HEAD の first-parent は B の 2 つの merge だけ）。B の path だけの `git diff --numstat f549b50b HEAD`:

| path | 追加 | 削除 |
|---|---|---|
| `scripts/dev/repo-health/health/lang_deps.py` | 151 | 31 |
| `scripts/dev/repo-health/rules/deps.py` | 45 | 6 |
| `scripts/dev/repo-health/rules/size_growth.py` | 10 | 8 |
| `scripts/tests/test_repo_health_scc_edges.py` | 232 | 0 |

検査器の 3 file の純増 161 行が tooling +161 と一致し、試験 file の 232 行が external_test +232 と一致する。ほかの差分は agent-docs（docs の class）だけ。

## 再現コマンド（repo の root で）
```sh
B0=bc665168c9caf1898adc4c611404bbee056daf9a; H=$(git rev-parse HEAD)
mkdir -p $TMPDIR/old $TMPDIR/new
git archive $B0 scripts/dev | tar -x -C $TMPDIR/old; git archive HEAD scripts/dev | tar -x -C $TMPDIR/new
# (1) C1（4 通り）
python3 -I $TMPDIR/<old|new>/scripts/dev/repo-health/repo_health.py --repo . --base <$B0|$H> --head <$B0|$H> --policy-version 1 --out $TMPDIR/health.json
# (1) C2: A の §7 C2 の snippet の sys.path と rules/deps.py の場所を検査器の dir に替えたもの（artifacts の raw/c2.py）
python3 -I raw/c2.py $TMPDIR/<old|new>/scripts/dev/repo-health <$B0|$H> $TMPDIR/cycle-edges.json
# (4) C7b: A の §7 C7b の snippet の検査器の dir を引数にしたもの（raw/c7b.py）
python3 -I raw/c7b.py $TMPDIR/new/scripts/dev/repo-health $H S1:crates/task-api/src/handlers,crates/task-api/src/cos,crates/task-core/src/store S2:crates/task-dispatch/src/dispatcher,crates/task-dispatch/src/run_settlement
# (2)(3) A の §7 C3〜C6 を B=$H で（raw/c3-c6-head.txt）、source の同一性
git diff --quiet $B0 HEAD -- crates web Cargo.toml Cargo.lock; echo $?     # 0
# (5) health の時間（S1・S2 を交互に 3 回。probe を足し、waitload、計測、git checkout で戻す）
bash raw/health_time.sh $TMPDIR/new/scripts/dev/repo-health new 3; bash raw/health_time.sh $TMPDIR/old/scripts/dev/repo-health old 3
# (6) LoC
git show f549b50b:scripts/dev/source-size-report.py > $TMPDIR/ssr.py; git show f549b50b:scripts/dev/source-size-report.toml > $TMPDIR/ssr.toml
python3 -I $TMPDIR/ssr.py --root . --config $TMPDIR/ssr.toml --exclusive-classes --rev <f549b50b|HEAD> --format json --out $TMPDIR/ssr.json
git diff --numstat f549b50b HEAD -- scripts/dev/repo-health/ scripts/tests/test_repo_health_scc_edges.py
```

## リリースゲートと回帰条件
- リリースゲート: 未実施（統合後の release の gate は人と operator が行う）。本番への昇格はしていない。
- 回帰条件: 新しい検査器を HEAD に当てた C1 が verdict=pass・new 0・worsened 0、deps.cycle 17 件、C2 の合計が単位 198・組 756・行 1,827 から
  理由なく変わらないこと。health の S1・S2 の中央値が本表の after（92.81 s・88.68 s）より大きく延びないこと。

## 未解決
- health の時間の悪化（+6〜11 s）の原因は未確認。CPU の差（+1.5 s）より wall の差が大きい。
- test-affected・cargo の build・review の時間と費用は前後比較していない（「未達/要判断」）。

## 提案
- closeout か G が、新しい検査器の health の時間を profile（`python3 -X importtime` や cProfile で lang_deps.build の呼び出しの回数）で調べ、
  論理 module の表を base・head ごとに 1 回だけ作る形で戻せるか確かめる。policy の file の変更になるので、policy だけの branch と人の承認が要る。
