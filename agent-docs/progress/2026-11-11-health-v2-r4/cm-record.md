---
title: "健全性v2 R4 cm-record: equivalence の取り込みと change-map の要約"
tasks: [01M74VAMWFCC4JGZY4ENA92HJJ]
status: completed
updated: 2026-11-11
completed: 2026-11-11
---
# 健全性v2 R4 cm-record: equivalence の取り込みと change-map の要約

完了日: 2026-11-11。コードは変えていない。基準は `$CELERIS_WU_BASE`。書いたのは `agent-docs/progress/2026-11-11-health-v2-r4/` の change-map-measure.py・change-map.json・change-map.md・cm-record.md の 4 file だけ。

## 実装前 brief

1. 根本原因: 等価性の 6 記録（`equivalence/<id>.json`）が change-map.json に入っておらず、形や参照（file・行・関数）が壊れても検出する仕組みが無かった。
2. 責務 owner: change-map-measure.py（change-map.json を生成する唯一の script）。
3. 既存の再利用先: measure.py の `mask_rust`（comment・文字列を空白にする）と出力の `method` 欄。検査の項目は cm-equiv-run・cm-equiv-flow の validator と同じ観点（関数の実在・参照行の実在）。
4. 選択肢: (a) change-map.json に手で貼る、(b) 別の script で取り込む、(c) measure.py に取り込みと検査を足す。生成元を 1 つに保つため (c) を採った。
5. 新しい状態・分岐・fallback・port: 無し（読み込みと検査の関数だけ）。
6. 撤去対象: 出力の `method.not_in_this_unit`（`["equivalence"]`）を消し、`method.equivalence_rules` に置き換えた。
7. 不変条件: 再生成の前後で `fields` と `co_change` が変わらない。同じ HEAD なら同じ bytes を書く。違反が 1 件でもあれば出力を書かずに exit 1。下の証拠で確かめた。

## 変更

- `phase 2: import equivalence records into the R4 change map` — measure.py に `load_equivalence` を足し、change-map.json を再生成した。
  - 形の検査: key は id・sites・verdict・evidence・shared_contract だけ。id は小文字・数字・`-` で file 名と同じ。verdict は equivalent か different。shared_contract は空でない文字列。sites は 2 件以上で、各要素の key は at・fn・subject だけ（fn は識別子、subject は空でない文字列）。evidence は 1 件以上。
  - 参照の検査: sites の at と evidence の各要素は repo の root からの `path:行`。file が在り、行が file の行数以内。sites の fn は at の file に `fn <名前>` の宣言がある（comment・文字列は除く）。
  - 違反は全件を stderr に出し、出力を書かずに exit 1。記録は id の順に最上位の `equivalence` に入れる（中身は変えない）。
- `phase 2: summarize the R4 change map and record cm-record` — change-map.md の題の直後に `## 要約` を足した（field 数、owner ごとの field 数、owner_conflict の 12 field、equivalence 6 件の verdict と共有できる契約、7 関数の節と反例の節への案内、再生成のコマンド）。元の導入文には見出し `## 関数の節の読み方` を付けた。題を内容に合わせて改め、front matter を status: done・completed: 2026-11-11 にした。7 関数の節と `## 反例試験の不足` の本文は変えていない。この file を書いた。

## 各 unit の記録

| unit | 記録 | 内容 | commit |
|---|---|---|---|
| cm-access | [cm-access.md](cm-access.md) | field の読み書き file と共変更を測る script を作成 | `phase change-map: measure Dispatcher field access and co-change` |
| cm-owners | [cm-owners.md](cm-owners.md) | 116 field を 8 つの owner 候補へ割り当て、競合 12 件 | `phase 1: assign Dispatcher field owner candidates with evidence` |
| cm-equiv-run | [cm-equiv-run.md](cm-equiv-run.md) | run-close・lease-renew・retry-backoff の等価性 | `phase 1: document dispatcher run lifecycle equivalence` |
| cm-equiv-flow | [cm-equiv-flow.md](cm-equiv-flow.md) | continuation-resume・wu-failure・review-result の等価性 | `phase 1: record dispatcher flow equivalence measurements` |
| cm-functions | [cm-functions.md](cm-functions.md) | 大きい 7 関数の判断と副作用の区間 | `r4 cm-functions: record decision/side-effect intervals of 7 dispatcher functions` |
| cm-test-gaps | [cm-test-gaps.md](cm-test-gaps.md) | lease・restart・同時完了・review/integration・権限/予算の反例試験の不足 | `phase 1: document dispatcher counterexample test gaps` |
| cm-record | この file | equivalence の取り込みと要約 | 上の「変更」の 2 件 |

成果の本体は [change-map.md](change-map.md) と change-map.json。

## 証拠（提出時の HEAD で実行）

- `python3 agent-docs/progress/2026-11-11-health-v2-r4/change-map-measure.py --out agent-docs/progress/2026-11-11-health-v2-r4/change-map.json` → exit 0。fields 116 件（全件に owner）、equivalence 6 件（6 件とも different、sites 5〜9 件・evidence 10〜18 件）。最上位の key は co_change・equivalence・fields・method。
- 同じ command を `$TMPDIR` の別 file に出して `cmp` → exit 0（byte 一致）。
- 再生成の前（HEAD の change-map.json）と後を python3 で比較 → `fields` と `co_change` は一致、`method` は `not_in_this_unit` の削除と `equivalence_rules` の追加だけが違う。
- 反例 8 件（`$TMPDIR` に script・owners.json・equivalence/ を写して壊し、repo の root から実行）: fn の不在・行数超え・file の不在・key の欠け・verdict の不正・id と file 名の不一致・site の subject の欠け・0 件 → 8 件とも exit 1、出力 file は作られない。壊さない写しは exit 0。
- WorkUnit の check 0〜4（計画の cmd をそのまま実行）: 0 fields と equivalence の照合 → exit 0（`ok 116`）、1 change-map.json の 6 件 → exit 0、2 change-map.md の節と行参照 → exit 0、3 `sh scripts/dev/check-doc-links.sh && sh scripts/dev/check-adr-numbers.sh && sh scripts/dev/progress-index.sh --check` → exit 0、4 範囲 → exit 0。
- `bash scripts/dev/test-affected.sh` → exit 0。doc-only で試験対象の crate は 0。repo-health は pass（new 0・worsened 0・existing 141）。
- `cargo clippy --workspace -- -D warnings` → exit 0。依存の deno_core に既存の deprecated 警告 1 件。
- `cargo fmt --all -- --check` → exit 0。
- `sh "$CELERIS_WU_SCOPE_PATHS"` → 変更は上の 4 file だけ。`git diff "$CELERIS_WU_BASE" -- crates/ scripts/` → 差分 0。

全 workspace の試験・doc test・browser/userns・web e2e は走らせていない（release gate の範囲）。

## 未解決事項

- cm-functions の未解決「owner 候補の意味が cm-owners と食い違えば合わせる」: 7 関数の節の owner 候補は、その関数の module の owners.json の候補と 7 件とも一致した。関数ごとの owner は field の owner と別物で、関数の分け方は R4 の設計で決める。
- cm-access の提案にある `health_review/` の 9 file（`#[path]` で組み込まれる module）の field 参照は未測定のまま。measure.py の読む範囲は変えていない。owner_conflict が false でも競合が無いとは限らない。
- 等価性は静的な読みで、実行時の等価性を証明したものではない。measure.py の検査は参照の実在までで、verdict と shared_contract の内容の正しさ、fn が at の行を含むことは検査しない。
- 参照の行番号は各 unit の base の時点のもの。コードが動いて行や関数が消えれば、再生成が exit 1 で知らせる。行がずれただけ（行数以内で関数が残る）なら検出しない。

## 提案（R4 の分割で先に扱うもの）

1. 実行中の run の memory 上の追跡（`running`・`reviewing`・`checking`・`integrating`・`awaiting_human`・`pending_subjects`）と `infra_backoff`: owner_conflict 12 件のうちこの 7 件は、lease 回収（dispatcher/leases、run-supervisor）と起動・終了の側（dispatch_run・review_spawn・review_verdict・phase_integration・work_units、execution-coordination）の両方が書く。最初にこの集合の正本と書き手を 1 つの owner（候補は run-supervisor）に決め、他の module は遷移の要求を返すだけにする。change-map.md の `### lease`・`### restart`・`### 同時完了` の不足と同じ箇所なので、分割の前に決定的な反例試験（注入した時計・出来事待ち・試験専用 hook）を足す。
2. retry-backoff の重複（different）: worker 完了（`crates/task-dispatch/src/dispatcher/worker_finish.rs:578`）と lease 回収（`crates/task-dispatch/src/dispatcher/leases.rs:363`）の infra 上限判定を、`(infra_n, max)` から再試行するかを返す純粋な関数 1 つにし、`infra_backoff` の書き手を 1 つの owner にする。孤児の再起動で待たないこと・WorkerFinished の end/class の違いは呼び出し側に残す。
3. wu-failure の重複（different）: `finish_worker_result_with`（worker_finish.rs:902）、`deferred_work_unit_trigger`（work_units.rs:1219）、`integration_gives_up_with`（phase_integration.rs:1310）の 3 か所の replan 上限判定を、Replan か Ask と理由を返す純粋な判定 1 つにする。WU の行の更新・子 task の作り直し・承認と通知は各 owner に残す。
4. run-close・review-result（different）: on_review_finished の「reviewer run を閉じる」4 か所の写し（cm-functions）を終了記録の組み立て 1 つにし、決定的 check が不合格なら成功にしない判定を review と WU check で共有する。lease の検証・状態遷移・transaction の境界は一律の close(run) に入れない。
5. 後に回してよいもの: `accounts_scan_cache`・`cos_chat_launch`・`scratch`・`stagnation`・`task_workspaces` の競合 5 件は run の lifecycle から離れた cache・保守の状態で、1〜4 と独立に扱える。continuation-resume と lease-renew は既に共通の実装があり、共有できる範囲が小さい。

本番への移行: 無し（文書と測定 script だけ）。
