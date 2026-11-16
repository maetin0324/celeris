---
title: 健全性v2 V0 loc — B0 と同じ方法で 4 見方の行数・class 別の純増減・依存と共有状態を数え直す
tasks: [01M6TYVJT97XH88TEEAVRD7JK6]
status: done
updated: 2026-11-15
completed: 2026-11-15
---
# 健全性v2 V0 loc

完了日: 2026-11-15

## 結論
- B0 の基準（slices.json の base_sha）から計測点（この WU の開始時の HEAD。`git merge-base HEAD main` と同じ commit で、
  件名「integrate wu/close (phase close)」）までに、本体のみ（product_only）は **+19,935 行**（366,177 → 386,112）増えた。
  健全性 v2 の期間に本体は減っていない。
- 本体の増加の大半は健全性 v2 の slice 自身が足した機能（G2 +3,945・G3 +5,074・R4 +9,789 ほか、計 +18,884）。
  行を減らした slice は R1（−369）と R6（−741）だけで、合計 −1,110 行。他の task と取り込みの分は +1,051 行。
- 4 見方・class × 言語の数え方は B0 と同じ（分類規則 `source-size-exclusive/1`、config の sha256 が B0 と同じ）。
  base を数え直すと slices.json の loc_views と 4 見方すべてで一致する（`--verify` で確かめる）。
- 依存: crate 依存の辺 49 → 49、crate 循環 0 → 0、module 循環 3 → 3（同じ 3 組）、module の use 1,157 → 1,296（+139）。
  共有状態: 複数の crate が書く SQLite 表 3 → 1（縮小）、複数の書き手の表 14 → 15、複数の書き手の event 種類 20 → 25、
  複数の書き手の file 5 → 5。依存と共有状態は全体として縮んでいない。
- 見かけの増減: R4・R5 の試験の file 分割（`*_suite/` への移動）で約 1.9 万行が file 間で動いたが、class の合計は変わらない。
  分類の付け替えは 1 file（`crates/celeris/src/config/jev.rs`、inline_test → production、147 行）。生成や vendor への移動による
  本体の見かけの削減は無い（vendor の差 0、generated +997）。

## 4 見方（B0 の基準・merge-base・計測点）

| 見方 | B0 の基準 | merge-base | 計測点（HEAD） | 差 | うち Rust の差 |
|---|---:|---:|---:|---:|---:|
| 本体のみ（product_only） | 366,177 | 386,112 | 386,112 | +19,935 | +19,937 |
| 自前総量（own_total_with_tests） | 967,314 | 1,042,476 | 1,042,476 | +75,162 | +65,340 |
| 全追跡行（all_tracked） | 1,512,205 | 1,887,008 | 1,887,008 | +374,803 | +65,340 |
| 外部分離候補（external_split_candidates） | 256,886 | 257,152 | 257,152 | +266 | +240 |

この WU の開始時は HEAD と `git merge-base HEAD main` が同じ commit だった。この WU の commit は agent-docs の下だけを
変える（docs class が増えるだけで、本体・試験の数は変わらない）。

## class × 言語の純増減（計測点 − B0 の基準）

| class | rust | typescript | python | shell | sql | markdown | json | toml | other | 計 |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| production | +19,937 | +6 | 0 | 0 | 0 | 0 | 0 | −8 | 0 | +19,935 |
| external_test | +42,803 | +46 | +3,547 | +507 | 0 | 0 | 0 | 0 | 0 | +46,903 |
| inline_test | +2,360 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | +2,360 |
| generated | 0 | +159 | 0 | 0 | 0 | 0 | +838 | 0 | 0 | +997 |
| vendor | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| fixture | 0 | 0 | +84 | 0 | 0 | 0 | +83 | 0 | 0 | +167 |
| evaluation_infra | +240 | 0 | 0 | +26 | 0 | 0 | 0 | 0 | 0 | +266 |
| tooling | 0 | 0 | +4,694 | +505 | 0 | 0 | 0 | +376 | +4 | +5,579 |
| docs | 0 | 0 | +9,070 | +1,108 | 0 | +31,696 | +202,173 | 0 | +41,668 | +285,715 |
| schema | 0 | 0 | 0 | 0 | 0 | 0 | +12,761 | 0 | 0 | +12,761 |
| migration | 0 | 0 | 0 | 0 | +119 | 0 | 0 | 0 | 0 | +119 |
| lockfile | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | +1 | +1 |

javascript・yaml の列はすべて 0 なので省いた。全追跡行の増加の 76%（+285,715）は docs で、その 7 割は agent-docs/progress の
計測 raw の json（+202,173）。生の数は [loc.json](loc.json) の `counts.*.matrix` と `diffs.head_minus_base.matrix`。

## slice ごとの帰属（本体の増減は誰が足し、誰が減らしたか）

各 slice の task branch（`refs/heads/celeris/<task id>`）の first-parent を辿り、その slice の WU の統合と直接の commit の差を
class × 言語で足した。main や他の slice の取り込み（`Merge commit '…' into`・`sync: main`）は除いた。

| slice | 自身の commit | 本体のみ | 自前総量 | 主な中身（進捗との突き合わせ） |
|---|---:|---:|---:|---|
| B0 | 7 | 0 | +993 | 計測の script と raw（docs・tooling） |
| G1 | 6 | 0 | +5,741 | repo health の検査器（scripts/dev/repo-health、tooling と試験） |
| G2 | 60 | +3,945 | +11,626 | 設計 review・health verdict・policy の関門（新機能） |
| G3 | 12 | +5,074 | +16,149 | 通常の開発 loop への強制・証跡・費用上限（新機能） |
| G4 | 12 | 0 | +4,588 | 非 Celeris の fixture の試験 |
| R1 | 5 | −369 | −175 | 5 adapter の結果契約の共通化（R1 の進捗の −597 / +184 と同じ向き） |
| R2 | 12 | +276 | +2,105 | EventSink/WorkerAdapter の能力分離 |
| R3 | 9 | +298 | +600 | TaskStore の port と表の書き手の整理 |
| R4 | 21 | +9,789 | +31,766 | Dispatcher の状態所有の分割。下の注意を参照 |
| R5 | 22 | +612 | +2,352 | 試験の束ね（`*_suite/`）と計測 |
| R6 | 10 | −741 | −210 | 旧経路の削除（R6 の進捗の −741 と一致） |
| slice の計 | 176 | +18,884 | +75,535 | |
| 他の task と取り込み | — | +1,051 | −373 | 総差 − slice の計 |

注意（数の読み方）:
- R4 の自身の commit には R5 の試験の移動（`browser_live_cross.rs` → `browser_eval_surface_suite/` など）と R5 の計測 raw
  （docs 約 20 万行）が重なって入っている。R4 の WU の 1 つが R5 を取り込んだ branch から統合されたためと見られる（未確認）。
  このため slice の docs の計（+479,786）は総差（+285,715）を超え、「他」の docs が −194,071 になる。
  docs と external_test の slice 別の数は二重計上を含むので使わない。production の slice 別の数は進捗（R1・R6）と合う。
- 帰属は first-parent の構造から決めた推定で、取り込みの merge の衝突解消の差は「他」に入る。

## 見かけの増減と実質の増減

| 種類 | 量 | 扱い |
|---|---:|---|
| 実質の削減（本体） | −1,110 行（R1 −369・R6 −741） | 機能・経路を消した、または共通化した |
| 実質の増加（本体） | +18,884 − (−1,110) の残り、slice の計 +18,884 | G2・G3・R4 の新機能と分割の受け皿 |
| file 分割・行の移動（slice 内、strip 後 12 文字以上の同じ行） | external_test → external_test 約 1.9 万行（R4・R5 の重複分を含む）、production → production 約 3 千行（R4 の `dispatcher/cluster.rs` → `cluster_runtime.rs` 813 行など） | 合計は変わらない。削減に数えない |
| class をまたぐ移動（slice 内） | production → external_test 約 180 行、production → inline_test 約 33 行 | 本体の見かけの減少。量は小さい |
| 分類の付け替え（同じ path の class の変化） | 1 file、147 行（`crates/celeris/src/config/jev.rs` inline_test → production） | 削減に数えない |
| 生成・vendor・別 repo への移動 | vendor の差 0、generated +997（schema の生成の増加）、外部分離候補 +266 | 本体を外へ移した削減は無い |

本体の純増 +19,935 行のうち、実質の削減は −1,110 行だけ。健全性 v2 の行数の効果は「増やした機能の分だけ増えた」で、
削減の成功として数えられる分は R1 と R6 の 1,110 行に限られる。

## 依存と共有状態（B0 の module-deps.json と並べる）

| 指標 | B0 | 計測点 | 差 |
|---|---:|---:|---:|
| crate 依存の辺（workspace member 間） | 49 | 49 | 0 |
| module の use（6 crate、src 直下の module 単位） | 1,157 | 1,296 | +139 |
| module 循環（強連結成分） | 3 | 3 | 0（同じ 3 組: task-core 10 module・task-api 2・task-api 9） |
| crate 循環 | 0 | 0 | 0 |
| SQLite 表（CREATE TABLE） / 生きている表 | 135 / 120 | 138 / 121 | +3 / +1 |
| 複数の書き手の表 | 14 | 15 | +1（`chat_runs`・`cos_notification_routes` が加わり、`chat_client_requests` が外れた） |
| 複数の crate が書く表 | 3 | 1 | −2（R3 の書き手の整理） |
| 書き手の無い生きている表 | 0 | 1 | +1（`route_judgments`） |
| event の種類 / 複数の書き手の種類 | 108 / 20 | 114 / 25 | +6 / +5（`auto_replan_requested`・`browser_prerequisite_blocked`・`plan_approval_requested`・`question_raised`・`worker_started`） |
| 複数の crate が書く event の種類 | 17 | 17 | 0 |
| daemon と worker がともに書く file | 5 / 6 | 5 / 6 | 0 |

方法は B0 と同じ: `python3 agent-docs/progress/2026-11-07-health-v2-b0/module-deps-measure.py <rev>` を `<rev>` の tree
（`git archive` で `$TMPDIR` に展開）で実行した。base の tree で実行すると B0 の module-deps.json と全欄（generator を除く）で一致した。

## 実行したコマンドと結果（提出時の HEAD で実行）
- `python3 scripts/dev/source-size-report.py --exclusive-classes --rev <B0 の base_sha> --format json`・`--rev HEAD`: exit 0、
  totals.consistent=true。base は product_only 366,177・own_total_with_tests 967,314・all_tracked 1,512,205・
  external_split_candidates 256,886（slices.json と同じ）。
- base の tree で `module-deps-measure.py`: exit 0、B0 の module-deps.json と summary・crate_deps・module_uses・cycles・shared_state が一致。
- `python3 agent-docs/progress/2026-11-15-health-v2-v0/loc-compare.py --build agent-docs/progress/2026-11-15-health-v2-v0/loc.json --head <計測点>`:
  exit 0（約 3 分 40 秒）。
- `python3 agent-docs/progress/2026-11-15-health-v2-v0/loc-compare.py --verify agent-docs/progress/2026-11-15-health-v2-v0/loc.json`: exit 0（約 37 秒）。
  base の product_only を 1 に書き換えた写しでは「loc.json の base product_only.lines 1 が slices.json の 366177 と違う」などを出して exit 1。
- `sh "$CELERIS_WU_SCOPE_PATHS"`: 変更 path は loc-compare.py・loc.json・loc.md の 3 件だけ。crates・web・control-ts・scripts・tests の差分 0。
- `sh scripts/dev/progress-index.sh --check`: exit 0。

## 比較の script
[loc-compare.py](loc-compare.py)。`--build <out>` は 3 rev の計測・帰属・移動・分類の付け替え・依存を数えて JSON に書く。
`--verify <loc.json>` は base の 4 見方を source-size-report で数え直して slices.json の loc_views と、head の 4 見方と class × 言語を
記録の sha で数え直して記録と比べ、差が head − base と合うこと、帰属の和（slice + 他）が総差と合うこと、依存の欄が揃うことを
確かめる。どれかが欠けるか食い違えば理由を出して 1 で終わる。

## 未解決事項
- R4 の帰属に R5 の変更が重なる理由（どの WU の統合が R5 を運んだか）は確かめていない。slice 別の docs・external_test の数は使わない。
- `route_judgments` は生きている表だが、非試験の書き手が見つからない。R6 routing の削除の残りか、読むだけの表かは未確認。

## 提案
- `route_judgments` の書き手の有無を確かめ、書き手が無いなら migration で表を消すか、残す理由を ADR に書く（R6 の残り）。
- 複数の書き手の event 種類が 5 増えた。close か次の健全性の段で、`question_raised` などの書き手を 1 つの owner に寄せるかを判断する。
- 帰属の二重計上を避けるため、WU の統合 commit の差を「WU の base → WU の先端」で数える方法（WU の base を記録から引く）に替える。
