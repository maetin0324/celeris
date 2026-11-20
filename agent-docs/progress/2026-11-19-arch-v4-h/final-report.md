---
title: アーキ v4 H final-report — A の基準からの before・after、検証の結果、未達・要判断、回帰条件
tasks: [01M7MZTKBPZPQE4XN451F8X84F]
status: done
updated: 2026-11-20
completed: 2026-11-19
---
# アーキ v4 H final-report

アーキ v4（依頼キー celeris-architecture-scalability-v4-20261117）の H（task 01M7MZTKBPZPQE4XN451F8X84F）のまとめ。
before は A の基準 main `bc665168c9caf1898adc4c611404bbee056daf9a`（以下 B0。[A の進捗](../2026-11-17-arch-v4-a.md)）、after は H の measure 段が数えた
`git merge-base HEAD main`（件名 `integrate wu/close (phase close)`。アーキ v4 G までが入った main）。
この file のために新しく測ったものは無い。数は measure 段と verify 段の md・tsv（[struct-deps](struct-deps.md)・[struct-store](struct-store.md)・
[loc-cost](loc-cost.md)・[review-prod](review-prod.md)・[gate](gate.md)・[prompt-audit](prompt-audit.md)・[safety](safety.md)）から写した。
replan（人の決定 unmet b。子 task R1〜R6 と leaf R7a・R7b）の後の値は「replan の後」の節にまとめた（[replan/](replan/) の md・tsv と各子の進捗の写し）。
(1)〜(6) の節の after は H の measure の値のままで、結論・未達・要判断・回帰条件・リリースゲートの節は replan の後の値で書き直した。

表の読み方:
- (1)〜(6) の節には表が 2 つある。「A の項目」の表は、A の監査根拠の 14 項目（A の表の行を項目に分けた順。A が「#13 修正の波及」と呼ぶ番号）と、
  A の共通受入条件 (1)〜(6) の before を条件ごとに振り分け、before・after・差・判定を書いたもの。
  「tsv の行」の表は [struct-deps.tsv](struct-deps.tsv)・[struct-store.tsv](struct-store.tsv)・[loc-cost.tsv](loc-cost.tsv) の全行で、
  metric・scope・checker・before・after は tsv の値をそのまま写した（桁区切りなし。同じ metric の別 scope・別 checker は別の行）。
- 14 項目の振り分け: (1) に #2 Cargo の依存の辺と crate 循環・#3 module 循環・#4 task-core の SCC・#5 task-api の SCC・#6〜#8 health の既存負債・#14 `#[path]`、
  (2) に #9 dispatcher の参照、(3) に #10〜#12 SqliteStore、(4) に #13 修正の波及、(6) に #1 LoC。#6〜#8 は A の (1)〜(6) のどれにも名前が無く、
  struct-deps が循環と同じ検査器で数えたので (1) に置いた。(5) に当たる項目は 14 項目に無く、A の代表シナリオの時間と review の実績を before にした。
- checker の欄: `b0` は B0 の `scripts/dev/repo-health`、`head` は after の tree の repo-health（アーキ v4 B の変更入り。`super::` の path と `#[path]` の論理 module を
  辺の単位に使う）、`a-c3`・`a-c4`・`a-c6`・`a-c7` は A の baseline.md §7 の snippet、`e-e1` は E の E1（改行をまたぐ参照も数える）、`d-narrow`・`d-wide` は
  D の ADR の付記の 2 本の command、`h-awk` は H の補助の awk、`ssr@bc665168` は B0 に固定した source-size-report、`-` は検査器の版によらない値
  （git grep・cargo metadata・G の値の写し）。
- 成果の数え方: 移動・分割・抽象の追加だけの変更と、分母の増加は成果に数えない。差がその種類のときは判定か理由の欄にそう書いた。

## 結論

人が決めること・人が行うこと（H は決めていない。H は起票もしていない）:

1. **未達の扱い（決定 final-unmet）**。人は計画 v2 の承認で、決定 final-unmet に「残りを見てから決める」（c）と答えた。R1〜R7 の後も、
   共通受入条件 (1)・(2)・(4) は未達のまま、(3) は一部だけ減った（handlers ⇄ cos の行と task-api の SCC の組。単位の循環は残る）、(6) は増えた、
   (5) は health の S1・S2 がどちらも A の +10 % を超え、review の時間と費用と修正の波及（#13）は比べられない。
   これらを未達として記録して H を閉じるか、次の task を求めるかを人が選ぶ。一覧は「未達・要判断」の節（項目ごとに replan の後の値・理由・次の候補）。
2. **提案の起票の要否**。「提案」の節の 17 件のうち R1〜R7 は 14 以外を扱った（「replan の後」の節）。残る提案 14 と、
   「未達・要判断」の次の候補の欄に挙げた R1〜R6 の進捗の提案を起票するかを人が選ぶ。
3. **2026-11-25T06:00Z の後の review の比較を誰が行うか**（[review-prod.md](review-prod.md) §7.1 の手順）。この file の材料（H と R1〜R7 の進捗）には担当の決定が書かれていない。
4. **H を含む release**。R1〜R6 は `crates/` と `scripts/` を変えたが、H の branch は main に入っておらず、R1〜R7 を含む release の gate は未実施。
   H の統合の後に人が `release.sh` を回す（「リリースゲート」の節）。

数値の要点（B0 → H の measure → replan の後。replan の後の値は「replan の後」の節の表、H の measure の値は (1)〜(6) の節の写し。この file のために測り直していない）:

- (1) 未達のまま: module の循環 17 → 17 → 17、task-core の SCC 55 単位・210 組と task-api の SCC 49 単位は 3 時点で同じ。
  task-api の SCC の組は 261 → 261 → 228（−33。R2 −31・R5 −2）、行の辺（b0）は 628 → 618 → 535。task-core の行の辺は 617 → 621 → 627（R4 の +6）。
  health の既存負債 94・210・31 は同じ（finding の scope の集合も同じ）。
- (2) 未達のまま: `rusqlite` を書く production file 74 → 75 → 74、SQL 文字列の file 50 → 50 → 49（R4 の −1 はどちらも task-core への移動で、成果に数えない）。
  R7b の補助の指標 `conn_open_files` は 48 → 47 → 45（R4 の −2 は task-api の接続の撤去で、成果に数える）。
  dispatcher で self.store を書く file 44 → 38 → 38・self.config 35 → 31 → 31（E の移動。成果に数えない）。出現数は self.store 633 → 633 → 630・
  self.config 218 → 220 → 212（R3 の同じ読みの 1 回化）。分母 69 は同じ。
- (3) 一部: handlers → cos 22 → 19 → 14 行、cos → handlers（A の C5 の式）52 → 50 → 29 行、`super::` 4 → 4 → 0 行。両方向が残るので単位の循環は解けていない。
  `SqliteStore` の `pub` method 633 → 631 → 631・private 42 → 41 → 41、`Fn(&Connection)` の closure（task-ops）5 → 4 → 4、task-api の `&Connection`・`&Transaction` の引数 14 → 15 → 15。
- (4) 未達のまま: S1 で変える file・単位 3 → 4 → 4、S2 は 4 file・2 単位のまま、test-affected の対象も同じ。S1 を直接 import する単位（b0）は 92 → 92 → 76（R2）に減ったが、
  推移的に import する単位は 369 → 369 → 372 に増えた。修正の波及（#13）は R7b の ADR D5 の下限（no-merges 30 件）に届かず未達（B0 の後の窓は no-merges 1 件。窓は 2026-12-01T00:00:00Z に閉じる）。
- (5) health:S1 は 86.56 → 93.58 → 98.74 s（A の +14.1 %）、health:S2 は 81.92 → 96.09 → 100.68 s（A の +22.9 %）で、両方とも A の +10 % を超えた。
  H の値からの増え（+5.5 %・+4.8 %）がコード由来か時間帯の host の差かは切り分けていない。check warm S1・S2 は 43.08・14.76 s で A の +10 % 以内。
  ほかの 6 key は G の値の写しで、R1〜R6 の後の tree では測っていない。review の時間と費用は窓の長さが違い比べられない（2026-11-25T06:00Z の後の窓で比べる）。
- (6) 増えた: production 375099 → 375921 → 376018 行（B0 から +919、すべて Rust）、試験 476771 → 481514 → 484793 行、own_total_with_tests 1025677 → 1031506 → 1034966 行。
  依頼は一律の純減を求めていない。replan の +97 は R1 +26・R2 +63・R3 −7・R4 +50・R5 −35（各子の進捗の差の写し。和が +97 に一致する）。
- 判定経路: [replan/gate.tsv](replan/gate.tsv) の 17 行すべて期待どおり（C の 6 ケース、prompt の試験 10 件、repo-health の試験 126 件、probe a は exit 1・probe b は exit 0、
  D・E・F の反例試験 81 件、clippy・fmt）。本番でも C を含む release の昇格の後の設計 review の要求 7 件（3 task・$14.33）は、すべて C の形の理由で始まった（H の measure の値）。
- 実 prompt: H の監査では worker 157 run・reviewer 20 run の objective の末尾と受入条件の cut が 0、planner の 24 run の受入条件が 0/126 件だった。
  R1 が planner の 4 builder に受入条件の節を足し、試験 7 件で planner・設計 review・final review の prompt に 3,000 文字の objective の末尾と受入条件が載ることを守った。
  本番の実 prompt での再監査は、R1 を含む release が無いので未実施。
- 安全不変条件: H の safety.tsv の 11 行は pass（延べ 563 件、fail 0）。replan の後は D・E・F の反例試験 81 件と、R2・R4・R5 の不変条件と契約の試験（104・316・39 件ほか）が pass。
  userns の env が無いと検査の前に return する 9 件は実質未検査のまま。
- `Applied` の形: 広い数えの file は 19 → 18 → 15（R5）で、回帰条件（18 以下）を満たす。
- リリースゲート: R1〜R7 を含む H の release の gate は未実施。いまの current の release（B〜G を含む）は gate ok・verify ok で昇格済み。

成果に数えたもの: H の measure の時点では、消した method（`pub` −2・private −1）と closure（−1）、`Applied` の旧い形を消して org_write の形にした関数（−3）。
replan で加わったもの: R4 が消した task-api の自前の接続（`conn_open_files` −2）と `ChatState.attachment_db_path` の欄、R5 が消した `Applied` の形の 6 関数（`applied_wide` −3）、
R3 が消した self.store −3・self.config −8 の出現（参照を持つ file の数は減っていない）、R2 が消した `super::` の 4 行と、定義を SCC の外へ移して消えた task-api の SCC の組 −31。
R2 の行の辺の減り 84 のうち 90 行は移動で、SCC の単位と循環の数は減っていない。成果に数えないもの: dispatcher の file 数の減り（E の移動）、R4 の `rusqlite`・SQL 文字列の −1（移動）、
増えた項目（`rusqlite` の file の H 時点の +1、S1 の file・単位 +1、`&Transaction` +1 は D の共有 use-case という抽象の追加）。分母（dispatcher の 69・76）は増えていない。

## replan の後

人の決定 unmet b（未達を直す子 task を起こす）に従い、H は R1〜R7（子 task の R1〜R6 と、H の leaf の R7a・R7b）で未達の一部を直した。
続いて R8（leaf の rc-deps・rc-store・rc-loc・rc-gate）が、R1〜R7 の後の H の branch を A の規則で数え直し、判定経路を再実行した。
この節はその結果をまとめる。この節のために新しく測ったものは無い（数は各子の進捗・metrics.tsv と [replan/](replan/) の md・tsv、[c10-window.md](c10-window.md) の写し）。

基点（[replan/struct-store.md](replan/struct-store.md) の「基点」）:

- B0: main `bc665168c9caf1898adc4c611404bbee056daf9a`（A の基準）。
- before（H の measure）: main の commit「integrate wu/close (phase close)」。この file の (1)〜(6) の after と同じで、いまも `git merge-base HEAD main`。
- after（replan の後）: R8 が数えた時点の H の branch の HEAD（R1〜R6 の統合と R7a・R7b の統合の後。R8 の commit は docs だけ）。
- H の branch は main に入っていない。main の先端は before と同じ commit で、before の後の main の commit は無い。

### R1〜R7 の変更・測定・ADR・試験

出所は各子の進捗（[R1](../2026-11-19-arch-v4-h-r1.md)・[R2](../2026-11-19-arch-v4-h-r2.md)・[R3](../2026-11-19-arch-v4-h-r3.md)・[R4](../2026-11-19-arch-v4-h-r4.md)・
[R5](../2026-11-19-arch-v4-h-r5.md)・[R6](../2026-11-19-arch-v4-h-r6.md)）と各子の metrics.tsv、R7a・R7b の commit と [c10-window.md](c10-window.md)。
「提案」の列は、この file の「提案」の節の番号。測定値の before は各子の開始時の `git merge-base HEAD main`（H の measure と同じ commit）か、R5 のように直前の統合の rev。
試験の結果は各子が提出時の HEAD で実行したもの。

| R | task・leaf（提案） | 消したもの・変えたもの | 測定値（before → after） | ADR | 試験の結果 |
|---|---|---|---|---|---|
| R1 | 子 task 01M7T5KM1VYFDW1RD0B90YHAZ6（提案 6・8） | planner の 4 builder（`build_execution_plan_prompt` の木と木でない形・`build_plan_prompt`・`build_project_plan_prompt`・`build_project_replan_prompt`）の prompt に task の受入条件の節（`## Task acceptance criteria (checked by the final review)`）を足した。消したもの: `build_review_prompt` と `build_execute_prompt` の受入条件の同じ loop（helper `acceptance_lines` に置き換え、出力は同じ byte） | 新しい試験 0 → 7 件（`planner_prompt_acceptance_` 3・`prompt_full_objective_` 4）。planner の 4 件は修正前の code で落ち、review の 3 件は objective を 1,500 文字に切る一時の変更で落ちた。production Rust は `prompt.rs` の +32・−6 行 | [2026-11-19-planner-reviewer-prompt-coverage](../../adr/2026-11-19-planner-reviewer-prompt-coverage.md)（D1〜D4） | task-worker の filter 14 passed、task-dispatch の filter 9 passed（`boundary_case_` 6・`prompt_full_objective_` 3）、test-affected 6130 passed・21 skipped、clippy・fmt exit 0 |
| R2 | 子 task 01M7T5KM3FW3430R5DT4QG5QKV（提案 1・2） | cos → handlers の `super::` 4 行（`cos/ops/admin.rs` の accounts）を消し、account の書き込みを `account_write.rs`、HTTP の道具を `http_io.rs`（どちらも SCC の外）へ移した。`handlers/accounts.rs` −546・`handlers.rs` −95・`cos/ops/admin.rs` −18・`cos/operations.rs` −9 行 | task-api の SCC の組 261 → 230、cos → handlers（A の C5 の式）50 → 29 行、handlers → cos 19 → 14 行、SCC の行の辺 622 → 538（head）。行の辺の減り 84 は消えた 91（移動 90・削除 1）− 増えた 7。SCC の単位は 49 → 49。production +63 行 | [2026-11-19-cos-handlers-remaining-edges](../../adr/2026-11-19-cos-handlers-remaining-edges.md)（付記: 高 risk の 5 file の変更と不変条件） | task-api の不変条件と account の契約の nextest 104 passed（契約試験 6 件を足した）、boundary の不変条件の nextest 222 passed・2 skipped |
| R3 | 子 task 01M7THYAPKESR1BG6BQZPJ749P（提案 3） | 同じ流れの同じ読みを 1 回にまとめた（ADR D1 (b)）: `tree_units.rs` の self.store −1、`cos_chat/root_done.rs` の self.store −2、`cos_chat/triage.rs` の self.config −3、`cos_chat/fallback.rs` の self.config −5。`review_verdict.rs`・`work_units.rs` は ADR D6 の決定で実装していない | request/C3 の self.store 633 → 630・self.config 220 → 212（4 scope とも −3・−8）。参照を持つ file 38・31 と分母 69 は同じ。crate 全体も store 683 → 680・config 240 → 232（他の file へ移しただけの減りは無い）。production −7 行 | [2026-11-19-dispatcher-narrow-ops-round2](../../adr/2026-11-19-dispatcher-narrow-ops-round2.md)（末尾に実測の付記） | 試験は足していない。反例試験と構造の guard 144 本 pass（filter 2 本で 120 passed・78 passed）、`measure.sh` の verify・gain・shrink exit 0 |
| R4 | 子 task 01M7TWBX8YYDKHV6C2JWDJS1VR（提案 4・5・12） | task-api の `chat/attachments.rs` の `Connection::open` 3 か所、`cos/triage_view.rs` の `Connection::open` と `busy_timeout`、`cos/inbox.rs` の `triage_db` と 503 `cos_inbox_unavailable` の分岐、`ChatState.attachment_db_path` の欄を消した（同じ DB を指す値は `Inner.db_path` の 1 つ）。読み取りは task-core の `chat/attachments.rs` と新しい `cos_inbox.rs` の、`with_read_conn` を使う自由関数へ移した | `conn_open_files` 47 → 45（接続の撤去。成果に数える）、`rusqlite` の file 75 → 74・SQL 文字列の file 50 → 49（task-core への移動。成果に数えない）。production Rust +302・−252 行（+50） | [2026-11-19-chat-attachment-store-connection](../../adr/2026-11-19-chat-attachment-store-connection.md)（付記 3 件） | 反例試験 9 件を足した（`attach_conn_` 3・`triage_conn_` 4・`attachment_flag_` 2）。回帰条件の filter 17 件 pass、boundary の不変条件の nextest 316 passed・2 skipped |
| R5 | 子 task 01M7VJY8NZ5G84ACBNQTPVV3R4（提案 15〜17） | `knowledge.rs` の `accept_op`・`reject_op`・`put_page_op`、`lifecycle.rs` の `task_pause_op`、`skills.rs` の `put_skill_op`・`delete_skill_op` の 6 関数と、その `Option<&OperationAudit>` の引数・`Applied` の戻り値・`audit` の分岐・HTTP の `.direct()` 6 か所・CoS の `audited(…)` 6 か所・3 file の `use` を消した。代わりに plan と効果の関数（`plan_accept`・`accept_effect` ほか計 11）を置いた | `applied_wide` 18 → 15、`applied_narrow` 14 → 11、`applied_functions` 55 → 49（足された file・関数 0）。task-api の SCC の組 230 → 228（`lifecycle → cos`・`skills → cos`）。task-api の src は +328・−363 行 | [2026-11-20-applied-shape-migration](../../adr/2026-11-20-applied-shape-migration.md)（付記: 実装で分かった違い）。org-write の ADR の付記の表も今の関数に合わせた | `applied_contract_` 12 本を足して pass、task-api の filter 39 passed、test-affected の nextest 8520 passed・22 skipped、web 705 passed |
| R6 | 子 task 01M7VJY8Q1MMAH87CSPQZWC7VY（提案 9・10） | `scripts/dev/test-parallel.sh` の nextest の Summary 行の解析を直した（注記付きの `8525 passed (3 slow)` の組を取れず、nextest の通過数を 0 にしていた）。gate.json の `cargo_test` に `nextest_passed`・`nextest_skipped`・`doc_passed`・`doc_ignored` を足した。`scripts/selfdeploy/lib.sh` の `sd_release_record` が gate・verify・promoted の要約を `$SD_LOGS/release-records.jsonl` に 1 行ずつ追記する（release dir の file が正本で、判断には読まない） | 広げた試験は base の test-parallel.sh で exit 1、HEAD で exit 0。tooling +84 行（`loc_tooling`） | [2026-11-20-release-record-summary-log](../../adr/2026-11-20-release-record-summary-log.md)（D1〜D4） | `release_parallel_test_gate.sh`・`release_record_summary_survives_prune.sh` が exit 0。release・verify・promote を起こす試験 22 本、それ以外の selfdeploy の試験 12 本、prune・promote の 10 本がどれも exit 0（一時の log dir で実行し、本番の log に書いていない） |
| R7a | H の leaf r7-map（提案 11） | `docs/architecture-map.md` の健全性の review の行に、`#[path]` で読む `health_review/check_item.rs`・`review_limits.rs`・`inventory.rs` の 3 file を足した（文書だけ。消したものは無い） | `path_attr_files` は 9・15 のまま（宣言は変えていない。R8 の rc-deps が宣言の一覧が H の after と同じことを確かめた） | 無し（文書の修正） | 試験の対象は無い（文書だけ） |
| R7b | H の leaf r7-rules（提案 7・12・13） | 計測の規則の付記を書いた: D1 `conn_open_files`（接続を開く行を持つ file の数）、D2 A の C4 と並べて checker で区別する、D3 過去の WorkUnit の run は開始時の計画の版の `units[]` で監査する、D4 run ごとの spec を残す仕組みは足さない（人が決める）、D5 C10 は窓の no-merges が 30 件以上のときだけ A と比べる。C10 を B0 の後の窓に当てた（[c10-window.md](c10-window.md)） | `conn_open_files` は B0 48・H の measure 47。C10 は B0 の後の窓（`bc665168..main`）で no-merges 1 件・with-merges 9 件で、未達（窓の件数が不足）。窓は 2026-12-01T00:00:00Z に閉じる | [2026-11-19-arch-v4-measure-rules-addendum](../../adr/2026-11-19-arch-v4-measure-rules-addendum.md)（D1〜D6） | A の窓の再現は A の値と全列で一致（no-merges 492・194、with-merges 653・246） |

R1〜R7 が扱わなかった提案は 14（2026-11-25T06:00Z の後の review の比較）だけである。R3 は提案 3 のうち `review_verdict.rs`・`work_units.rs` を実装せず、
R7b は提案 7 のうち run ごとの spec を残すかを人に残した（ADR D4）。

### A の規則で数え直した値（replan/ の 3 つの tsv）

[replan/struct-deps.tsv](replan/struct-deps.tsv)・[replan/struct-store.tsv](replan/struct-store.tsv)・[replan/loc-cost.tsv](replan/loc-cost.tsv) の全行。
数え方と再現コマンドは [replan/struct-deps.md](replan/struct-deps.md)・[replan/struct-store.md](replan/struct-store.md)・[replan/loc-cost.md](replan/loc-cost.md)。

- B0 の列は、H の measure の tsv（[struct-deps.tsv](struct-deps.tsv)・[struct-store.tsv](struct-store.tsv)・[loc-cost.tsv](loc-cost.tsv)）の同じ metric・scope・checker の
  before（B0 で数えた値）。`conn_open_files` は H の measure の tsv に無いので、B0 の 48 は R7b の ADR D2 の値。
  loc-cost の C10 の行（`fix_*`）の B0 の列は A の窓（2026-10-28〜B0）の値、時間の行（`check_*`・`test_*`・`health_*`）は A の中央値。
- before は replan の tsv の before（H の measure の after）。全行で H の measure の tsv の after と一致する。after は replan の tsv の after。
- 差は after − before と after − B0。注は replan の tsv の note の写し。
- 時間の 10 行のうち 6 行（`check_cold`・`test_build_*`・`test_affected_*`）は G の値の写しで、R1〜R6 の後の tree では測っていない（note のとおり）。

#### replan/struct-deps.tsv（29 行）

| metric | scope | checker | B0 | before（H の measure） | after（replan の後） | 差（after − before） | 差（after − B0） | 注（replan の tsv の note） |
|---|---|---|---|---|---|---|---|---|
| `cargo_edges` | workspace の Cargo 宣言（normal+dev、cargo metadata） | - | 49 | 49 | 49 | 0 | 0 | normal 35・dev 14 が同じ。H の after..HEAD で Cargo.toml・Cargo.lock・crates/*/Cargo.toml の差分 0 |
| `crate_cycles` | workspace の通常依存の crate の SCC | - | 0 | 0 | 0 | 0 | 0 | cargo metadata の normal の辺の強連結成分（2 crate 以上） |
| `module_cycles` | repo-health deps.cycle（C2 の SCC の数） | b0 | 17 | 17 | 17 | 0 | 0 | H と同じく profile は B0 から読む（b0 は HEAD の profile も読めない: profile_invalid） |
| `module_cycles` | repo-health deps.cycle（C1 の findings） | head | 17 | 17 | 17 | 0 | 0 | C2 の SCC の数も 17・17。finding の scope の集合も H の after と同じ |
| `tc_scc_units` | task-core の SCC（C2） | b0 | 55 | 55 | 55 | 0 | 0 |  |
| `tc_scc_units` | task-core の SCC（C2） | head | 55 | 55 | 55 | 0 | 0 |  |
| `tc_scc_pairs` | task-core の SCC の単位の組（C2） | b0 | 210 | 210 | 210 | 0 | 0 | 組の集合も同じ |
| `tc_scc_pairs` | task-core の SCC の単位の組（C2） | head | 210 | 210 | 210 | 0 | 0 | 組の集合も同じ |
| `tc_scc_lines` | task-core の SCC の行の辺（C2） | b0 | 617 | 621 | 627 | +6 | +10 | +6 は task-core/src/chat/attachments.rs → store（R4。接続を store へ移した） |
| `tc_scc_lines` | task-core の SCC の行の辺（C2） | head | 617 | 621 | 627 | +6 | +10 | b0 と同じ差（R4 の +6） |
| `ta_scc_units` | task-api の SCC（C2） | b0 | 49 | 49 | 49 | 0 | 0 |  |
| `ta_scc_units` | task-api の SCC（C2） | head | 49 | 49 | 49 | 0 | 0 |  |
| `ta_scc_pairs` | task-api の SCC の単位の組（C2） | b0 | 261 | 261 | 228 | −33 | −33 | −33（消えた 33・増えた 0）: R2 −31（<module> → handlers 29、notify → cos・releases → cos 2）、R5 −2（lifecycle → cos・skills → cos） |
| `ta_scc_pairs` | task-api の SCC の単位の組（C2） | head | 261 | 261 | 228 | −33 | −33 | b0 と同じ組の集合の差 |
| `ta_scc_lines` | task-api の SCC の行の辺（C2） | b0 | 628 | 618 | 535 | −83 | −93 | −83: R2 618 → 538（−80）、R4 −1（schema.rs → cos）、R5 −2（admin.rs → skills −2・knowledge.rs → cos −4・lifecycle.rs → cos −1・skills.rs → cos −1、decisions.rs → knowledge +6） |
| `ta_scc_lines` | task-api の SCC の行の辺（C2） | head | 632 | 622 | 535 | −87 | −97 | −87: R2 622 → 538（−84。super:: の 4 行を含む）、R4 −1、R5 −2。super:: の行が 0 になったので b0 と同じ値 |
| `handlers_to_cos_lines` | git grep crate::cos::（C5、handlers/ と handlers.rs） | - | 22 | 19 | 14 | −5 | −8 | 12 file → 9 file。−5 は R2 の Effect・effect_result の移動（accounts.rs 2・reload.rs 1・system.rs 2） |
| `handlers_to_cos_lines` | repo-health の辺（C2 の task-api の SCC、handlers→cos） | b0 | 22 | 19 | 14 | −5 | −8 | 12 file → 9 file |
| `handlers_to_cos_lines` | repo-health の辺（C2 の task-api の SCC、handlers→cos） | head | 22 | 19 | 14 | −5 | −8 | 12 file → 9 file |
| `cos_to_handlers_lines` | git grep crate::handlers（C5、cos/） | - | 48 | 46 | 29 | −17 | −19 | 9 file → 3 file（cos/ops/admin.rs 4・projects.rs 17・tasks.rs 8）。−17 は R2 |
| `cos_to_handlers_lines` | git grep (crate|super(::super)*)::handlers（A の C5 の式） | - | 52 | 50 | 29 | −21 | −23 | crate:: 29・super:: 0。−21 は R2 |
| `cos_to_handlers_lines` | repo-health の辺（C2 の task-api の SCC、cos→handlers） | b0 | 48 | 46 | 29 | −17 | −19 | 9 file → 3 file |
| `cos_to_handlers_lines` | repo-health の辺（C2 の task-api の SCC、cos→handlers） | head | 52 | 50 | 29 | −21 | −23 | super:: の行が 0 になったので b0 と同じ値 |
| `cos_to_handlers_super_lines` | git grep super(::super)*::handlers（C5、cos/） | - | 4 | 4 | 0 | −4 | −4 | cos/ops/admin.rs の accounts の 4 行が R2 で消えた |
| `responsibility_growth` | repo-health size.responsibility-growth（C1） | head | 94 | 94 | 94 | 0 | 0 | finding の scope の集合も H の after と同じ（evidence の行番号だけが 2 件変わった: task-api の state・cos） |
| `dup_contract` | repo-health dup.contract（C1） | head | 210 | 210 | 210 | 0 | 0 | finding の scope の集合も H の after と同じ |
| `owner_mismatch` | repo-health arch-map.wrong_owner（C1） | head | 31 | 31 | 31 | 0 | 0 | finding の scope の集合も H の after と同じ |
| `path_attr_files` | task-dispatch の production の #[path]（A §3.2） | - | 9 | 9 | 9 | 0 | 0 | 宣言の一覧は行番号を除いて H の after と同じ |
| `path_attr_files` | workspace の production の #[path]（単位の食い違い、H の追加） | - | 15 | 15 | 15 | 0 | 0 | 足された #[path] は試験の 1 件（task-worker/src/claude_code/tests.rs → planner_prompt_tests.rs、R1）だけで数えない |

#### replan/struct-store.tsv（46 行）

| metric | scope | checker | B0 | before（H の measure） | after（replan の後） | 差（after − before） | 差（after − B0） | 注（replan の tsv の note） |
|---|---|---|---|---|---|---|---|---|
| `rusqlite_prod_files` | prod-excl-task-core-store | a-c4 | 74 | 75 | 74 | −1 | 0 | −1 = 外れた 2（R4: crates/task-api/src/chat/attachments.rs・crates/task-api/src/cos/triage_view.rs）− 入った 1（R4 triage: crates/task-core/src/cos_inbox.rs。受信箱の読み取りを SqliteStore の with_read_conn で行う task-core の自由関数。接続は開かない）。crate 別 after task-core 43 task-api 15 task-ops 8 celeris 4 ほか 4 |
| `sql_string_prod_files` | prod-excl-task-core-store | a-c4 | 50 | 50 | 49 | −1 | −1 | −1。集合の差は rusqlite と同じ（R4: 外れた chat/attachments.rs・cos/triage_view.rs、入った task-core/src/cos_inbox.rs） |
| `conn_open_files` | prod-excl-task-core-store | r7-d1 | 48 | 47 | 45 | −2 | −3 | R7 の ADR 2026-11-19-arch-v4-measure-rules-addendum D1 の補助の指標（A の C4 と別の数え方。checker で区別する D2）。before は H の base（H の measure の after と同じ commit）で数えた値。B0 は 48。−2 は R4（chat/attachments.rs の Connection::open 3 行・cos/triage_view.rs の Connection::open と busy_timeout）。入った file 0 |
| `conn_arg_task_core` | crates/task-core/occ | a-c6 | 320 | 320 | 320 | 0 | 0 | 同じ |
| `conn_arg_task_core` | crates/task-core/files | a-c6 | 76 | 76 | 76 | 0 | 0 | 同じ |
| `conn_arg_task_core_store` | crates/task-core/src/store/occ | a-c6 | 205 | 205 | 205 | 0 | 0 | 同じ |
| `conn_arg_task_core_store` | crates/task-core/src/store/files | a-c6 | 49 | 49 | 49 | 0 | 0 | 同じ |
| `conn_arg_task_api` | crates/task-api/occ | a-c6 | 14 | 15 | 15 | 0 | +1 | 数は同じ（行番号の移動だけ: handlers/authority.rs・project_controller.rs・roadmap.rs・tasks.rs・knowledge.rs・repos.rs） |
| `conn_arg_task_api` | crates/task-api/files | a-c6 | 9 | 10 | 10 | 0 | +1 | 同じ |
| `conn_arg_task_ops` | crates/task-ops/occ | a-c6 | 7 | 7 | 7 | 0 | 0 | 同じ |
| `conn_arg_task_ops` | crates/task-ops/files | a-c6 | 4 | 4 | 4 | 0 | 0 | 同じ |
| `conn_fn_closure` | crates/task-core | a-c6 | 13 | 13 | 13 | 0 | 0 | 同じ |
| `conn_fn_closure` | crates/task-api | a-c6 | 4 | 4 | 4 | 0 | 0 | 補助。数は同じ（cos/operations.rs・repos.rs の行番号の移動だけ） |
| `conn_fn_closure` | crates/task-ops | a-c6 | 5 | 4 | 4 | 0 | −1 | 同じ |
| `sqlitestore_impl` | plain | a-c6 | 102 | 102 | 102 | 0 | 0 | 同じ |
| `sqlitestore_impl` | path | a-c6 | 105 | 105 | 105 | 0 | 0 | 同じ |
| `sqlitestore_impl` | trait | a-c6 | 6 | 6 | 6 | 0 | 0 | 同じ |
| `sqlitestore_pub` | plain | a-c6 | 633 | 631 | 631 | 0 | −2 | 同じ |
| `sqlitestore_pub` | path | a-c6 | 636 | 634 | 634 | 0 | −2 | 同じ |
| `sqlitestore_pub_crate` | plain | a-c6 | 67 | 67 | 67 | 0 | 0 | 同じ |
| `sqlitestore_pub_crate` | path | a-c6 | 67 | 67 | 67 | 0 | 0 | 同じ |
| `sqlitestore_private` | plain | a-c6 | 42 | 41 | 41 | 0 | −1 | 同じ |
| `sqlitestore_rdeps` | normal-transitive | a-c7 | 11 | 11 | 11 | 0 | 0 | 同じ（HEAD の作業ツリーで cargo metadata --offline。宣言 normal 35・dev 14 も同じ。H の base から HEAD まで Cargo.toml・Cargo.lock の差分 0） |
| `dispatcher_files` | request/C3 | a-c3 | 69 | 69 | 69 | 0 | 0 | 同じ |
| `dispatcher_files` | request/E1 | e-e1 | 69 | 69 | 69 | 0 | 0 | 同じ |
| `dispatcher_files` | logical/C3 | a-c3 | 76 | 76 | 76 | 0 | 0 | 分母。HEAD で #[path] を引き直して同じ 8 file と lib.rs:47（project_decider_ts.rs） |
| `dispatcher_files` | logical/E1 | e-e1 | 76 | 76 | 76 | 0 | 0 | 分母。HEAD で #[path] を引き直して同じ 8 file と lib.rs:47（project_decider_ts.rs） |
| `dispatcher_store_files` | request/C3 | a-c3 | 44 | 38 | 38 | 0 | −6 | 同じ |
| `dispatcher_store_files` | request/E1 | e-e1 | 46 | 40 | 40 | 0 | −6 | 同じ |
| `dispatcher_store_files` | logical/C3 | a-c3 | 48 | 42 | 42 | 0 | −6 | 同じ |
| `dispatcher_store_files` | logical/E1 | e-e1 | 50 | 44 | 44 | 0 | −6 | 同じ |
| `dispatcher_store_hits` | request/C3 | a-c3 | 633 | 633 | 630 | −3 | −3 | −3（R3: tree_units.rs −1・cos_chat/root_done.rs −2。既存の owner の狭い操作に替えた）。file 数は同じ |
| `dispatcher_store_hits` | request/E1 | e-e1 | 759 | 759 | 756 | −3 | −3 | −3（R3） |
| `dispatcher_store_hits` | logical/C3 | a-c3 | 661 | 661 | 658 | −3 | −3 | −3（R3） |
| `dispatcher_store_hits` | logical/E1 | e-e1 | 791 | 791 | 788 | −3 | −3 | −3（R3） |
| `dispatcher_config_files` | request/C3 | a-c3 | 35 | 31 | 31 | 0 | −4 | 同じ |
| `dispatcher_config_files` | request/E1 | e-e1 | 35 | 31 | 31 | 0 | −4 | 同じ |
| `dispatcher_config_files` | logical/C3 | a-c3 | 39 | 35 | 35 | 0 | −4 | 同じ |
| `dispatcher_config_files` | logical/E1 | e-e1 | 39 | 35 | 35 | 0 | −4 | 同じ |
| `dispatcher_config_hits` | request/C3 | a-c3 | 218 | 220 | 212 | −8 | −6 | −8（R3: cos_chat/triage.rs −3・cos_chat/fallback.rs −5）。file 数は同じ |
| `dispatcher_config_hits` | request/E1 | e-e1 | 227 | 227 | 219 | −8 | −8 | −8（R3） |
| `dispatcher_config_hits` | logical/C3 | a-c3 | 224 | 226 | 218 | −8 | −6 | −8（R3） |
| `dispatcher_config_hits` | logical/E1 | e-e1 | 233 | 233 | 225 | −8 | −8 | −8（R3） |
| `applied_narrow` | crates/task-api/src | d-narrow | 14 | 14 | 11 | −3 | −3 | −3 = knowledge.rs・lifecycle.rs・skills.rs（R5）。入った file 0 |
| `applied_wide` | crates/task-api/src | d-wide | 19 | 18 | 15 | −3 | −4 | −3 = knowledge.rs・lifecycle.rs・skills.rs（R5）。入った file 0。回帰条件 18 以下を満たす。型の定義側 cos/operations.rs を除く操作の側は 14 file |
| `applied_functions` | crates/task-api/src | h-awk | 58 | 55 | 49 | −6 | −9 | 補助（H の awk）。−6 = knowledge.rs の accept_op・reject_op・put_page_op、lifecycle.rs の task_pause_op、skills.rs の put_skill_op・delete_skill_op（R5）。入った関数 0。操作の側は 48 |

#### replan/loc-cost.tsv（36 行）

| metric | scope | checker | B0 | before（H の measure） | after（replan の後） | 差（after − before） | 差（after − B0） | 注（replan の tsv の note） |
|---|---|---|---|---|---|---|---|---|
| `loc_prod_all` | production 全言語（views.product_only.lines） | ssr@bc665168 | 375099 | 375921 | 376018 | +97 | +919 | +97。HEAD に B0 の source-size-report を当てた。before は H の measure の after（1680de5c に当て直して同じ値）。差はすべて Rust |
| `loc_prod_rust` | production Rust（views.product_only.rust_lines） | ssr@bc665168 | 330408 | 331230 | 331327 | +97 | +919 | +97。ほかの言語の production の差は 0 行 |
| `loc_test` | 試験 = class_totals.external_test + inline_test | ssr@bc665168 | 476771 | 481514 | 484793 | +3279 | +8022 | +3279。external_test 460106→463385（Rust 410773→413950 +3177、shell 8229→8331 +102）、inline_test 21408→21408 |
| `loc_own_total` | own_total_with_tests 全言語（views.own_total_with_tests.lines） | ssr@bc665168 | 1025677 | 1031506 | 1034966 | +3460 | +9289 | +3460。Rust 884431→887705（+3274）。tooling 25517→25601（+84）と fixture shell 747→882（+135）を含む |
| `loc_own_total_rust` | own_total_with_tests Rust（views.own_total_with_tests.rust_lines） | ssr@bc665168 | 879344 | 884431 | 887705 | +3274 | +8361 | +3274（production +97、external_test +3177） |
| `loc_tooling` | tooling（class_totals.tooling） | ssr@bc665168 | 25253 | 25517 | 25601 | +84 | +348 | +84（shell 13041→13125。R6 の selfdeploy の台本） |
| `s1_files` | S1 で変える production file | - | 3 | 4 | 4 | 0 | +1 | G の scenarios.env の S1_FILES の 4 file はすべて HEAD にある。R2 は cos/ops/admin.rs（+75 −87）と handlers/org.rs（+1 −1）を変えたが、同じ意味の変更で変える file は同じ |
| `s1_units` | S1 で変える単位（repo-health の単位 crates/<c>/src/<第 1 要素>） | - | 3 | 4 | 4 | 0 | +1 | task-api の org_write・handlers・cos と task-core の store |
| `s1_crates` | S1 で変える crate | - | 2 | 2 | 2 | 0 | 0 | task-api・task-core |
| `s2_files` | S2 で変える production file | - | 4 | 4 | 4 | 0 | 0 | G の S2_FILES の 4 file。R の変更はこの 4 file を変えていない |
| `s2_units` | S2 で変える単位 | - | 2 | 2 | 2 | 0 | 0 | task-dispatch の dispatcher・run_settlement |
| `s2_crates` | S2 で変える crate | - | 1 | 1 | 1 | 0 | 0 | task-dispatch |
| `s1_direct_importers` | S1 の単位を直接 import する単位（C7b） | b0 | 92 | 92 | 76 | −16 | −16 | −16。b0 の検査器・profile は bc665168・測る tree は HEAD。消えた単位 17 は R2 で HTTP の道具を http_io へ移した task-api の module、足された単位 1 は R4 の task-core の cos_inbox（head の集合の差） |
| `s1_direct_importers` | S1 の単位を直接 import する単位（C7b） | head | 93 | 93 | 77 | −16 | −16 | −16。HEAD の検査器（H の measure から scripts/dev/repo-health の差分 0）。消えた 17・足された 1 は b0 の行と同じ |
| `s1_transitive_importers` | S1 の単位を推移的に import する単位（C7b） | b0 | 369 | 369 | 372 | +3 | +3 | +3。package 12 → 12。足された単位は task-api の http_io・account_write（R2）と task-core の cos_inbox（R4）、消えた単位 0（head の集合の差） |
| `s1_transitive_importers` | S1 の単位を推移的に import する単位（C7b） | head | 365 | 365 | 368 | +3 | +3 | +3。package 12 → 12。足された 3 単位は b0 の行と同じ |
| `s2_direct_importers` | S2 の単位を直接 import する単位（C7b） | b0 | 6 | 6 | 6 | 0 | 0 | head の検査器でも 6 → 6 |
| `s2_transitive_importers` | S2 の単位を推移的に import する単位（C7b） | b0 | 14 | 14 | 14 | 0 | 0 | package 4 → 4。head の検査器でも 14 → 14 |
| `s1_ta_crates` | S1 の test-affected --list の crate（comment の probe） | - | 14 | 14 | 14 | 0 | 0 | ＋ web: typecheck+unit ＋ ts-verify: test-ts-verify。H の measure と同じ集合（comment の probe、4 file・4 行） |
| `s2_ta_crates` | S2 の test-affected --list の crate（comment の probe） | - | 5 | 5 | 5 | 0 | 0 | celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch。H の measure と同じ集合 |
| `fix_commits` | 修正の波及 C10 no-merges（production を変えた first-parent の非 merge commit） | ssr@bc665168 | 194 | 1 | 1 | 0 | −193 | bc665168..HEAD（SINCE 2026-11-17T00:00Z）: commits 12 のうち production を変えたのは H の measure と同じ C の PathClass の 1 件。R の変更はすべて integrate の merge で入るので no-merges に出ない |
| `fix_commits` | 修正の波及 C10 with-merges（merge commit を first-parent の差分で含める） | ssr@bc665168 | 246 | 9 | 14 | +5 | −232 | commits 56 のうち 14 件。H の measure の 9 件に R1〜R5 の integrate の 5 件が足された（R6 は scripts/selfdeploy だけで production でない。R7 は docs だけ）。件数が少なく比較不能 |
| `fix_region_pct` | C10 no-merges の複数 region（crate か top-level の dir）の割合 % | ssr@bc665168 | 46.9 | 0.0 | 0.0 | 0 | −46.9 | 1 件中 0 件（比較不能） |
| `fix_region_pct` | C10 with-merges の複数 region の割合 % | ssr@bc665168 | 49.6 | 55.6 | 42.9 | −12.7 | −6.7 | 14 件中 6 件。R で足されたのは R4（task-api・task-core）の 1 件。子 task の WU 単位の merge を 1 件と数えるので A の窓と比べられない |
| `fix_module_pct` | C10 no-merges の複数 module の割合 % | ssr@bc665168 | 69.1 | 0.0 | 0.0 | 0 | −69.1 | 1 件中 0 件（比較不能） |
| `fix_module_pct` | C10 with-merges の複数 module の割合 % | ssr@bc665168 | 71.1 | 55.6 | 57.1 | +1.5 | −14.0 | 14 件中 8 件。R で足されたのは R2・R4・R5 の 3 件（R1・R3 は 1 module）。比較不能（理由は fix_region_pct と同じ） |
| `check_cold` | cargo check --workspace の cold の wall 中央値 s | - | 53.65 | 53.90 | 53.90 | 0 | +0.25 | H の値（G の中央値）を写した。測り直していない（この WorkUnit の objective が測り直す key を health と check_warm の 4 つに限った）。R1〜R6 で crates/ が変わったので G の tree の値である |
| `check_warm_s1` | cargo check の warm S1（const の probe）の wall 中央値 s | - | 42.47 | 42.64 | 43.08 | +0.44 | +0.61 | 測り直した（HEAD、G の rules.md・build-check.sh の手順、const の probe）。3 回 43.02・43.18・43.08（開始時の 1 分 load 2.68・1.93・1.77、待ち 0 s）、user 中央値 58.76 s。A の +1.4 %、H の値 42.64 の +1.0 % |
| `check_warm_s2` | cargo check の warm S2 の wall 中央値 s | - | 14.67 | 15.56 | 14.76 | −0.80 | +0.09 | 測り直した（同上）。3 回 14.76・15.96・14.56（開始時の 1 分 load 2.05・2.34・2.35、待ち 0 s）、user 中央値 18.90 s。A の +0.6 %、H の値 15.56 の −5.1 % |
| `test_build_cold` | cargo nextest run --workspace --no-run の cold の wall 中央値 s | - | 142.39 | 147.03 | 147.03 | 0 | +4.64 | H の値（G の中央値）を写した。測り直していない（この WorkUnit の objective が測り直す key を health と check_warm の 4 つに限った）。R1〜R6 で crates/ が変わったので G の tree の値である |
| `test_build_warm_s1` | test build の warm S1 の wall 中央値 s | - | 138.69 | 138.10 | 138.10 | 0 | −0.59 | H の値（G の中央値）を写した。測り直していない（この WorkUnit の objective が測り直す key を health と check_warm の 4 つに限った）。R1〜R6 で crates/ が変わったので G の tree の値である |
| `test_build_warm_s2` | test build の warm S2 の wall 中央値 s | - | 77.64 | 81.60 | 81.60 | 0 | +3.96 | H の値（G の中央値）を写した。測り直していない（この WorkUnit の objective が測り直す key を health と check_warm の 4 つに限った）。R1〜R6 で crates/ が変わったので G の tree の値である |
| `health_s1` | repo-health S1（const の probe、--head worktree）の wall 中央値 s | head | 86.56 | 93.58 | 98.74 | +5.16 | +12.18 | 測り直した（HEAD、G の rules.md・c1-health-tsv.sh の手順、--base HEAD --head worktree、const の probe）。3 回 98.74・99.39・96.31（開始時の 1 分 load 1.71・2.07・2.77、待ち 0 s）、user 中央値 74.95 s（H の値の G の 71.19 s の +5.3 %）。A の +14.1 %（印あり）、H の値 93.58 の +5.5 %。fail は size.responsibility-growth（findings 698・new 3・worsened 1） |
| `health_s2` | repo-health S2 の wall 中央値 s | head | 81.92 | 96.09 | 100.68 | +4.59 | +18.76 | 測り直した（同上）。3 回 91.47・109.87・100.68（開始時の 1 分 load 1.73・1.35・2.40、待ち 0 s）、user 中央値 74.77 s（G の 71.86 s の +4.0 %）。A の +22.9 %（印あり）、H の値 96.09 の +4.8 %。fail は size.responsibility-growth・source-size（findings 699・new 4・worsened 2）。before の同じ host の測り直しはしていない |
| `test_affected_s1` | bash scripts/dev/test-affected.sh S1（comment の probe）の wall 中央値 s | - | 893.41 | 906.03 | 906.03 | 0 | +12.62 | H の値（G の中央値）を写した。測り直していない（規則: test-affected の時間は測り直さない）。R1〜R6 で crates/ が変わったので G の tree の値である |
| `test_affected_s2` | test-affected S2 の wall 中央値 s | - | 655.59 | 707.76 | 707.76 | 0 | +52.17 | H の値（G の中央値）を写した。測り直していない（規則: test-affected の時間は測り直さない）。R1〜R6 で crates/ が変わったので G の tree の値である |

### 判定経路と反例試験の再実行（replan/gate.tsv）

R8 の rc-gate が R1〜R7 の後の HEAD で再実行した（[replan/gate.md](replan/gate.md)）。17 行すべて pass（期待どおり）で、fail は 0。
result は「期待どおりの結果だったか」で、検査器の verdict ではない（危険な probe は検査器が fail を出せば pass）。
H の measure の [gate.tsv](gate.tsv)（10 行）と比べると、R1 の試験 2 行（`prompt_full_objective`・`planner_prompt_acceptance`）と、D・E・F の反例試験 3 行、clippy・fmt の 2 行が増えた。

| key | result | note（replan/gate.tsv の写し） |
|---|---|---|
| `case1` | pass | boundary_case_1_authz_one_line_is_high PASS 3.77 秒（認可の 1 行 → High）。check 1 の nextest は 16 tests run: 16 passed、exit 0 |
| `case2` | pass | boundary_case_2_equivalent_plain_change_is_low PASS 3.00 秒（同等の普通の変更 → Low） |
| `case3` | pass | boundary_case_3_move_cannot_escape PASS 2.84 秒（移動・別名・#[path] で class の判定を逃れられない） |
| `case4` | pass | boundary_case_4_worsening_dependency_blocks PASS 2.09 秒（既存の循環への新しい組を health の関門が止める） |
| `case5` | pass | boundary_case_5_policy_self_approval_impossible PASS 1.67 秒（候補自身の policy で class を外せない） |
| `case6` | pass | boundary_case_6_fail_closed PASS 11.01 秒（方針・差分・判定が欠けると合格にしない） |
| `prompt_boundary` | pass | task-worker change_rules::review::tests の prompt_boundary_ 3 件 PASS（worker・planner の規約の節、設計 review の理由の行、final review の finding） |
| `prompt_full_objective` | pass | prompt_full_objective_ 4 件 PASS（R1 が足した試験）: planner の objective と受入条件の末尾（task-worker 1）、設計 review の objective の末尾（task-dispatch 1）、final review の objective の末尾と全条件・reviewer への受け渡し（task-dispatch 2） |
| `planner_prompt_acceptance` | pass | planner_prompt_acceptance_ 3 件 PASS（R1 が足した試験。task-worker の planner_prompt_tests）: tree の planner・flat の planner と plan の種類・全 harness の入力に受入条件が全部載る |
| `repo_health_tests` | pass | python3 -I -m unittest discover -s scripts/tests -p 'test_repo_health_*.py': Ran 126 tests in 52.5s OK、exit 0（scripts/dev/repo-health は merge-base と同じ内容） |
| `probe_scc_edge` | pass | crates/task-api/src/lifecycle.rs に use crate::knowledge::KnowledgeItem; を 1 行足して本物の検査器を走らせた。期待どおり exit 1・verdict=fail（findings 696、new 1、worsened 0、existing 695）。deps.cycle fail（existing 17 + new 1）、symbol crates/task-api/src/lifecycle -> crates/task-api/src/knowledge、fingerprint deps.cycle:a106-75aa-c992-c815（4 桁ずつ区切って書いた）。deps.violation pass（0 件）。probe は直後に git checkout で戻した |
| `probe_plain` | pass | 同じ file に comment を 1 行足した。期待どおり exit 0・verdict=pass（findings 695、new 0、worsened 0）。deps.cycle pass（existing 17）、deps.violation pass（0 件）。probe は直後に戻した。probe なしの run も exit 0・new 0（R1〜R7 の差分は merge-base に対して new の finding を出さない） |
| `org_write_contract` | pass | cargo nextest run -p task-api --test cos_delivery_suite org_write_contract: 6 tests run: 6 passed、exit 0 |
| `tx_boundary_contract` | pass | cargo nextest run -p task-api --test cos_delivery_suite tx_boundary_contract: 8 tests run: 8 passed、exit 0 |
| `dispatcher_cex` | pass | cargo nextest run -p task-dispatch -E 'test(cex_) \| test(write_set_) \| test(store_access_guard) \| test(structure_guard)': 67 tests run: 67 passed、exit 0（cex_ 40 = review_authority 19・lease_restart 12・late_completion 6・same_tick 3、write_set_ 13、structure_guard 14〈store_access_guard 3 を含む〉） |
| `clippy` | pass | cargo clippy --workspace -- -D warnings: exit 0（53 秒） |
| `fmt` | pass | cargo fmt --all -- --check: exit 0、出力 0 行 |

### 修正の波及 C10（c10-window）

R7b が A の C10 の snippet を変えずに B0 の後の窓（`bc665168..main`、SINCE 2026-11-17T00:00:00Z）へ当てた結果（[c10-window.tsv](c10-window.tsv)）。
**未達（窓の件数が不足）**: no-merges の「production を変えた commit」は 1 件で、R7b の ADR D5 の下限 30 件に満たない。with-merges の 9 件は
アーキ v4 の子 task・WorkUnit の変更を束ねた merge で、通常の修正の代わりにならない。A の窓の再現は A の値と全列で一致した。

| metric | scope | checker | A の窓（B0 で閉じる） | B0 の後の窓（`bc665168..main`） | note（c10-window.tsv の写し） |
|---|---|---|---|---|---|
| `fix_commits` | 修正の波及 C10 no-merges（production を変えた first-parent の非 merge commit） | ssr@bc665168 | 194 | 1 | before は A の窓 [2026-10-28T00:00Z, bc665168]（この run の再現も commits 492・194 で A と一致）。after は B0 の後の窓 bc665168..main（SINCE 2026-11-17T00:00Z、main は件名「integrate wu/close (phase close)」の commit）: commits 11 のうち production を変えたのは 1 件。30 件に満たないので未達（窓の件数が不足） |
| `fix_commits` | 修正の波及 C10 with-merges（merge commit を first-parent の差分で含める） | ssr@bc665168 | 246 | 9 | before は A の窓（再現も 653・246 で一致）。after は bc665168..main の commits 40 のうち 9 件。9 件はアーキ v4 C・E・F の変更の merge と、main を E・F の branch に取り込んだ merge で、通常の修正 1 件と粒度が違う |
| `fix_region_pct` | C10 no-merges の複数 region（crate か top-level の dir）の割合 % | ssr@bc665168 | 46.9 | 0.0 | after は 1 件中 0 件。1 件では割合として意味が無い（比較不能） |
| `fix_region_pct` | C10 with-merges の複数 region の割合 % | ssr@bc665168 | 49.6 | 55.6 | after は 9 件中 5 件。子 task・WorkUnit 単位の merge を 1 件と数えるので A の窓と比べられない（比較不能） |
| `fix_module_pct` | C10 no-merges の複数 module の割合 % | ssr@bc665168 | 69.1 | 0.0 | after は 1 件中 0 件（比較不能） |
| `fix_module_pct` | C10 with-merges の複数 module の割合 % | ssr@bc665168 | 71.1 | 55.6 | after は 9 件中 5 件（比較不能。理由は fix_region_pct と同じ） |

- B0 の後 2 週間の窓は 2026-12-01T00:00:00Z に閉じる。閉じた後に c10-window.md §5 の command を同じ規則で当て直す。no-merges が 30 件に届くかは未確認で、
  届かなかったときの扱い（窓を延ばす、with-merges で数えるなど）は人が決める。
- R8 の rc-loc は同じ snippet を H の branch の HEAD（`bc665168..HEAD`）に当てた。no-merges は 1 件のまま、with-merges は 9 → 14 件（R1〜R5 の統合の 5 件）で、
  比較不能のまま（replan/loc-cost.tsv の `fix_*` の行）。

## (1) import・依存・循環の内部の辺

判定: **未達**。循環の単位と組は before と after で同じだった（module の循環 17、task-core の SCC 55 単位・210 組、task-api の SCC 49 単位・261 組。
組の集合も同じ。b0 と head のどちらの検査器でも同じ）。減ったのは task-api の SCC の行の辺だけ（b0 628 → 618、head 632 → 622、どちらも −10）で、
task-core の SCC の行の辺は 617 → 621（+4）に増えた。2 つの SCC の行の合計は −6。

- task-api の −10 の内訳: D の org の pilot が −12、F の knowledge-pin が +2（`cos/ops/decisions.rs → knowledge`）。D は重複していた書き込み 2 本と
  SqliteStore の wrapper 3 つを消し、org の処理を SCC の外の新しい単位 `org_write` に集めた。行の減りには新しい単位への移動の分が含まれ、
  移動の分と削除の分は分けて測っていない。
- task-core の +4 は `task-core/src/chat/attachments.rs → store`（F の provenance-read）。組は増えていないので repo-health の `deps.cycle` は fail にならない。
- health の既存負債（size.responsibility-growth 94、dup.contract 210、owner 不一致 31）は件数も finding の scope の集合も同じ。
- `#[path]` で物理と論理の単位が食い違う file は task-dispatch で 9 → 9、workspace で 15 → 15。A の未解決（`docs/architecture-map.md:67` が
  check_item・review_limits・inventory の 3 file を `#[path]` の説明に書いていない）も残る（[struct-deps.md](struct-deps.md) §3）。

A の項目:

| # | A の項目 | A の before | after | 差 | 判定 |
|---|---|---|---|---|---|
| #2 | Cargo の依存の辺 / crate 循環 | 49（normal 35・dev 14）/ 0 | 49（normal 35・dev 14）/ 0 | 0 / 0 | 変化なし（Cargo.toml・Cargo.lock の差分 0） |
| #3 | 既存の module 循環 | 17 | 17 | 0 | 減っていない |
| #4 | task-core の SCC（単位・組・行） | 55・210・617 | 55・210・621 | 0・0・+4 | 行の辺が増えた（組は同じ） |
| #5 | task-api の SCC（単位・組・行。b0） | 49・261・628 | 49・261・618 | 0・0・−10 | 行の辺だけ減った（単位・組は同じ） |
| #6 | size.responsibility-growth | 94 | 94 | 0 | 変化なし |
| #7 | dup.contract | 210 | 210 | 0 | 変化なし |
| #8 | owner 不一致（arch-map.wrong_owner） | 31 | 31 | 0 | 変化なし |
| #14 | `#[path]`（task-dispatch） | 6 ＋ 2（逆向き 1） | 同じ宣言（H の数え方で 9 → 9） | 0 | 変化なし（地図の 3 file の抜けも残る） |

tsv の行（struct-deps.tsv の循環・Cargo・health・`#[path]` の行）:

| metric | scope | checker | before | after | 差 | A と違う数え方の理由・注 |
|---|---|---|---|---|---|---|
| `cargo_edges` | workspace の Cargo 宣言（normal+dev、cargo metadata） | - | 49 | 49 | 0 | A と同じ値（49 = normal 35 + dev 14）。Cargo.toml・Cargo.lock の B0 との差分 0 なので after の tree の cargo metadata を before にも使える |
| `crate_cycles` | workspace の通常依存の crate の SCC | - | 0 | 0 | 0 | A は数え方の snippet を書いていない。H は cargo metadata の normal の辺の強連結成分（2 crate 以上）で数えた |
| `module_cycles` | repo-health deps.cycle（C2 の SCC の数） | b0 | 17 | 17 | 0 | b0 は after の profile を読めない（profile_invalid）ので、profile を B0 から読み、測る tree を after にした（struct-deps.md §2 の 1） |
| `module_cycles` | repo-health deps.cycle（C1 の findings） | head | 17 | 17 | 0 | head の検査器（B の変更入り）。C2 の SCC の数も 17 → 17 |
| `tc_scc_units` | task-core の SCC（C2） | b0 | 55 | 55 | 0 | after の profile は B0 から読む（§2 の 1） |
| `tc_scc_units` | task-core の SCC（C2） | head | 55 | 55 | 0 | head の検査器でも task-core の値は A と同じ |
| `tc_scc_pairs` | task-core の SCC の単位の組（C2） | b0 | 210 | 210 | 0 | 組の集合も同じ。after の profile は B0 から読む |
| `tc_scc_pairs` | task-core の SCC の単位の組（C2） | head | 210 | 210 | 0 | 組の集合も同じ |
| `tc_scc_lines` | task-core の SCC の行の辺（C2） | b0 | 617 | 621 | +4 | A と同じ数え方。+4 は task-core/src/chat/attachments.rs → store（F の provenance-read） |
| `tc_scc_lines` | task-core の SCC の行の辺（C2） | head | 617 | 621 | +4 | b0 と同じ差（+4） |
| `ta_scc_units` | task-api の SCC（C2） | b0 | 49 | 49 | 0 | after の profile は B0 から読む |
| `ta_scc_units` | task-api の SCC（C2） | head | 49 | 49 | 0 | — |
| `ta_scc_pairs` | task-api の SCC の単位の組（C2） | b0 | 261 | 261 | 0 | 組の集合も同じ |
| `ta_scc_pairs` | task-api の SCC の単位の組（C2） | head | 261 | 261 | 0 | 組の集合も同じ |
| `ta_scc_lines` | task-api の SCC の行の辺（C2） | b0 | 628 | 618 | −10 | A と同じ数え方。−12 は D の org の pilot（org.rs→cos −3・admin.rs→handlers −2・admin.rs→skills −2・skills.rs→handlers −4・schema.rs→handlers −1）、+2 は F（cos/ops/decisions.rs→knowledge） |
| `ta_scc_lines` | task-api の SCC の行の辺（C2） | head | 632 | 622 | −10 | head の検査器は super:: の 4 行を辺にするので before が A より 4 多い（632）。差は b0 と同じ −10 |
| `responsibility_growth` | repo-health size.responsibility-growth（C1） | head | 94 | 94 | 0 | C1 の b0 × after は profile_invalid で測れないので head の行だけ（§2 の 2）。finding の scope の集合も同じ。b0 × B0 は 94 |
| `dup_contract` | repo-health dup.contract（C1） | head | 210 | 210 | 0 | 同上。b0 × B0 は 210 |
| `owner_mismatch` | repo-health arch-map.wrong_owner（C1） | head | 31 | 31 | 0 | 同上。b0 の arch-map は checker_missing で 0 になるので、比較は head × before と head × after（scope の集合も同じ） |
| `path_attr_files` | task-dispatch の production の #[path]（A §3.2） | - | 9 | 9 | 0 | A §3.2 の「8 ＋ 逆向き 1」を足して 9 とした |
| `path_attr_files` | workspace の production の #[path]（単位の食い違い、H の追加） | - | 15 | 15 | 0 | H の追加。workspace 全体で repo-health の単位が物理と論理で違う宣言（task-dispatch 9・task-worker 5・task-core 1） |

## (2) 共有状態・SQLite を直接参照する実装 file

判定: **未達**。`rusqlite` を書く production file（task-core/src/store/ の外）は 74 → 75（+1）、SQL 文字列を持つ file は 50 → 50（file の集合も同じ）。

- +1 は D が足した共有 use-case `crates/task-api/src/org_write.rs`（`use rusqlite::Transaction`。呼び手の transaction を受けて書く）で、抽象の追加に伴う増加。
- F は task-api の `knowledge.rs` と task-ops の `lifecycle.rs` の中の自前の接続と closure を消したが、A の正規表現は「接続を開く行」と
  「呼び手の `&Transaction` を受ける行」を区別しないので、file の数には出ない（[struct-store.md](struct-store.md) §7）。
- Dispatcher の参照: self.store を書く file は依頼 scope 44 → 38・論理 scope 48 → 42（C3。E1 でも −6）、self.config を書く file は
  依頼 35 → 31・論理 39 → 35（−4）。分母（依頼 69・論理 76）は変わらない。出現数は self.store が C3 633 → 633・E1 759 → 759、self.config が
  C3 218 → 220（+2 は rustfmt で割れていた 2 か所を 1 行にした分）・E1 227 → 227 で、参照の総量は減っていない。file 数の減りは E が対象 file の参照を
  入口の file へ移した結果（struct-store.tsv の note）なので、移動として成果に数えない（記録する）。

A の項目:

| 出所 | A の項目 | A の before | after | 差 | 判定 |
|---|---|---|---|---|---|
| (2) の before | `rusqlite` を書く production file（task-core/src/store/ の外） | 74 | 75 | +1 | 未達（org_write.rs の追加で増えた） |
| (2) の before | SQL 文字列を持つ production file | 50 | 50 | 0 | 未達 |
| #9 | dispatcher/ の非試験 file・self.store・self.config（依頼 scope、C3。file 数/出現数） | 69・44/633・35/218 | 69・38/633・31/220 | 0・−6/0・−4/+2 | file 数だけ減った（参照の移動。成果に数えない） |
| #9 | 同（論理 scope、C3） | 76・48/661・39/224 | 76・42/661・35/226 | 0・−6/0・−4/+2 | 同上 |

tsv の行（struct-store.tsv の rusqlite・SQL 文字列・dispatcher の行）:

| metric | scope | checker | before | after | 差 | A と違う数え方の理由・注 |
|---|---|---|---|---|---|---|
| `rusqlite_prod_files` | prod-excl-task-core-store | a-c4 | 74 | 75 | +1 | A と同じ数え方（A §7 C4）。+1 は crates/task-api/src/org_write.rs（D の共有 use-case、use rusqlite::Transaction） |
| `sql_string_prod_files` | prod-excl-task-core-store | a-c4 | 50 | 50 | 0 | A と同じ数え方（C4）。file の集合も同じ |
| `dispatcher_files` | request/C3 | a-c3 | 69 | 69 | 0 | 分母。A §7 C3 |
| `dispatcher_files` | request/E1 | e-e1 | 69 | 69 | 0 | 分母。E1 は E の数え方（改行をまたぐ self\n.store も数える） |
| `dispatcher_files` | logical/C3 | a-c3 | 76 | 76 | 0 | 分母。69 − project_decider_ts.rs ＋ #[path] の 8 file（after で選び直して同じ） |
| `dispatcher_files` | logical/E1 | e-e1 | 76 | 76 | 0 | 分母（E1） |
| `dispatcher_store_files` | request/C3 | a-c3 | 44 | 38 | −6 | A と同じ数え方（C3）。−6 は E が対象 file の参照を入口の file に移した分（出現数は同じ） |
| `dispatcher_store_files` | request/E1 | e-e1 | 46 | 40 | −6 | E1 で数えても −6 |
| `dispatcher_store_files` | logical/C3 | a-c3 | 48 | 42 | −6 | 論理 scope でも −6 |
| `dispatcher_store_files` | logical/E1 | e-e1 | 50 | 44 | −6 | 論理 scope・E1 でも −6 |
| `dispatcher_store_hits` | request/C3 | a-c3 | 633 | 633 | 0 | A と同じ数え方（C3）。出現数は同じ |
| `dispatcher_store_hits` | request/E1 | e-e1 | 759 | 759 | 0 | E1 |
| `dispatcher_store_hits` | logical/C3 | a-c3 | 661 | 661 | 0 | 論理 scope |
| `dispatcher_store_hits` | logical/E1 | e-e1 | 791 | 791 | 0 | 論理 scope・E1 |
| `dispatcher_config_files` | request/C3 | a-c3 | 35 | 31 | −4 | A と同じ数え方（C3）。−4（E） |
| `dispatcher_config_files` | request/E1 | e-e1 | 35 | 31 | −4 | E1 でも −4 |
| `dispatcher_config_files` | logical/C3 | a-c3 | 39 | 35 | −4 | 論理 scope でも −4 |
| `dispatcher_config_files` | logical/E1 | e-e1 | 39 | 35 | −4 | 論理 scope・E1 でも −4 |
| `dispatcher_config_hits` | request/C3 | a-c3 | 218 | 220 | +2 | A と同じ数え方（C3）。+2 は rustfmt で割れていた self\n.config 2 か所を 1 行にした分（E） |
| `dispatcher_config_hits` | request/E1 | e-e1 | 227 | 227 | 0 | E1 では改行をまたぐ参照も数えるので差 0 |
| `dispatcher_config_hits` | logical/C3 | a-c3 | 224 | 226 | +2 | 論理 scope。+2 は上と同じ |
| `dispatcher_config_hits` | logical/E1 | e-e1 | 233 | 233 | 0 | 論理 scope・E1 |

## (3) HTTP と CoS の相互 import と SQL の公開面

判定: **一部だけ減った**。消した method の分（`SqliteStore` の `pub` −2・private −1）と handlers ⇄ cos の行の辺の減り（−3・−2）がある。
handlers ⇄ cos の単位の循環は解けておらず、task-api の `&Transaction` の引数は +1 になった。

- 公開面: `SqliteStore` の `pub` method 633 → 631（D の store-prune が `org_upsert_browser_settings`・`org_delete` を消した）、private 42 → 41
  （`org_upsert_with_actor`）。消した method なので成果に数える。impl block 102、`pub(crate)` 等 67、task-core の逆依存の crate 11 は同じ。
- handlers ⇄ cos: handlers → cos 22 → 19 行（13 → 12 file）、cos → handlers は `crate::` 48 → 46 行（9 file のまま）、`super::` 4 → 4 行、
  A の C5 の式 52 → 50 行。git grep と両方の検査器の辺で同じ差。task-api の SCC の単位 49・組 261 は同じ（(1)）。この減りは D が org の処理を
  `org_write` に集めた変更による。
- `&Connection`・`&Transaction` の引数: task-api 14 → 15（+1 は `org_write.rs:77` の `tx: &Transaction`）、task-core 320（うち store/ 205）と
  task-ops 7 は同じ。`Fn(&Connection)` の closure 引数は task-ops 5 → 4（F が `apply_project_action` の closure を消した）。
- A の数え方の外（F の進捗の値。H は再計測していない）: task-core に `&SqliteStore` を取る `pub` の自由関数が F で 7 → 8 に増えた
  （method を増やさない代わりに関数にした。[struct-store.md](struct-store.md) §1）。
- `Applied` の形の残数（applied_narrow・applied_wide・applied_functions）も下の表に置いた。領域ごとの内訳と移行の提案は「Applied の形の残数と領域ごとの移行の提案」の節。

A の項目:

| 出所 | A の項目 | A の before | after | 差 | 判定 |
|---|---|---|---|---|---|
| #10 | `SqliteStore` の impl（plain・path 付き込み・trait） | 102・105・6 | 102・105・6 | 0 | 変化なし |
| #11 | `SqliteStore` の pub method / pub(crate) 等 | 633 / 67 | 631 / 67 | −2 / 0 | 減った（D が消した 2 method） |
| #12 | task-core に依存する crate（通常依存の直接・間接） | 11 | 11 | 0 | 変化なし |
| (3) の before | handlers → cos の行（git grep） | 22 | 19 | −3 | 行は減った。単位の循環は残る |
| (3) の before | cos → handlers の行（`crate::` ＋ `super::`） | 48 ＋ 4 | 46 ＋ 4 | −2 ＋ 0 | 同上 |
| (3) の before | `&Connection` 型の引数（task-core（うち store/）・task-api・task-ops） | 320（205）・14・7 | 320（205）・15・7 | 0・+1・0 | task-api で増えた |

tsv の行（struct-deps.tsv の handlers ⇄ cos、struct-store.tsv の引数・SqliteStore・Applied の行）:

| metric | scope | checker | before | after | 差 | A と違う数え方の理由・注 |
|---|---|---|---|---|---|---|
| `handlers_to_cos_lines` | git grep crate::cos::（C5、handlers/ と handlers.rs） | - | 22 | 19 | −3 | A §7 C5 の git grep と同じ。13 → 12 file |
| `handlers_to_cos_lines` | repo-health の辺（C2 の task-api の SCC、handlers→cos） | b0 | 22 | 19 | −3 | A §6 の 2 に従い repo-health の辺も並べた（b0） |
| `handlers_to_cos_lines` | repo-health の辺（C2 の task-api の SCC、handlers→cos） | head | 22 | 19 | −3 | 同（head） |
| `cos_to_handlers_lines` | git grep crate::handlers（C5、cos/） | - | 48 | 46 | −2 | A の C5 の式から crate:: の行だけを分けた（H の分け方）。9 file のまま |
| `cos_to_handlers_lines` | git grep (crate\|super(::super)*)::handlers（A の C5 の式） | - | 52 | 50 | −2 | A の C5 の式（crate:: と super:: の合計） |
| `cos_to_handlers_lines` | repo-health の辺（C2 の task-api の SCC、cos→handlers） | b0 | 48 | 46 | −2 | b0 の検査器は super:: を辺にしない（A の 48 と同じ数え方） |
| `cos_to_handlers_lines` | repo-health の辺（C2 の task-api の SCC、cos→handlers） | head | 52 | 50 | −2 | head の検査器は super:: の 4 行を辺にする |
| `cos_to_handlers_super_lines` | git grep super(::super)*::handlers（C5、cos/） | - | 4 | 4 | 0 | super:: の行だけを分けた（H の分け方）。cos/ops/admin.rs の accounts の 4 行 |
| `conn_arg_task_core` | crates/task-core/occ | a-c6 | 320 | 320 | 0 | A と同じ数え方（C6） |
| `conn_arg_task_core` | crates/task-core/files | a-c6 | 76 | 76 | 0 | 同じ grep の file 数（補助） |
| `conn_arg_task_core_store` | crates/task-core/src/store/occ | a-c6 | 205 | 205 | 0 | A と同じ数え方（task-core の内数） |
| `conn_arg_task_core_store` | crates/task-core/src/store/files | a-c6 | 49 | 49 | 0 | A は file 数を書いていない。H が同じ grep の出力の file 数を数えた |
| `conn_arg_task_api` | crates/task-api/occ | a-c6 | 14 | 15 | +1 | A と同じ数え方。+1 は org_write.rs:77 の tx: &Transaction（D） |
| `conn_arg_task_api` | crates/task-api/files | a-c6 | 9 | 10 | +1 | 同じ grep の file 数。+1 は org_write.rs |
| `conn_arg_task_ops` | crates/task-ops/occ | a-c6 | 7 | 7 | 0 | A と同じ数え方。lifecycle.rs の apply_project_change_tx の引数の型が &Connection → &Transaction（F）。数は同じ |
| `conn_arg_task_ops` | crates/task-ops/files | a-c6 | 4 | 4 | 0 | 同じ grep の file 数 |
| `conn_fn_closure` | crates/task-core | a-c6 | 13 | 13 | 0 | 補助（A §4 の Fn(&Connection) の closure 引数） |
| `conn_fn_closure` | crates/task-api | a-c6 | 4 | 4 | 0 | 補助 |
| `conn_fn_closure` | crates/task-ops | a-c6 | 5 | 4 | −1 | 補助。−1 は task-ops/src/lifecycle.rs の apply_project_action（F が消した） |
| `sqlitestore_impl` | plain | a-c6 | 102 | 102 | 0 | A と同じ数え方（C6）。88 file のまま |
| `sqlitestore_impl` | path | a-c6 | 105 | 105 | 0 | path 付きの impl crate::store::SqliteStore 3 を含む |
| `sqlitestore_impl` | trait | a-c6 | 6 | 6 | 0 | 補助。impl … for SqliteStore の 6 件は同じ |
| `sqlitestore_pub` | plain | a-c6 | 633 | 631 | −2 | A と同じ数え方（括弧を数える近似）。−2 は org_upsert_browser_settings・org_delete（D の store-prune が消した） |
| `sqlitestore_pub` | path | a-c6 | 636 | 634 | −2 | path 付きの 3 block を含む |
| `sqlitestore_pub_crate` | plain | a-c6 | 67 | 67 | 0 | pub(crate)・pub(super)・pub(in …) |
| `sqlitestore_pub_crate` | path | a-c6 | 67 | 67 | 0 | path 付きを含む |
| `sqlitestore_private` | plain | a-c6 | 42 | 41 | −1 | 補助。−1 は org_upsert_with_actor（D） |
| `sqlitestore_rdeps` | normal-transitive | a-c7 | 11 | 11 | 0 | A と同じ数え方（C7）。before は git archive した B0 の tree に cargo metadata --offline を当てた（作業ツリーを切り替えないため） |
| `applied_narrow` | crates/task-api/src | d-narrow | 14 | 14 | 0 | A に無い指標（D の付記の狭い command）。file の集合も同じ |
| `applied_wide` | crates/task-api/src | d-wide | 19 | 18 | −1 | A に無い指標（D の付記の広い command）。−1 は handlers/org.rs（D）。回帰条件 18 以下を満たす |
| `applied_functions` | crates/task-api/src | h-awk | 58 | 55 | −3 | H の補助（awk）。−3 は org_commit（D）・record_op・project_action_op（F） |

## (4) 変更波及

判定: **未達**（減っていない）。修正の波及（#13）は **比較不能**。

- S1（新しい CoS 業務操作）: 変える production file 3 → 4、単位 3 → 4、crate 2 → 2。+1 は D が足した `crates/task-api/src/org_write.rs`
  （G の scenarios.env の選び直し）で、抽象の追加による増加。
- S2（WorkUnit の終了処理）: 4 file・2 単位・1 crate のまま。
- import する単位（C7b）は検査器の版をそろえると同じ: b0 で S1 92 / 369（12 package）、S2 6 / 14（4 package）。head で S1 93 / 365（before' は
  head の検査器 × B0）。`test-affected.sh --list` の対象も A と同じ集合（S1 14 crate ＋ web ＋ ts-verify、S2 5 crate）。
- 修正の波及（#13）: A の窓の値は再現した（no-merges 194 件・複数 region 46.9 %・複数 module 69.1 %）。B0 の後の窓
  （`bc665168..HEAD`、SINCE 2026-11-17T00:00Z）で production を変えた commit は no-merges で 1 件、merge を含めて 9 件しかない。9 件はアーキ v4 の C・E・F の
  commit と WorkUnit の統合、main の変更を E・F の branch に取り込んだ merge で、通常の修正 1 件と粒度が違う。割合は A の窓と比べられない
  （[loc-cost.md](loc-cost.md) §3）。

A の項目:

| 出所 | A の項目 | A の before | after | 差 | 判定 |
|---|---|---|---|---|---|
| (4) の before | S1 で変える file・単位・crate | 3・3・2 | 4・4・2 | +1・+1・0 | 増えた（抽象の追加） |
| (4) の before | S2 で変える file・単位・crate | 4・2・1 | 4・2・1 | 0 | 変化なし |
| (4) の before | S1 の単位を直接・推移的に import する単位（b0） | 92・369 | 92・369 | 0 | 変化なし |
| (4) の before | S2 の単位を直接・推移的に import する単位（b0） | 6・14 | 6・14 | 0 | 変化なし |
| (4) の before | test-affected の対象（S1・S2） | 14 crate ＋ web ＋ ts-verify・5 crate | 同じ集合 | 0 | 変化なし |
| #13 | 修正の波及（no-merges。件数・複数 region・複数 module） | 194・46.9 %・69.1 % | 1・0.0 %・0.0 % | 比較不能 | 比較不能（件数が少ない） |
| #13 | 修正の波及（with-merges） | 246・49.6 %・71.1 % | 9・55.6 %・55.6 % | 比較不能 | 比較不能（1 件が WorkUnit か task の束） |

tsv の行（loc-cost.tsv の S1・S2・修正の波及の行）:

| metric | scope | checker | before | after | 差 | A と違う数え方の理由・注 |
|---|---|---|---|---|---|---|
| `s1_files` | S1 で変える production file | - | 3 | 4 | +1 | after は G の scenarios.env の選び直し。+1 は D の org_write.rs（抽象の追加による増加） |
| `s1_units` | S1 で変える単位（repo-health の単位 crates/<c>/src/<第 1 要素>） | - | 3 | 4 | +1 | +1 は task-api の org_write |
| `s1_crates` | S1 で変える crate | - | 2 | 2 | 0 | A と同じ数え方 |
| `s2_files` | S2 で変える production file | - | 4 | 4 | 0 | A と同じ数え方（A と同じ 4 file） |
| `s2_units` | S2 で変える単位 | - | 2 | 2 | 0 | A と同じ数え方 |
| `s2_crates` | S2 で変える crate | - | 1 | 1 | 0 | A と同じ数え方 |
| `s1_direct_importers` | S1 の単位を直接 import する単位（C7b） | b0 | 92 | 92 | 0 | b0 の検査器で HEAD を数えた（profile は B0 から読む）。after は 4 単位。b0 × B0 は A の 92 を再現 |
| `s1_direct_importers` | S1 の単位を直接 import する単位（C7b） | head | 93 | 93 | 0 | head の検査器。before は before'（head の検査器 × B0。A の値ではない）。A との差は B の検査器の変更による |
| `s1_transitive_importers` | S1 の単位を推移的に import する単位（C7b） | b0 | 369 | 369 | 0 | b0。package 12 → 12。b0 × B0 は A の 369 を再現 |
| `s1_transitive_importers` | S1 の単位を推移的に import する単位（C7b） | head | 365 | 365 | 0 | head。before は before'。package 12 → 12 |
| `s2_direct_importers` | S2 の単位を直接 import する単位（C7b） | b0 | 6 | 6 | 0 | b0。head の検査器でも 6 → 6 |
| `s2_transitive_importers` | S2 の単位を推移的に import する単位（C7b） | b0 | 14 | 14 | 0 | b0。package 4 → 4。head の検査器でも 14 → 14 |
| `s1_ta_crates` | S1 の test-affected --list の crate（comment の probe） | - | 14 | 14 | 0 | A と同じ数え方（comment の probe）。＋ web ＋ ts-verify。A と同じ集合 |
| `s2_ta_crates` | S2 の test-affected --list の crate（comment の probe） | - | 5 | 5 | 0 | A と同じ数え方。A と同じ集合 |
| `fix_commits` | 修正の波及 C10 no-merges（production を変えた first-parent の非 merge commit） | ssr@bc665168 | 194 | 1 | 比較不能 | after の窓は bc665168..HEAD（SINCE 2026-11-17T00:00Z）。production を変えたのは 12 commit のうち 1 件 |
| `fix_commits` | 修正の波及 C10 with-merges（merge commit を first-parent の差分で含める） | ssr@bc665168 | 246 | 9 | 比較不能 | after は 41 commit のうち 9 件。すべてアーキ v4 の変更か WU の統合の merge で、通常の修正と粒度が違う |
| `fix_region_pct` | C10 no-merges の複数 region（crate か top-level の dir）の割合 % | ssr@bc665168 | 46.9 | 0.0 | 比較不能 | after は 1 件中 0 件。1 件では割合にならない |
| `fix_region_pct` | C10 with-merges の複数 region の割合 % | ssr@bc665168 | 49.6 | 55.6 | 比較不能 | after は 9 件中 5 件。merge 1 件が WorkUnit か task の束 |
| `fix_module_pct` | C10 no-merges の複数 module の割合 % | ssr@bc665168 | 69.1 | 0.0 | 比較不能 | after は 1 件中 0 件 |
| `fix_module_pct` | C10 with-merges の複数 module の割合 % | ssr@bc665168 | 71.1 | 55.6 | 比較不能 | after は 9 件中 5 件 |

## (5) build・test-affected・review・health の時間と費用

判定: **health の S2 は A の +17.3 % で悪化が残る。review の時間と費用は比較不能**。ほかの 9 組は A の +10 % 以内。

- 10 組の after は G の 3 回の中央値の写し。G の測定の tree と HEAD で crates/・scripts/・web/・Cargo.lock・Cargo.toml の差分は 0 で、H は測り直していない
  （[loc-cost.md](loc-cost.md) §4）。
- health_s2 の 96.09 s は A の +10 %（90.11 s）を超える。G が同じ host で before を測り直した中央値は 87.26 s、コード由来は `deps.violation` の CPU +2.02 s
  （+2.3 %）で、人が決定 h1 で「直さない（keep）」と決めた（[G の進捗](../2026-11-19-arch-v4-g.md)）。
- review の窓: A は [2026-11-10T00:00Z, 2026-11-17T15:00Z)（7.625 日）、after は [2026-11-17T15:00Z, 2026-11-19T12:00Z)（1.875 日）、promo は C を含む release の
  昇格の後 [2026-11-18T12:18:40Z, 2026-11-19T12:00Z)（0.987 日）。日あたりの reviewer run と費用は A の窓より少ないが、窓が短く仕事の量も違うので
  増減とは書かない（[review-prod.md](review-prod.md) §0）。A と同じ長さの窓は 2026-11-25T06:00Z の後に揃う（同 §7.1）。
- 設計 review（review-prod.md §4・§6）: promo の窓で要求 7 件・3 task（D・C の申し送り・F）、run の費用の合計 $14.33、日あたり 7.092 件・$14.52
  （A の窓は 3.541 件・$5.87）。C の見積もり（日あたり +0.670〜+4.498 回・+$1.37〜+$9.21）と比べると、A の窓との差の数え方では +3.551 回/日・+$8.65/日
  （範囲の中）、`new_*` の宣言の無い要求の数え方では 6.08 回/日・$12.59/日（上限を超える）で、数え方で結論が変わる。

時間（tsv の行。loc-cost.tsv の 10 組。単位は wall の秒、差の括弧は A 比）:

| metric | scope | checker | before | after | 差 | A と違う数え方の理由・注 |
|---|---|---|---|---|---|---|
| `check_cold` | cargo check --workspace の cold の wall 中央値 s | - | 53.65 | 53.90 | +0.25（+0.5 %） | after は G の値（build-check.tsv）。H は測り直していない（G の測定の tree と source の差分 0） |
| `check_warm_s1` | cargo check の warm S1（const の probe）の wall 中央値 s | - | 42.47 | 42.64 | +0.17（+0.4 %） | after は G の値（build-check.tsv） |
| `check_warm_s2` | cargo check の warm S2 の wall 中央値 s | - | 14.67 | 15.56 | +0.89（+6.1 %） | after は G の値（build-check.tsv） |
| `test_build_cold` | cargo nextest run --workspace --no-run の cold の wall 中央値 s | - | 142.39 | 147.03 | +4.64（+3.3 %） | after は G の値（build-test.tsv） |
| `test_build_warm_s1` | test build の warm S1 の wall 中央値 s | - | 138.69 | 138.10 | −0.59（−0.4 %） | after は G の値（build-test.tsv） |
| `test_build_warm_s2` | test build の warm S2 の wall 中央値 s | - | 77.64 | 81.60 | +3.96（+5.1 %） | after は G の値（build-test.tsv） |
| `health_s1` | repo-health S1（const の probe、--head worktree）の wall 中央値 s | head | 86.56 | 93.58 | +7.02（+8.1 %） | after は G の値（scope.tsv） |
| `health_s2` | repo-health S2 の wall 中央値 s | head | 81.92 | 96.09 | +14.17（+17.3 %） | after は G の値（scope.tsv）。A の +10 %（90.11）を超える。G が同じ host で測り直した before は 87.26、コード由来 +2.02（人の決定 h1=keep） |
| `test_affected_s1` | bash scripts/dev/test-affected.sh S1（comment の probe）の wall 中央値 s | - | 893.41 | 906.03 | +12.62（+1.4 %） | after は G の値（ta-s1.tsv・ta-mix.tsv）。規則により測り直さない |
| `test_affected_s2` | test-affected S2 の wall 中央値 s | - | 655.59 | 707.76 | +52.17（+8.0 %） | after は G の値（ta-s2.tsv・ta-mix.tsv）。規則により測り直さない |

review の時間と費用（[review-prod.tsv](review-prod.tsv)。A §7 C11 の query。wall は s、cost は USD）:

| metric | A の窓 | after の窓 | promo の窓 | 注 |
|---|---|---|---|---|
| `review_runs` | 165 | 30 | 17 | 窓の日数が違う（7.625・1.875・0.987 日）ので件数は比べない。promo の 17 run のうち 7 run は設計 review |
| `review_wall_median` | 1137.5 | 514.4 | 434.5 | wall_ms のある run（A 130・after 27・promo 16）の中央値。A の値は A の進捗と同じ |
| `review_cost_median` | 2.047 | 2.167 | 2.036 | cost_usd のある run（A 127・after 27・promo 16）の中央値。A の値は A の進捗と同じ |
| `review_runs_per_day` | 21.64 | 16.00 | 17.22 | 窓が短く、仕事の量も違うので増減とは書かない |
| `review_cost_per_day` | 36.74 | 28.59 | 26.92 | 同上 |

## (6) production と試験の LoC

判定: **増えた**（production は純減していない。アーキ v4 の依頼は一律の純減を求めていない）。分類規則は B0 の source-size-report に固定した
（before と after の `config_sha256` が同じ）。

- production は全言語・Rust とも +822 行（差はすべて Rust）。試験 +4743 行、own_total_with_tests +5829 行、tooling +264 行。
- before は A の (6) と一致し、after は G の進捗の (6) と一致した（[loc-cost.md](loc-cost.md) §1）。

A の項目:

| 出所 | A の項目 | A の before | after | 差 | 判定 |
|---|---|---|---|---|---|
| #1 | production 全言語 / Rust | 375099 / 330408 | 375921 / 331230 | +822 / +822 | 増えた |
| (6) の before | 試験 / own_total_with_tests | 476771 / 1025677 | 481514 / 1031506 | +4743 / +5829 | 増えた |

tsv の行（loc-cost.tsv の LoC の行）:

| metric | scope | checker | before | after | 差 | A と違う数え方の理由・注 |
|---|---|---|---|---|---|---|
| `loc_prod_all` | production 全言語（views.product_only.lines） | ssr@bc665168 | 375099 | 375921 | +822 | A と同じ数え方（A §7 C12、分類規則は B0 に固定）。差はすべて Rust |
| `loc_prod_rust` | production Rust（views.product_only.rust_lines） | ssr@bc665168 | 330408 | 331230 | +822 | A と同じ数え方 |
| `loc_test` | 試験 = class_totals.external_test + inline_test | ssr@bc665168 | 476771 | 481514 | +4743 | A と同じ数え方（external_test + inline_test）。Rust +4265、Python +478 |
| `loc_own_total` | own_total_with_tests 全言語（views.own_total_with_tests.lines） | ssr@bc665168 | 1025677 | 1031506 | +5829 | A と同じ数え方。tooling を含む |
| `loc_own_total_rust` | own_total_with_tests Rust（views.own_total_with_tests.rust_lines） | ssr@bc665168 | 879344 | 884431 | +5087 | A と同じ数え方 |
| `loc_tooling` | tooling（class_totals.tooling） | ssr@bc665168 | 25253 | 25517 | +264 | A の (6) に無い区分（H の補助）。Python +190、TOML +74 |

## 判定経路

判定: **10 行すべて期待どおり（未達 0）**。C の 6 ケース・prompt の 3 件・repo-health の python 試験 126 件が通り、本物の検査器では SCC の中の新しい組が止まり、
同じ file の普通の変更は通った。本番でも、C を含む release の昇格の後の設計 review の要求 7 件はすべて C の形の理由で始まった。

- 出所は [gate.md](gate.md)・[gate.tsv](gate.tsv)（列 `case result command note`）と [review-prod.md](review-prod.md) §5。gate の tree は gate の WorkUnit の HEAD
  （件名 `integrate wu/struct-store (phase measure)`）、health の base は `git merge-base HEAD main`（件名 `integrate wu/close (phase close)`）。
  両者の `scripts/dev/repo-health` の差分は 0 で、検査器は base と同じ内容。host は home-dev、2026-11-19T11:55Z〜12:05Z。
- result は「期待どおりの結果だったか」で、検査器の verdict ではない（危険な probe は検査器が fail を出せば pass）。

gate.tsv の行（case と result は tsv のまま）:

| case | result | 確かめたこと | 要点（gate.tsv の note・gate.md §1・§3） |
|---|---|---|---|
| case1 | pass | `boundary_case_1_authz_one_line_is_high` | 認可の条件 1 行の変更は、`new_*` の宣言が無くても High で、設計 review を求める |
| case2 | pass | `boundary_case_2_equivalent_plain_change_is_low` | 画面・class 外の handler の同等の変更は Low で、設計 review を求めない |
| case3 | pass | `boundary_case_3_move_cannot_escape` | file の移動・別名・`#[path]` の付け替えで class の判定を逃れられない |
| case4 | pass | `boundary_case_4_worsening_dependency_blocks` | 汎用 Rust の fixture で、既存の循環への新しい組は health の関門で止まる |
| case5 | pass | `boundary_case_5_policy_self_approval_impossible` | 候補自身の policy で class を外しても、base の policy で判定される |
| case6 | pass | `boundary_case_6_fail_closed` | 方針が読めない・差分が取れない・判定が欠けるときは合格にしない |
| prompt_boundary | pass | `prompt_boundary_` の 3 件 | 3 tests run: 3 passed, 1311 skipped、exit 0。3,000 文字の objective の末尾を確かめるのは worker の prompt だけ（gate.md §2） |
| repo_health_tests | pass | `test_repo_health_*.py` | Ran 126 tests in 51.6s OK、exit 0。B が足した SCC の内部辺の 5 件（`test_repo_health_scc_edges.py`）を含む |
| probe_scc_edge | pass | 本物の検査器に probe a | SCC の中にまだ無い組を 1 行足した。exit 1・verdict=fail、`deps.cycle` の new 1 件 |
| probe_plain | pass | 本物の検査器に probe b | 同じ file に comment を 1 行足した。exit 0・verdict=pass、`deps.*` の new・worsened 0 件 |

command（gate.tsv の command 列。exit はどれも 0、probe は期待どおりの exit）:

- case1〜case6: `cargo nextest run -p task-dispatch -E 'test(/boundary_case_/)'` → 6 tests run: 6 passed, 1912 skipped
- prompt_boundary: `cargo nextest run -p task-worker -E 'test(/prompt_boundary_/)'` → 3 tests run: 3 passed, 1311 skipped
- repo_health_tests: `python3 -I -m unittest discover -s scripts/tests -p 'test_repo_health_*.py'` → Ran 126 tests、OK
- probe_scc_edge・probe_plain: `python3 scripts/dev/repo-health/repo_health.py --repo . --base "$(git merge-base HEAD main)" --head worktree --policy-version 1 --out <report.json>`
  （dispatcher の health の関門 `health_review/gate.rs` の `run_report` と同じ引数の組で、head だけを `worktree` にした）

本物の検査器の probe の出力の要点（gate.md §3）。どちらも task-api の 49 単位の SCC の単位 `crates/task-api/src/lifecycle` を使った。この単位から
同じ SCC の `knowledge` への組は、probe の前には無い:

- probe a（probe_scc_edge）: `crates/task-api/src/lifecycle.rs` に `use crate::knowledge::KnowledgeItem;` を 1 行足した。
  - exit 1、verdict=fail。summary は findings 696・fail_grade 1・new 1・worsened 0・existing 695。
  - `deps.cycle` は fail（findings 18 = existing 17 ＋ new 1）。new の finding の symbol は `crates/task-api/src/lifecycle -> crates/task-api/src/knowledge`、
    evidence は `crates/task-api/src/lifecycle.rs:26`。fingerprint は gate.tsv の note にある。`deps.violation` は pass（0 件）。
  - 49 単位の SCC の finding 自身は existing のままで、単位の数は増えていない。単位の数が変わらない悪化を、B が足した組の finding（`cycle-edge:` の鍵）が止めた。
- probe b（probe_plain）: 同じ位置に `// probe: comment only (gate-cases)` を 1 行足した。
  - exit 0、verdict=pass。summary は findings 695・fail_grade 0・new 0・worsened 0・existing 695。
  - `deps.cycle` は pass（findings 17、すべて existing）、`deps.violation` は pass（0 件）。
- probe は検査器を走らせた直後に `git checkout -- crates/task-api/src/lifecycle.rs` で戻し、commit していない。戻した後の
  `git status --porcelain -- crates scripts web` は 2 回とも空だった。

本番の判定経路で設計 review が始まった証拠（[review-prod.md](review-prod.md) §0・§5。本番 DB は `mode=ro` で読んだだけ）:

- C を含む release `f85df0209504`（件名「integrate wu/loc-fix (phase fix)」）は 2026-11-18T12:18:40Z に本番の active になった。
  その後の窓 [2026-11-18T12:18:40Z, 2026-11-19T12:00Z)（0.987 日）に、設計 review の要求は 7 件・3 task（D・C の申し送り・F）で、level は 7 件とも high。
  設計 review の run は 7 件・費用の合計 $14.33（合格 4・差し戻し 3）。
- 理由の形: A の窓の 27 件の理由 75 個は、すべて旧い判定の `path_class:<class>+new_*` の形だった。昇格の後の 7 件は C の形
  （`path_class:<class>:<path>` と `invariant:<class>:<不変条件>`）で、7 件中 6 件は `declared:new_*` が無い。review-gate・transaction は C の profile で足した class。
- 「`new_*` の宣言が無い 6 件は旧い判定では High にならなかった」は推定で、旧い判定で再判定はしていない（review-prod.md §5）。

昇格の後の 7 件（review-prod.md §5 の表の写し）:

| event | task | `declared:new_*` | 結果 | cost | wall |
|---|---|---|---|---|---|
| 960582 | D | 無し | 差し戻し | $4.556 | 312.6 s |
| 960771 | D | 無し | 差し戻し | $2.301 | 298.4 s |
| 961175 | C の申し送り | 無し | 合格 | $0.117 | 14.0 s |
| 961342 | D | 無し | 合格 | $2.506 | 354.6 s |
| 962093 | C の申し送り | 無し | 合格 | $0.124 | 12.9 s |
| 964461 | F | new_branch | 差し戻し | $1.905 | 300.0 s |
| 964589 | F | 無し | 合格 | $2.823 | 514.4 s |

理由の例（3 件。`risk.reasons` の写し）:

- 960582（D、2026-11-18T12:38:58Z）: `path_class:store:crates/task-core/src/store/org.rs` ほか store の path 3 個、`invariant:store:task-state-in-sqlite`、`invariant:store:no-data-loss`
- 961175（C の申し送り 01M7Q70NDQAK1KKXHY6WT027QZ、2026-11-18T13:02:46Z）: `path_class:review-gate:crates/task-core/src/design_review.rs`、
  `invariant:review-gate:completion-not-self-reported`、`invariant:review-gate:policy-change-needs-human`、`invariant:review-gate:no-llm-in-dispatcher`
- 964589（F、2026-11-18T23:16:12Z）: `path_class:transaction:crates/task-api/src/knowledge.rs`、`path_class:transaction:crates/task-ops/src/lifecycle.rs`、
  `invariant:transaction:task-state-in-sqlite`、`invariant:transaction:lifecycle-human-epoch-same-tx`

注意（未達ではない）: `prompt_boundary_` の試験が 3,000 文字の objective の末尾まで確かめるのは worker の prompt だけで、planner・design review・final review の
prompt の objective の末尾は確かめていない（gate.md §2・§8）。実 prompt に載ったかは次の節の記録で確かめた。

## 実 prompt

判定: **worker と reviewer は、prompt が残る run のすべてで objective の末尾と受入条件が入っていた（cut 0 件）**。未確認は prompt の無い worker の 4 run。
planner は objective の末尾が 24/24 run で入ったが、task の受入条件は 0/126 件で 1 件も入っていない（planner の prompt に受入条件の節が無い）。
planner の欠けを直すかは人が決める（[prompt-audit.md](prompt-audit.md) の提案 1）。

- 出所は [prompt-audit.md](prompt-audit.md)・[prompt-audit.tsv](prompt-audit.tsv)（201 行と header）。本番 DB は `mode=ro` の URI で開いて読んだだけ（2026-11-19T11:58Z）。
- 範囲: 親 01M7MZT8KX547464YYS76QNP4G の子孫の 15 task（A〜H の 8 件と子 task の 7 件）の、role が worker・planner・reviewer の run 201 件
  （worker 157・planner 24・reviewer 20）。prompt は `<task の workspace>/runs/<run id>/prompt.txt`。
- 判定の規則: 両側の空白の連なりを 1 つにしてから比べる。`objective_tail` は objective の末尾 200 字が prompt に含まれれば found・含まれなければ cut・
  prompt.txt が無ければ missing。受入条件は text ごとに含まれるかを数える。reviewer は review する task、WorkUnit の run は run の開始時に有効だった
  計画の版（`execution_plans` の `units[]`）の objective と done_when と比べた。

kind ごとの集計（prompt-audit.md の表の写し）:

| kind | run | found | cut | missing | 受入条件（prompt のある run） | 受入条件の欠け | prompt の無い run の受入条件 |
|---|---:|---:|---:|---:|---:|---:|---:|
| worker | 157 | 153 | 0 | 4 | 651/651 | 0 | 18（未確認） |
| reviewer | 20 | 20 | 0 | 0 | 112/112 | 0 | 0 |
| planner | 24 | 24 | 0 | 0 | 0/126 | 126 | 0 |
| 計 | 201 | 197 | 0 | 4 | 763/889 | 126 | 18 |

- objective が 1,500 字を超えて found の run は worker 116・reviewer 19・planner 23。最長は A の 7,247 字で、A の worker 3 run と reviewer 2 run の全部で
  末尾が入っていた。以前の「親 task の objective が先頭 1,500 字で切れる」不具合は、この木の run では再現していない。
- 参考（tsv に無い数）: prompt のある WorkUnit の run 150 件で、「このタスク全体の目的（全文）」の節の親 task の objective（1,168〜7,247 字）の末尾 200 字は
  150/150 で見つかった。「最終レビューで確かめる Task の受け入れ条件（参考）」の節の task の受入条件は 849/849 件で見つかった。

切れた例: この audit の規則では cut は 0 件。WorkUnit の objective を今の `work_units` 行と比べた場合だけ、次の 7 run が cut に見える。
7 run とも prompt には計画 v1 の unit の objective と done_when が全部入っていた。計画の改訂が同じ id の WorkUnit の行を続きの run 用の文に書き換えたため、
今の行は run が受け取った文と違う。prompt が切れたのではない。

| task | WorkUnit | run | 計画の版（開始時 → 今） | 今の行と比べた done_when の欠け |
|---|---|---:|---|---|
| B（01M7MZTKAZQY72EX2R4RQ1WVWF） | checker（v1 の objective 1,632 字） | 4 | v1 → v2 | 各 4 件（計 16） |
| C（01M7MZTKB29VF5TX7FCQ15XEMW） | risk-rule（v1 の objective 1,256 字） | 3 | v1 → v3 | 各 3 件（計 9） |

開始時の版と比べると欠けは 0 件。過去の run の監査は `work_units` 行でなく、run の開始時の `execution_plans` の `units[]` を引く必要がある
（prompt-audit.md の提案 2。run ごとの spec を残すかは人が決める）。

prompt の無い run（missing 4 件。どれも `runs/<run id>/` の dir が無く、request.json・stdout.jsonl も無い）:

| task | run | WorkUnit | 開始（UTC） | 終了までの秒 | 受入条件（done_when）の数 | 記録された終わり方 |
|---|---|---|---|---:|---:|---|
| C（01M7MZTKB29VF5TX7FCQ15XEMW） | 01M7PBH4HWDFMXSEMC5AKVKJVK | risk-rule | 2026-11-18T03:34:41 | 2.7 | 5 | `interrupted: aborted (task no longer running under this lease)`、usage null |
| C（01M7MZTKB29VF5TX7FCQ15XEMW） | 01M7PBPTFQQG9N8WDMRW8M5435 | risk-rule | 2026-11-18T03:37:48 | 2.7 | 5 | 同上 |
| C（01M7MZTKB29VF5TX7FCQ15XEMW） | 01M7PBWQKNTNHKM4E3S00DJ1AV | risk-rule | 2026-11-18T03:41:01 | 2.7 | 5 | 同上 |
| D（01M7MZTKB4RYM80S6CT3RST2N9） | 01M7QC2E69H17AXEYD9AS8TK10 | adr-forms | 2026-11-18T13:03:22 | 2.5 | 3 | 同上 |

原因の候補は、worker が prompt を書く前に lease を失い、dispatcher が run を中止したこと（4 run とも計画の改訂の間に始まった）。LLM は呼ばれていない
（usage null）ので、作業に使われた prompt は無い。これらの run に何が渡るはずだったかは未確認。

planner の受入条件（0/126）:

- planner の 24 run で、task の受入条件の text は 1 件も prompt に無く、prompt の見出しにも受入条件の節が無い（B の planner run 01M7ND92G1Q4CVS3YEYKQ9B6SF で確かめた）。
- 原因: `crates/task-worker/src/claude_code/prompt.rs` の `build_plan_prompt`（:745）は `prompt_header`（objective の全文を含む）に計画の指示を足すだけで、
  `task.acceptance` を出さない。`task.acceptance` を出すのは worker 用の `build_execute_prompt`（:649〜:675）と review の prompt（:1685）だけ。
- 影響: planner は task の受入条件の text を見ずに WorkUnit の done_when を書く。受入条件の text にだけある条件は planner の prompt に入らない。
- H の task の受入条件 1 は worker・reviewer の prompt を対象にしているので、planner の欠けはそれとは別の観察として書く。H では直していない。
  今の planner の試験（`crates/task-worker/src/claude_code/tests.rs` の `planner_prompt_*`）は受入条件の有無を見ていないので、この欠けを検出できない。

## 安全不変条件

判定: **11 行すべて pass（fail 0・untested 0）**。passed は延べ 563 件（`cex_lease_restart` 12 件と `cex_same_tick` 3 件は lease_atomicity と
arch_v4_counterexamples の両方に数えた）。D・E・F の反例試験（arch_v4_counterexamples）は 81 passed で、3 つの command がどれも exit 0。

- 出所は [safety.md](safety.md)・[safety.tsv](safety.tsv)（列 `invariant result command passed note`）。試験は `docs/architecture-map.md` の試験の列と、
  `CLAUDE.md`・review 方針（`scripts/dev/repo-health/profiles/celeris.toml` の `[review]` 節）が挙げる根拠の file から選び、`cargo nextest list` で実在を
  確かめてから crate と filter を絞って走らせた。workspace 全体・doc test・e2e は走らせていない。各 filter の `--run-ignored only` の件数はどれも 0。
- 注意（実質は未検査の 9 件）: `CELERIS_USERNS_TESTS` などの env を付けていないので、sqlite_source の task-worker の `db_guard` 7 件と、credential の
  celeris-credentiald の `injection_peer_uid` 2 件は、userns の検査の前に return して pass に数えられた。release gate の gate.json は `userns: true` だが、
  この 9 件が release gate で検査まで進んだかは、gate log が試験を 1 件ずつ出さないので未確認（safety.md の「気づいた点」）。
- nextest の summary の `skipped` は filter の外の試験の数で、実行時の skip ではない。

safety.tsv の行（invariant・result・passed は tsv のまま）:

| invariant | result | passed | 守る条件（safety.md の要約） | 内訳（safety.tsv の note） |
|---|---|---:|---|---|
| sqlite_source | pass | 169 | task の状態の真実は SQLite。状態は `transition` が許す遷移でだけ変わる。worker の run は DB を読むだけ | task-core 140（store::tests 111・legacy::tests 7・client_open_tests 6・migration_harness_tests 2・transition::tests 14）＋ task-worker 29（db_guard::tests。うち 7 件は userns の env 無しで早期 return） |
| events_append_only | pass | 13 | events は追記専用。遷移と event は同じ transaction で書く | task-core 13（append_event_*・apply_transition_*appends* 4 件と create_task_inserts_task_and_events_atomically、test(append_only) の 8 件） |
| lease_atomicity | pass | 51 | lease は 1 つの持ち主だけが持つ。取得と `Transitioned` event は同じ transaction。回収は compare-and-release | task-core 12（lease_reclaim_tests 5・store::tests の lease 7）＋ task-dispatch 39（cex_lease_restart 12・cex_same_tick 3・ownerless_runs 11・orphan_takeover 6・run_supervisor::tests 7） |
| replay | pass | 14 | events を replay した結果は現在の投影と一致する | task-ops 14（replay::tests 全件） |
| project_controller_stop | pass | 14 | 案件の off・paused・cancelled・pause・cancel のとき controller の効果は実行されず suppressed になる | task-dispatch 14（project_wake_pending_* 4・project_wake_human_races_* 3・project_wake_off_is_inert 1・project_recovery_k37_* 3・project_controller_replay_* 3） |
| rust_ts_canary | pass | 16 | canary の対象は単位ごとに決定的に選ぶ。拒否・失敗・予算超過は Rust に倒れる | task-dispatch 16（script_canary 7・cos_digest_script::canary 3・cos_digest_behavior_change の canary 4・ほか 2） |
| acl | pass | 28 | API の主体は認証済みの Bearer token から決め、自己申告から決めない | task-api 28（auth_and_guards 11・principal::tests 3・middleware::tests 3・resolve_principal 1・api_principals 10） |
| credential | pass | 78 | credential の値は broker の外に出さず、相手 process の uid を確かめて渡す | celeris-credentiald 54（broker 17・injection_ipc 19・injection_peer_uid 4・prod_admission 14。うち 2 件は env 無しで早期 return）＋ task-worker 5 ＋ task-api 19 |
| cos_audit | pass | 16 | CoS の変更操作は監査付きの操作層だけを通り、監査と業務の書き込みは同じ transaction に入る | task-api 16（cos_operations 8・cos::operations::skill_table_tests 1・cos_delivery_suite_a の cos_auth 7） |
| selfdeploy_authority | pass | 83 | 昇格していない release の daemon は本番を引き継がない（exit 4）。昇格は verify 済みの release だけ | celeris 80（instance::tests 29・releases::tests 21・releases::audit::tests 2・unpromoted_release 10・instance_handoff 8・releases_api 10）＋ task-api 3 |
| arch_v4_counterexamples | pass | 81 | D・E・F の反例が再発しない（org の書き込みは plan と apply の 1 本、HTTP と CoS の書き込みは 1 つの transaction 等） | org_write_contract 6 ＋ tx_boundary_contract 8 ＋ task-dispatch 67（cex_ 40・structure_guard 14・write_set_ 13） |

command（safety.tsv の command 列の写し。repo の root で。exit はどれも 0）:

```sh
# sqlite_source（169）
cargo nextest run -p task-core -E 'test(/^store::(tests|migration_harness_tests|client_open_tests|legacy::tests)::/) | test(/^transition::tests::/)' && cargo nextest run -p task-worker -E 'test(/^db_guard::tests::/)'
# events_append_only（13）
cargo nextest run -p task-core -E 'test(/^store::tests::append_event_/) | test(/^store::tests::apply_transition_.*appends/) | test(/^store::tests::create_task_inserts_task_and_events_atomically$/) | test(append_only)'
# lease_atomicity（51）
cargo nextest run -p task-core -E 'test(/^store::lease_reclaim_tests::/) | test(/^store::tests::(acquire_lease_|acquire_work_unit_lease_|renew_lease_|apply_transition_releases_lease_)/)' && cargo nextest run -p task-dispatch -E 'test(/^dispatcher::tests::(cex_lease_restart|cex_same_tick|ownerless_runs|orphan_takeover)::/) | test(/^run_supervisor::tests::/)'
# replay（14）
cargo nextest run -p task-ops -E 'test(/^replay::tests::/)'
# project_controller_stop（14）
cargo nextest run -p task-dispatch -E 'test(/^dispatcher::tests::project_wake::effects::project_wake_pending_/) | test(/^dispatcher::tests::project_wake::project_wake_(human_races_|off_is_inert$)/) | test(/project_recovery_k37_/) | test(/^dispatcher::tests::project_wake::replay::project_controller_replay_(paused_project|cancelled_project|invalid_proposals_stop)$/)'
# rust_ts_canary（16）
cargo nextest run -p task-dispatch -E 'test(canary)'
# acl（28）
cargo nextest run -p task-api -E 'binary(auth_and_guards) | (kind(lib) & test(/^(principal|middleware)::tests::|^handlers::tests::resolve_principal_/)) | (binary(browser_eval_surface_suite) & test(/^api_principals::/))'
# credential（78）
cargo nextest run -p celeris-credentiald -E 'binary(broker) | binary(injection_peer_uid) | binary(prod_admission) | binary(injection_ipc)' && cargo nextest run -p task-worker -E 'test(browser_credential)' && cargo nextest run -p task-api -E '(kind(lib) & test(/^secrets::tests::/)) | (binary(run_worker_suite) & test(/^secrets_admin::/))'
# cos_audit（16）
cargo nextest run -p task-api -E 'binary(cos_operations) | (kind(lib) & test(/^cos::operations::/)) | (binary(cos_delivery_suite_a) & test(/^cos_auth::/))'
# selfdeploy_authority（83）
cargo nextest run -p celeris -E '(kind(lib) & test(/^(instance|releases)::/)) | binary(unpromoted_release) | binary(instance_handoff) | (binary(cos_delivery_suite) & test(/^releases_api::/))' && cargo nextest run -p task-api -E '(kind(lib) & test(/^releases::tests::/)) | (binary(browser_eval_surface_suite) & test(/^api_principals::promotion_audit_/))'
# arch_v4_counterexamples（81）
cargo nextest run -p task-api --test cos_delivery_suite org_write_contract && cargo nextest run -p task-api --test cos_delivery_suite tx_boundary_contract && cargo nextest run -p task-dispatch -E 'test(store_access_guard) | test(structure_guard) | test(cex_) | test(write_set_)'
```

## Applied の形の残数と領域ごとの移行の提案

`Applied` の形（`Option<&OperationAudit>` を受けて `Applied` を返す操作）の残数は、D の ADR の付記の 2 本の command と H の補助の awk で数えた
（[struct-store.md](struct-store.md) §3）。広い command の file は 19 → 18 で、回帰条件（18 以下）を満たす。

| metric | 数え方 | before | after | 差 | 注 |
|---|---|---|---|---|---|
| `applied_narrow` | `git grep -l 'Option<&OperationAudit>' crates/task-api/src` | 14 | 14 | 0 | file の集合も同じ（`handlers/org.rs` はもともと入らない） |
| `applied_wide` | 広い command（`'name` の寿命と `crate::cos::operations::` の完全修飾も当てる） | 19 | 18 | −1 | `handlers/org.rs`（D） |
| `applied_functions` | 広い command の file で該当の引数を受ける関数（H の補助） | 58 | 55 | −3 | `org_commit`（D）・`record_op`・`project_action_op`（F） |

- 広い 18 file の内訳: 型の定義側 `cos/operations.rs`（`file_op`）の 1 file・1 関数と、操作の側の 17 file・54 関数。
  狭い command は広い 18 file のうち ★ の 4 file に当たらない（18 − 4 = 14）。
- 撤去の条件（[ADR 2026-11-18 org-write-shared-use-case](../../adr/2026-11-18-org-write-shared-use-case.md) の付記）: その領域の共有の操作を変える task が、
  同じ変更で org_write の形へ移す。
- 下の表の「移行の task を提案する」は提案で、H は起票しない（起票は人が決める）。path は `crates/task-api/src/` からの相対。

| 領域 | file | 残る関数 | 残数（file / 関数） | 提案 | 理由 |
|---|---|---|---|---|---|
| 知識 | `knowledge.rs` | `accept_op`・`reject_op`・`put_page_op` | 1 / 3 | 移行の task を提案する | F が同じ file の `record_op` を移し、HTTP の `write_tx`・CoS の `reject`・`apply_checked` の手順と契約試験が既にある。`knowledge.rs` は `transaction` class の高 risk の path で、2 つの形が同じ file に並ぶ（F の提案 3） |
| task（lifecycle） | `lifecycle.rs` | `task_pause_op` | 1 / 1 | 移行の task を提案する（知識と別の 1 件） | F が同じ file の `project_action_op` を移した。1 関数で、F の lifecycle-tx と同じ手順を当てられる見込み（H は確かめていない） |
| skills | `skills.rs` | `put_skill_op`・`delete_skill_op` | 1 / 2 | 移行の task を提案する | D で org の書き込みを org_write に移した HTTP adapter で、同じ file に 2 つの形が並ぶ |
| task（その他） | `handlers/task_actions.rs`・`handlers/tasks.rs`・`integrate_task.rs` | `retry_op`・`patch_task_op`・`create_comment_op`・`reopen_op`・`rereview_op`・`create_task_op`・`create_task_linked_op`・`integrate_task_op` | 3 / 8 | 意図して残す | 下の共通の理由 |
| 承認 | `approvals.rs` | `decide_op`・`create_standing_rule_op`・`delete_standing_rule_op` | 1 / 3 | 意図して残す | 同上 |
| 変更 | `changes.rs` | `changes_effect`・`integrate_op`・`pr_merge_op` | 1 / 3 | 意図して残す | 同上 |
| 判断 | `decisions.rs` | `answer_op`・`withdraw_op`・`revise_op`・`resolve_op` | 1 / 4 | 意図して残す | 同上 |
| 文書 | `docs.rs` | `docs_effect`・`reject_for`・`docs_init_op`・`put_page_op`・`delete_page_op`・`promote_op`・`maintenance_op` | 1 / 7 | 意図して残す | 同上 |
| 案件 | `handlers/projects.rs` | `create_project_op`・`put_project_brief_op` | 1 / 2 | 意図して残す | 同上 |
| 実行計画 | `execution.rs` ★ | `execution_plan_adopt_op`・`tree_adopt_op`・`phase_gate_op`・`decompose_op` | 1 / 4 | 意図して残す | 同上 |
| cron | `cron_jobs.rs` ★ | `create_job_op`・`update_job_op`・`delete_job_op`・`pause_job_op`・`resume_job_op`・`run_job_op`・補助の `reject_with`・`resolve_for`（`type Audit<'a> = Option<&'a OperationAudit>` を受ける） | 1 / 8 | 意図して残す | 同上 |
| clusters | `handlers/clusters.rs` ★ | `put_local_settings_op`・`put_cluster_settings_op` | 1 / 2 | 意図して残す | 同上 |
| inbox の通知 | `inbox_notifications.rs` ★ | `notice_read_op`・`notice_read_all_op` | 1 / 2 | 意図して残す | 同上 |
| repos | `repos.rs` | `create_repo_op`・`patch_repo_op`・`delete_repo_op` | 1 / 3 | 意図して残す | 同上 |
| model_catalog | `model_catalog.rs` | `put_override_op`・`delete_override_op` | 1 / 2 | 意図して残す | 同上 |
| （型の定義側） | `cos/operations.rs` | `file_op` | 1 / 1 | 残す | 操作の側の file が残る間は要る |

操作の側の合計は 17 file・54 関数（移行の提案 3 領域で 3 file・6 関数、意図して残す領域で 14 file・48 関数）。

意図して残す共通の理由: 移すには領域ごとに HTTP と CoS の契約試験を旧コードで先に固定する作業が要る（D の ADR の付記の「D がこの task で全部を移さない理由」と同じ）。
その領域を次に変える task が、撤去の条件に従って同じ変更で移す。新しい操作をこの形で足さないことは、広い command の回帰条件（18 以下）で見る。

## 未達・要判断

A の 14 項目と共通受入条件 (1)〜(6)、verify 段の md から、減らなかった・増えた・比べられない・確かめられなかった項目を拾い、R1〜R7 の後の値（「replan の後」の節）で書き直した。
「H の measure の数値」の列は書き直す前の表の値のまま、「replan の後」の列が R1〜R7 の後の値。解けた項目は状態の欄に「replan の後に解消」と根拠の値を書き、
残る項目は未達のまま理由と次の候補を書いた（達成とは書かない）。移動・分割・抽象の追加だけの変更と分母の増加は成果に数えていない。
次の候補の「提案 N」は「提案」の節の番号、R の提案は各子の進捗の「提案」の節（どれも起票していない）。
未達のまま閉じるか、次の task を求めるかは人が決める（決定 final-unmet）。

| 項目 | H の measure の数値 | replan の後 | 状態 | 理由 | 次の候補 |
|---|---|---|---|---|---|
| (1) #3〜#5 module の循環と SCC の単位・組 | module の循環 17 → 17。task-core の SCC 55 単位・210 組、task-api の SCC 49 単位・261 組（before と after で同じ、組の集合も同じ）。行の辺は task-api −10（b0 628 → 618。移動の分を含む）、task-core +4（617 → 621） | module の循環 17、task-core の SCC 55 単位・210 組・行の辺 627（R4 の +6）、task-api の SCC 49 単位・228 組（−33）・行の辺 535（b0、−83） | 未達のまま（task-api の組は減ったが、循環の数と SCC の単位は減っていない） | R2・R5 は task-api の 33 組を消した。SCC の単位はどれも route の登録で handlers から呼ばれ、20 単位は `state -> handlers`、9 単位は `cos -> handlers` の経路で戻る（[R2 の進捗](../2026-11-19-arch-v4-h-r2.md) (3)）。task-core の SCC に R1〜R7 は手を入れていない | cos → handlers の残り 29 行（`cos/ops/projects.rs` 17・`cos/ops/tasks.rs` 8・`cos/ops/admin.rs` 4）を account と同じ形で移す。`state.rs` の `crate::handlers::project_controller::ControllerApiConfig` の 2 行を外す（R2 の進捗 (3)）。task-core の SCC を減らす候補は材料に無い |
| (1) #6〜#8 health の既存負債 | `size.responsibility-growth` 94 → 94、`dup.contract` 210 → 210、owner 不一致 31 → 31（finding の scope の集合も同じ） | 94・210・31（finding の scope の集合も同じ） | 未達のまま | R1〜R7 もこの 3 rule の finding を減らす変更をしていない（[replan/struct-deps.md](replan/struct-deps.md) §0） | 減らす候補は材料に無い。増えないことを回帰条件で見る。減らす task を起こすかは人が決める |
| (1) #14 `#[path]` と地図の抜け | 物理と論理が食い違う宣言は task-dispatch 9 → 9、workspace 15 → 15。`docs/architecture-map.md:67` は 3 file（check_item・review_limits・inventory）を書いていない | 地図: `docs/architecture-map.md:67` に 3 file（check_item・review_limits・inventory）が入った（R7a）。宣言の食い違いは task-dispatch 9・workspace 15 のまま | 地図の抜けは replan の後に解消（根拠: R7a の commit、地図の 67 行目に 3 file の名がある）。宣言の食い違い 9・15 は未達のまま | R7a は文書だけを直した。`#[path]` の宣言は B〜G・R1〜R7 とも変えていない（R1 が足した `#[path]` は試験の 1 件で数えない） | 宣言の食い違いを減らす候補は材料に無い（head の検査器は `#[path]` の論理 module を辺の単位に使う。「表の読み方」の checker の欄） |
| (2) `rusqlite`・SQL 文字列の production file | `rusqlite` 74 → 75（+1 は抽象の追加）、SQL 文字列 50 → 50（file の集合も同じ） | `rusqlite` 74（B0 74、±0）、SQL 文字列 49（B0 50、−1）。補助の `conn_open_files` 45（B0 48、−3。R7b の ADR D1） | 未達のまま（A の C4 の数で `rusqlite` は B0 比 ±0、SQL 文字列の −1 は移動） | R4 は task-api の自前の接続を消したが、SQL を task-core（`chat/attachments.rs`・新しい `cos_inbox.rs`）へ移したので、A の C4 の数はほぼ同じ。比較の合否は A の C4 の数で決める（R7b の ADR D2） | [R4 の進捗](../2026-11-19-arch-v4-h-r4.md)の提案: `ChatAttachmentStore` の書き込みの接続を `SqliteStore` に載せる（ADR §6 の案 B）、`state.rs` の `measure_journal_mode` を `SqliteStore` の読み取りへ移す（task-api の `Connection::open` が 0 になる） |
| (2) #9 dispatcher の参照 | self.store を書く file 44 → 38、self.config 35 → 31（依頼 scope・C3。移動なので成果に数えない）。出現数 633 → 633・218 → 220、分母 69 → 69 | 参照を持つ file 38・31 のまま。出現数 self.store 630（B0 633、−3）・self.config 212（B0 218、−6）。分母 69 | 未達のまま（出現は減ったが、参照を持つ file の数は減っていない） | R3 の手（同じ読みの 1 回化）で減る参照が少なかった（store 0.5 %・config 3.6 %）。既存の owner の操作への置き換えで数が減る参照は、棚卸しで見つからなかった（R3 の ADR D2）。`review_verdict.rs`・`work_units.rs` は ADR D6 で見送った | [R3 の進捗](../2026-11-19-arch-v4-h-r3.md)の提案: `review_verdict.rs`（順序を固定する試験を先に置く）、`work_units.rs`（遷移の純粋関数の置き場を ADR で先に決める）、`tree_units.rs` の同じ並びの helper（純減に数える規則を先に決める）、`dispatch_run.rs` の 2 関数 |
| (3) handlers ⇄ cos の単位の循環 | handlers → cos 22 → 19 行、cos → handlers 48 → 46 行（`crate::`）と `super::` 4 → 4 行。task-api の SCC の組 261 は同じ | handlers → cos 14 行（9 file）、cos → handlers 29 行（`crate::` 29・`super::` 0。3 file）、task-api の SCC の組 228 | 未達のまま（両方向の行が残り、単位の循環は解けていない） | R2 は accounts と HTTP の道具を移した。残る cos → handlers は CoS の操作が HTTP adapter の `*_op`・`*_effect` を呼ぶ形で、handlers → cos の 12 行は `Applied` の形の残り（R2 の進捗 (3)） | 1 行目と同じ（projects・tasks・admin の 29 行）。handlers → cos の 12 行は、`Applied` の形の移行（「Applied の形の残数」の節の残る領域）と合わせて消す |
| (3) task-api の `&Connection`・`&Transaction` の引数 | 14 → 15（+1 は抽象の追加） | 15（R1〜R7 で変わらない） | 未達のまま（意図した増加として記録する） | `org_write.rs:77` の `tx: &Transaction` は、CoS の `apply_checked` と同じ transaction に入るために要る（D） | 減らす候補は無い。接続を開く行の数は `conn_open_files`（R7b の ADR D1）で見る |
| (3) #10・#12 と #2 SqliteStore の impl・逆依存・Cargo の辺 | impl block 102 → 102、task-core に依存する crate 11 → 11、Cargo の依存の辺 49 → 49 | impl block 102、逆依存の crate 11、Cargo の依存の辺 49（同じ） | 未達のまま | R1〜R7 も store の impl の分け方と crate の構成を変えていない（H の base から HEAD まで Cargo.toml・Cargo.lock の差分 0） | 材料に候補は無い（G は task-core の crate 分離の利益を S1・S2 とも 0 % と見積もった） |
| (4) 変更波及（S1・S2） | S1 で変える file・単位 3 → 4（抽象の追加）、crate 2 → 2。S2 は 4 file・2 単位・1 crate のまま。import する単位は b0 で S1 92 / 369・S2 6 / 14 のまま。test-affected の対象も同じ集合 | S1 4 file・4 単位・2 crate、S2 4・2・1、test-affected の対象も同じ。S1 を直接 import する単位 76（b0、−16）・77（head、−16）、推移的に import する単位 372（b0、+3）・368（head、+3） | 未達のまま（直接 import する単位は減ったが、変える file・crate と test-affected の対象は減っていない） | R2 が HTTP の道具を `http_io` へ移し、17 module が handlers を import しなくなった。推移の +3 は R2 の `http_io`・`account_write` と R4 の `cos_inbox` の単位が足された分（[replan/loc-cost.md](replan/loc-cost.md) §0） | 材料に候補は無い。1 行目の残りの辺を消したときに、同じ手順で数え直す |
| (4) #13 修正の波及 | A の窓 194 件・複数 region 46.9 %・複数 module 69.1 %。B0 の後の窓は no-merges 1 件（0.0 %・0.0 %）、with-merges 9 件（55.6 %・55.6 %）。比較不能 | R7b: B0 の後の窓（`bc665168..main`）は no-merges 1 件・with-merges 9 件。H の branch の HEAD まででは with-merges 14 件（R1〜R5 の統合の 5 件） | 未達（窓の件数が不足。R7b の ADR D5） | B0 の後の窓は no-merges の「production を変えた commit」が下限の 30 件に満たない。with-merges は WorkUnit や task の変更を束ねた merge で、代わりにならない（[c10-window.md](c10-window.md)） | 2026-12-01T00:00:00Z（窓が閉じる）の後に c10-window.md §5 を当て直す。30 件に届かなければ、窓を延ばすか with-merges で数えるかを人が決める |
| (5) health:S2 の悪化 | 96.09 s（A 81.92 s の +17.3 %、上限 90.11 s を超える）。G が同じ host で測り直した before は 87.26 s、コード由来は `deps.violation` の CPU +2.02 s（+2.3 %） | health:S1 98.74 s（A の +14.1 %、上限 95.22 s を超える）、health:S2 100.68 s（A の +22.9 %、上限 90.11 s を超える）。H の値からは +5.5 %・+4.8 % | 未達のまま（S1 も上限を超えた） | H の値からの増えがコード由来か時間帯の host の差かは切り分けていない（同じ時間帯の before の測り直しをしていない。replan/loc-cost.md §6）。H の時点の S2 の超過は人が決定 h1 で keep と決めた | replan/loc-cost.md §6 の提案 1: G の control-health.sh の手順で before の tree と HEAD を同じ時間帯に交互に 3 回ずつ測り、rule ごとの計時でコード由来の差を切り分ける。`deps.violation` が原因なら `lang_deps.py` の `mod` 宣言の走査を 1 回にまとめる（policy の別 review。G の次の候補） |
| (5) review の時間と費用（C11） | A の窓 7.625 日・165 run・中央値 1137.5 s・$2.047。after の窓 1.875 日・30 run・514.4 s・$2.167。promo の窓 0.987 日・17 run。比較不能 | R1〜R7 は測っていない（H の measure の値のまま） | 未達のまま（比較不能） | 窓の長さと仕事の量が違う（[review-prod.md](review-prod.md) §0） | 提案 14（2026-11-25T06:00Z の後に review-prod.md §7.1 の手順で A と同じ長さの窓を比べる）。誰が比べるかは、この file の材料に決定が書かれていない |
| (5) 昇格後の設計 review と C の見積もり | promo の窓で要求 7 件・3 task・$14.33。見方 1 は +3.551 回/日・+$8.65/日（見積もり +0.670〜+4.498 回/日・+$1.37〜+$9.21/日の範囲の中）、見方 2 は 6.08 回/日・$12.59/日（上限を超える） | R1〜R7 は測っていない（H の measure の値のまま） | 要判断のまま（比較不能） | 窓が 1 日で、task がアーキ v4 の 3 件に偏る。見積もりは commit の単位、実績は計画の単位の要求で単位が違う（review-prod.md §6） | 提案 14 と同じ手順。見積もりを見直すかは人が決める（決定 final-unmet） |
| (6) LoC | production +822 行（375099 → 375921、すべて Rust）、試験 +4743 行、own_total_with_tests +5829 行 | production 376018 行（H の measure から +97、B0 から +919。すべて Rust）、試験 484793 行（+3279）、own_total_with_tests 1034966 行（+3460） | 未達のまま（増えた。依頼は一律の純減を求めていない） | R1〜R5 が足した code（R1 +26・R2 +63・R3 −7・R4 +50・R5 −35）と試験。H の回帰条件「375921 行を超えたら調べる」に当たり、R2・R4 が各進捗で内訳を調べた | 純減は求めない。production が 376018 行を超えて増えたら、増えた区分と file を調べる（回帰条件） |
| 実 prompt: planner の受入条件 | planner の 24 run で task の受入条件 0/126 件（objective の末尾は 24/24） | R1 が planner の 4 builder に受入条件の節を足した。試験 `planner_prompt_acceptance_` 3 件が pass（[replan/gate.tsv](replan/gate.tsv)） | code と試験では replan の後に解消（根拠: 試験 0 → 7 件。修正前の code で planner の 4 件が落ちる）。本番の実 prompt は未確認 | R1 を含む release が無く、本番の planner の run の prompt に受入条件が載ったかは数えていない | release の後に R1 の回帰条件 1（新しい planner の run の受入条件の欠け 0 件）を prompt-audit の規則で数える |
| 実 prompt: prompt の無い worker の run | 4 run（C の risk-rule 3・D の adr-forms 1）。開始から 2.5〜2.7 秒で lease 喪失により中止、usage null。何が渡るはずだったかは未確認 | 変わらない。R7b の ADR D4 は run ごとの spec を残す仕組みを足さず、人に残した | 未確認のまま | run の dir が作られる前に中止された。LLM は呼ばれていない（[prompt-audit.md](prompt-audit.md)） | run ごとの spec を残すかを人が決める（R7b の ADR D4 と「提案」） |
| 実 prompt: 過去の WorkUnit の run の監査 | 今の `work_units` 行と比べると 7 run が cut に見える（B の checker 4 run・C の risk-rule 3 run）。開始時の版と比べると欠け 0 | R7b の ADR D3 が、run の開始時に有効だった計画の版の `units[]` と比べる規則を決めた。その比べ方での欠けは 0（H の prompt-audit の値） | 監査の規則は replan の後に解消（根拠: ADR D3、開始時の版と比べた欠け 0）。run ごとの spec を残すかは要判断のまま | 計画の改訂が同じ id の WorkUnit の行を書き換える。spec を残す仕組みは足していない（ADR D4） | 1 つ上の行と同じ |
| 判定経路: prompt の試験の範囲 | `prompt_boundary_` が 3,000 文字の objective の末尾を確かめるのは worker の prompt だけ（未達ではない） | R1 の `prompt_full_objective_` 4 件が、planner・設計 review・final review の prompt に 3,000 文字の objective の末尾が載ることを確かめ、replan/gate.tsv で pass | replan の後に解消（根拠: `prompt_full_objective` の行が pass、4 件。review の 3 件は objective を 1,500 文字に切ると落ちる） | 残る穴: 設計 review の試験は `run_design_reviewer` を呼ばず、同じ組み立てを試験の中で行う。`synthetic_review_task` の後で objective を変える変更は検出できない（R1 の未解決事項） | [R1 の進捗](../2026-11-19-arch-v4-h-r1.md)の提案: `run_design_reviewer` の prompt の組み立てを試験から呼べる同期の関数に分けるかを決める（review-gate の高 risk path で、設計 brief が要る） |
| 安全不変条件: userns の 9 件 | sqlite_source の `db_guard` 7 件と credential の `injection_peer_uid` 2 件は、env が無いと検査の前に return して pass に数えられた（実質は未検査） | 変わらない。R6 の `nextest_passed` は通過の総数で、試験ごとの結果を持たない | 未確認のまま | `CELERIS_USERNS_TESTS` などの env を付けていない。release gate で検査まで進んだかは gate.json から読めない（R6 の未解決） | [R6 の進捗](../2026-11-19-arch-v4-h-r6.md)の提案: 該当の試験が検査まで進んだ数を gate.json の `cargo_test` に出す案を別の task で検討する（設計の判断が要る） |
| リリースゲート: B・C・D の最初の release | 3 release（B・C・D を最初に含んだもの）の gate・verify は未確認。昇格は promote ログで確かめた | R6 が gate・verify・promoted の要約を `release-records.jsonl` に残す仕組みと、gate.json の nextest の件数の欄を足した（R6 を含む release はまだ無い） | 以後の release の記録は R6 の code で直した（release の後に確かめる）。B・C・D の 3 release の gate・verify は未確認のまま | 掃除で既に消えた記録は戻らない（R6 の未解決）。B・C・D は後の 4 release にも含まれ、その gate・verify は ok | R6 の「本番への移行」の 2 点（`release-records.jsonl` に gate の行が増える、`nextest_passed` が Summary 行の passed と等しい）を、R6 を含む最初の release の後に人が確かめる |
| リリースゲート: H | H を含む release は無い（gate・verify・昇格とも未実施） | H の branch（R1〜R7 を含む）は main に入っておらず、H を含む release は無い | 未実施のまま | R1〜R6 は `crates/` と `scripts/` を変えたので、H の統合の後の release gate がこれらの変更の最初の gate になる | H の統合の後に人が `release.sh` を回し、gate.json の `ok` と R1〜R6 の試験の pass を確かめる（「リリースゲート」の節） |

## 提案

どれも提案で、H は起票していない。起票するかは人が決める。出所の D・E・F は [D の進捗](../2026-11-18-arch-v4-d.md)・[E の進捗](../2026-11-18-arch-v4-e.md)・
[F の進捗](../2026-11-18-arch-v4-f.md) の「提案」の節。struct-deps の提案 3 と prompt-audit の提案 3 は回帰条件なので「回帰条件」の節に入れた。
F の提案 4（org-write の ADR の付記の表を今の関数に合わせる）は H の struct-store が済ませた（[struct-store.md](struct-store.md) の「4. ADR の付記の表の修正」）。

replan の後: 人の決定 unmet b により、提案 14 以外は R1〜R7 が扱った（1・2 は R2、3 は R3〈`review_verdict.rs`・`work_units.rs` は見送り〉、4・5 は R4、6・8 は R1、
7 は R7b〈監査の規則は ADR D3、run ごとの spec は人に残した〉、9・10 は R6、11 は R7a、12 は R7b と R4、13 は R7b〈窓の件数が不足で未達〉、15〜17 は R5）。
結果は「replan の後」の節、残りは「未達・要判断」の節。下の表は H の report の時点の写しのまま。

| # | 提案 | 出所 | 関係する項目 |
|---|---|---|---|
| 1 | `cos/ops/admin.rs` → `handlers::accounts` の 9 行（`crate::` 5 行と `super::` 4 行。`pool_adapter`・`create_account_op` ほか）を、org と同じ形（plan と transaction の中の apply を SCC の外の module に置く）へ移す | D の次の候補、struct-deps.md の提案 2 | (1) task-api の SCC の組、(3) handlers ⇄ cos |
| 2 | `cos/operations.rs` → handlers の HTTP の道具（`ApiResult`・`Params`・`json_response`・`no_query`・`read_json`・`cluster_ids`）を SCC に入らない module に置く | D の次の候補、struct-deps.md の提案 2 | 同上 |
| 3 | dispatcher の参照を同じ手で減らす次の file: `review_verdict.rs`（store 32・config 3。先に review の順序の設計が要る）、`tree_units.rs`（store 56・config 19）、cos_chat の `root_done.rs`・`triage.rs`・`fallback.rs`（13〜24 の参照）。`work_units.rs` は本体の行が閾値 2000 を超えるので、先に WorkUnit の遷移と commit の副作用を分ける設計が要る | E の次の候補 | (2) #9 dispatcher の参照 |
| 4 | `chat/attachments.rs` と `cos/inbox.rs`（`triage_db`、`cos/triage_view.rs` の `Connection::open`）の自前の接続を、`SqliteStore` の `write_tx`・読み取りの接続へ移す（`ChatAttachmentStore` の設計と合わせる） | F の提案 1、struct-store.md の提案 | (2) `rusqlite` を書く file |
| 5 | 4 が済んだ時点で、`knowledge.rs` の有無の旗の用途と合わせて `state.chat.attachment_db_path` を `ChatState` の欄ごと消す。有無の旗は添付の保存の設定（`attachment_data_dir`）から導く | F の提案 2 | 同じ DB の file を指す値が 2 つある状態をなくす |
| 6 | planner の prompt に task の受入条件の節を足すか決める。owner は `crates/task-worker/src/claude_code/prompt.rs` の `build_plan_prompt`。守る不変条件は「planner の prompt に task の `acceptance[].text` が全部入る」 | prompt-audit.md の提案 1 | 実 prompt: planner の受入条件 0/126 |
| 7 | 過去の WorkUnit の run の監査は、run の開始時に有効だった `execution_plans` の `units[]` で行う。run ごとの spec を残すか（例: request.json の work_unit の写しを正本にする）を決める | prompt-audit.md の提案 2 | 実 prompt: 7 run の見かけの cut、prompt の無い 4 run |
| 8 | planner と reviewer（design review・final review）の prompt に長い objective を入れ、末尾まで載ることを確かめる試験を足す | gate.md の提案 | 判定経路: prompt の試験の範囲 |
| 9 | `release.sh` の gate.json の `cargo_test` に nextest の passed・skipped を入れる（いまの `passed`・`ignored` は doc test の値と nextest の skipped の和に見える） | safety.md の提案 | リリースゲートの記録の読み取り |
| 10 | release の掃除で消える gate.json・verify.json・promoted.json の要約を、promote ログのように消えない場所へ 1 行ずつ残す | safety.md の提案 | リリースゲート: B・C・D の最初の release |
| 11 | `docs/architecture-map.md:67` の `#[path]` の説明に check_item・review_limits・inventory の 3 file を足す（文書だけ） | struct-deps.md の提案 1 | (1) #14 `#[path]` と地図の抜け |
| 12 | A の C4 の正規表現に、「接続を開く行」（`Connection::open`・`TransactionBehavior`・`busy_timeout`）を持つ file の数を補助の指標として並べる（次の計測の規則の変更。H は規則を変えていない） | struct-store.md の提案 | (2) `rusqlite` を書く file |
| 13 | 修正の波及（A §7 C10）の snippet を、通常の修正が数十件たまった窓（例: B0 の後の 2 週間）に当てる | loc-cost.md §3 の次の候補 | (4) #13 修正の波及 |
| 14 | 2026-11-25T06:00Z の後（その窓に始まった reviewer run が全部終わった後）に、[review-prod.md](review-prod.md) §7.1 の手順で A と同じ長さの窓の reviewer run と設計 review を比べ、C の見積もりと並べる。誰が比べるかは決定 unmet で人が決める | review-prod.md §7 | (5) review の時間と費用、昇格後の設計 review |
| 15 | 知識の `knowledge.rs` の `accept_op`・`reject_op`・`put_page_op` を org_write の形へ移す | 「Applied の形の残数と領域ごとの移行の提案」の節、F の提案 3 | `Applied` の形の残数 |
| 16 | task-api の `lifecycle.rs` の `task_pause_op` を org_write の形へ移す（知識と別の 1 件） | 同上、F の提案 3 | 同上 |
| 17 | `skills.rs` の `put_skill_op`・`delete_skill_op` を org_write の形へ移す | 同上 | 同上 |

## 回帰条件

後の変更で次の条件が崩れたら、原因を調べる。数値が変わったら、各 md の再現コマンドを新しい `git merge-base HEAD main` と HEAD に当て直す。

`Applied` の形（D の ADR の付記、[struct-store.md](struct-store.md) §3）:

- 広い command `git grep -lE "Option<&('[a-z_]+ )?(crate::cos::operations::)?OperationAudit>" crates/task-api/src` の file の数（applied_wide）が **18 以下**であること（H の measure で 18、replan の後は 15。R5）。
  新しい操作をこの形で足さない。狭い command `git grep -l 'Option<&OperationAudit>' crates/task-api/src` は H の measure で 14、replan の後は 11。
  上限を 18 のままにするか 15 に下げるかは人が決める（[replan/struct-store.md](replan/struct-store.md) §8 の提案）。
- R5 の 3 file に旧い形が戻らないこと: `git grep -nwE 'OperationAudit|Applied' -- crates/task-api/src/knowledge.rs crates/task-api/src/lifecycle.rs crates/task-api/src/skills.rs` と
  `git grep -nE 'audited\(crate::(knowledge|lifecycle|skills)::' -- crates/task-api/src/cos/` が 0 行（R5 の回帰条件）。
- その領域の共有の操作を変える task は、同じ変更で org_write の形へ移す（ADR の付記の撤去の条件）。

G の回帰条件（health・build・test-affected の上限。[G の進捗](../2026-11-19-arch-v4-g.md)「回帰条件」、上限は [G の compare.md](../2026-11-19-arch-v4-g/compare.md) §1 の「A の +10 %」の列）:

- build・test-affected: A の baseline.md §7 の手順（G の [rules.md](../2026-11-19-arch-v4-g/rules.md)）で測った中央値が下の上限を超えたら、before（B0）を同じ host で
  3 回測り直して原因を絞る（rules.md の ## BEFORE）。
- health: 同時に並べた before と after の health の wall の差が before の +10 % を超えたら、rule ごとの計時（G の cause の手順、cause.md §5）で rule を絞る。
  `deps.violation` が原因なら cause.md §3 の修正案を policy の別 review で入れる。health:S2 はいま A の上限を超えている（96.09 s。人の決定 h1 で keep）ので、
  health は A の上限でなく、この同時に並べた差で見る。
- 変更波及: S1・S2 の file を選び直したとき、変える crate・再 build される crate・`test-affected.sh --list` の対象が (4) の表より増えたら原因を調べる
  （import する単位は b0 で S1 92 / 369・S2 6 / 14、head で S1 93 / 365・S2 6 / 14。[loc-cost.md](loc-cost.md) §7）。
- HEAD の crates/・scripts/・web/・Cargo.lock が G の測定の tree と変わったら、(5) の after は G の値として写せない。health の S1・S2 を G の rules.md の手順で
  3 回ずつ測り直す（loc-cost.md §7）。

| metric | A の中央値（s） | after の中央値（s） | 上限 = A の +10 %（s） | replan の後（s） |
|---|---|---|---|---|
| `check_cold` | 53.65 | 53.90 | 59.02 | 53.90（G の値の写し） |
| `check_warm_s1` | 42.47 | 42.64 | 46.72 | 43.08（測り直した） |
| `check_warm_s2` | 14.67 | 15.56 | 16.14 | 14.76（測り直した） |
| `test_build_cold` | 142.39 | 147.03 | 156.63 | 147.03（G の値の写し） |
| `test_build_warm_s1` | 138.69 | 138.10 | 152.56 | 138.10（G の値の写し） |
| `test_build_warm_s2` | 77.64 | 81.60 | 85.40 | 81.60（G の値の写し） |
| `health_s1` | 86.56 | 93.58 | 95.22 | 98.74（測り直した。上限を超えた） |
| `health_s2` | 81.92 | 96.09 | 90.11（いま超えている。上の health の条件で見る） | 100.68（測り直した。上限を超えた） |
| `test_affected_s1` | 893.41 | 906.03 | 982.75 | 906.03（G の値の写し） |
| `test_affected_s2` | 655.59 | 707.76 | 721.15 | 707.76（G の値の写し） |

replan の後（[replan/loc-cost.md](replan/loc-cost.md)）: health は S1・S2 とも A の上限を超えた。G の回帰条件どおり、before を同じ host で測り直して原因を絞る作業は
未実施（replan/loc-cost.md §6 の提案 1）。「G の値の写し」の 6 key は R1〜R6 の後の tree では測っていない（HEAD の crates/ が G の測定の tree と違うので、上の条件どおり測り直しが要る）。

構造（(1)〜(4)。head の検査器で数える）:

- module の循環 17、task-core の SCC の組 210、task-api の SCC の組 228（H の measure の 261 から R2・R5 で減った）を超えないこと（struct-deps.md の提案 3）。repo-health の `deps.cycle` は SCC の中の新しい組を
  fail にする（判定経路の probe a）。
- handlers → cos の 14 行と、A の C5 の式の cos → handlers の 29 行を超えないこと（H の measure の 19・50 から R2 で減った）。
  `git grep -nE 'handlers::accounts|super(::super)*::handlers' -- crates/task-api/src/cos` が 0 行のこと（R2 の回帰条件）。`git grep -nE 'handlers::org|handlers::load_org_node|org_commit' -- crates/task-api/src` が
  空のこと（D の回帰条件）。
- health の既存負債（`size.responsibility-growth` 94・`dup.contract` 210・owner 不一致 31）が増えないこと。
- F の境界: `git grep -nE 'Connection::open|TransactionBehavior|busy_timeout' -- crates/task-api/src/knowledge.rs` と
  `git grep -nE 'Fn(Once|Mut)?\(&(rusqlite::)?(Connection|Transaction)' -- crates/task-ops/src/lifecycle.rs` が 0 件のこと（1 件でも出たら F の境界が戻った印。F の回帰条件）。
- R4 の境界: `git grep -nE 'Connection::open|TransactionBehavior|busy_timeout' -- crates/task-api/src/chat/ crates/task-api/src/cos/ crates/task-api/src/knowledge.rs` と
  `git grep -n 'attachment_db_path' -- crates` が 0 行、`conn_open_files`（R7b の ADR D1）が 45 を超えないこと（R4 の回帰条件）。
- dispatcher: request/C3 の self.store の出現 630・self.config の出現 212 を超えないこと（R3 の after）。
- LoC: 同じ規則（B0 の source-size-report）で production が 376018 行（replan の後。H の measure では 375921 行）を超えて増えたら、増えた区分と file を調べる（loc-cost.md §7）。

判定経路・実 prompt・安全不変条件:

- `boundary_case_` の 6 件、`prompt_boundary_` の 3 件、`test_repo_health_*.py` の 126 件が通り続けること（[replan/gate.tsv](replan/gate.tsv) の 17 行が pass のまま）。
- R1〜R6 が足した試験が通り続けること（R3 と R7 は試験を足していない）:
  - R1: `planner_prompt_acceptance_` 3 件・`prompt_full_objective_` 4 件・`acceptance_lines_keep_review_and_work_unit_sections`（task-worker・task-dispatch）。
  - R2: account の契約試験 6 件（`cos_delivery_suite` の `cos_ops_admin_config`）と、R2 の (5) の 104 件（`llm_accounts_admin` を含む）。
  - R4: `attach_conn_` 3 件・`triage_conn_` 4 件・`attachment_flag_` 2 件（`tx_boundary_contract` と合わせた filter で 17 件）。
  - R5: `applied_contract_` 12 本（6 op × HTTP・CoS）。
  - R6: `scripts/selfdeploy/tests/release_parallel_test_gate.sh` と `release_record_summary_survives_prune.sh` が exit 0、`grep -rn release-records crates/` が 0 件。本物の検査器に gate.md §3.1 の probe を当てると
  `deps.cycle` の new の組の finding が出て exit 1、§3.2 の probe（comment 1 行）は exit 0 のままであること（main で `lifecycle -> knowledge` の組が SCC の中にできたら、
  §3 の手順で、まだ無い組を選び直す。[gate.md](gate.md) §6）。
- prompt-audit を H と同じ規則で再実行し、worker・reviewer の `objective_tail` の cut が 0 件、prompt のある run の受入条件の欠けが 0 件であること（prompt-audit.md の提案 3）。
- safety.tsv の 11 の command が pass のままであること（D・E・F の反例試験 `org_write_contract`・`tx_boundary_contract`・`cex_`・`structure_guard`・`write_set_` を含む）。
  F の `tx_boundary_contract` の 8 件ほかが落ちたら、落ちた経路の commit を git の revert で戻す（F の回帰条件）。
- review の query: A の窓の再現が 165 run・wall の中央値 1137.5 s・cost の中央値 $2.047・合計 $280.17 に一致すること（query の条件が A と同じことの確認。review-prod.md）。

## リリースゲート

判定: **H の変更を含む release gate は未実施**（replan の後も同じ）。H の commit は、いまの current の release `1680de5cd1b4`
（G の最後の commit「integrate wu/close (phase close)」）より後で、H を含む release はまだ作られていない。H の measure・verify・report の commit は docs だけだったが、
replan の R1〜R6 は `crates/`（R1・R2・R3・R4・R5）と `scripts/`（R6 の `scripts/dev/test-parallel.sh`・`scripts/selfdeploy/`）を変えた。H の branch は main に入っておらず、
R1〜R7 を含む release も無い。各子の進捗も release gate を未実施と書く。release gate は H の統合の後に人が `release.sh` で走らせる。

H の統合の後に人が行う手順（各子の進捗の「リリースゲート」の写し。詳しくは `docs/ops/selfdeploy.md`）:

1. `scripts/selfdeploy/release.sh <main の sha>` を回し、`scripts/selfdeploy/status.sh` で新しい release の gate.json の `ok: true`・`failed_step` が空を確かめる。
   gate-logs/cargo-test.log の nextest で、R1〜R6 が足した試験（「回帰条件」の節）と `org_write_contract`・`tx_boundary_contract` が pass していることも見る。
2. R6 の「本番への移行」の 2 点を確かめる: `/local/celeris/state/logs/release-records.jsonl` に gate の行が 1 行増える。gate.json の `cargo_test.nextest_passed` が
   gate-logs/cargo-test.log の Summary 行の `passed` と等しい。
3. 昇格は人が決める（`verify.sh <sha12>` → `verify.json` の `ok`・`live_ok` → `promote.sh <sha12>`）。昇格の後に R1 の回帰条件 1・2（planner の受入条件の欠け 0、
   worker・reviewer の `objective_tail` の cut 0）を prompt-audit の規則で数える。
B〜G の変更は、いまの current の release にすべて含まれ、その release は gate ok・verify ok で 2026-11-19T11:30:18Z に昇格した。

- 出所は [safety.md](safety.md) の「release gate」節（unit record）。H は release の記録を読んだだけで、`release.sh`・`verify.sh`・`promote.sh`・`status.sh` は
  走らせていない。本番の file・DB・systemd には触れていない（[review-prod.md](review-prod.md) の「再現と検査」も、統合後の release の gate は未実施と書く）。
- 読んだ記録: `~/.local/celeris/releases/<sha12>/` の manifest.json・gate.json・verify.json・promoted.json・promote_failed.json、`releases/.build/<sha12>/gate.json`、
  current と previous の symlink、`/local/celeris/state/logs/promote-*.log`（2026-11-19 に読んだ）。promote_failed.json と rollback のログは無かった。

B〜G ごとの結果（safety.md の表の写し）:

| task | 最初に含んだ release | その release の gate | verify | 昇格 | いまの current（1680de5cd1b4） |
|---|---|---|---|---|---|
| B | 48b700ede57f（「integrate wu/closeout (phase closeout)」2026-11-17） | 未確認 | 未確認 | した（2026-11-17T22:28:42Z） | 含む。gate ok・verify ok・昇格済み |
| C | f85df0209504（「integrate wu/loc-fix (phase fix)」2026-11-18） | 未確認 | 未確認 | した（2026-11-18T12:18:41Z） | 含む。同上 |
| D | eb1244bb5e8d（D の branch への merge、2026-11-18） | 未確認 | 未確認 | した（2026-11-18T21:10:04Z） | 含む。同上 |
| E | 1c69c955f0fb（E の branch への merge、2026-11-18） | ok | ok | した（2026-11-18T23:25:35Z） | 含む。同上 |
| F | 7014d24222c4（F の branch への merge、2026-11-19） | ok | ok | した（2026-11-19T05:35:55Z） | 含む。同上 |
| G | 1680de5cd1b4（「integrate wu/close (phase close)」2026-11-19） | ok | ok | した（2026-11-19T11:30:18Z） | この release 自身 |
| H（R1〜R7 を含む） | 無し | 未実施 | 未実施 | 未実施 | 含まない |

昇格の連鎖（2026-11-17〜11-19。safety.md の表の写し。「含む」は `git merge-base --is-ancestor <B〜G の最後の commit> <release の commit>` が 0）:

| release | release の commit（件名・日時 UTC） | 昇格（UTC、from） | 含む task | gate | verify |
|---|---|---|---|---|---|
| 48b700ede57f | 「integrate wu/closeout (phase closeout)」2026-11-17T21:45:07Z | 2026-11-17T22:28:42Z、from f549b50b7ccc | B | 未確認 | 未確認（promote ログに `verify.live_ok=true`） |
| b2fe0a5e920f | 「repair: PathClass.invariants の進捗に test-affected の結果を書く」2026-11-18T02:25:45Z | 2026-11-18T03:16:41Z | B | 未確認 | 未確認（同） |
| f85df0209504 | 「integrate wu/loc-fix (phase fix)」2026-11-18T10:12:48Z | 2026-11-18T12:18:41Z | B・C | 未確認 | 未確認（同） |
| ed43a4abb7bf | 「integrate wu/fix-test (phase fix)」2026-11-18T13:42:24Z | 2026-11-18T15:25:24Z | B・C | 未確認 | 未確認（同） |
| 3033e0228853 | 「Merge commit '…' into celeris/01M7Q70NDQAK1KKXHY6WT027QZ」2026-11-18T16:18:10Z | 2026-11-18T17:17:11Z | B・C | 未確認 | 未確認（同） |
| eb1244bb5e8d | 「Merge commit '…' into celeris/01M7MZTKB4RYM80S6CT3RST2N9」2026-11-18T20:08:59Z | 2026-11-18T21:10:04Z | B・C・D | 未確認 | 未確認（同） |
| 1c69c955f0fb | 「Merge commit '…' into celeris/01M7MZTKB6GQ241F6JJYSPJNS1」2026-11-18T22:31:07Z | 2026-11-18T23:25:35Z、from eb1244bb5e8d | B〜E | 記録 ok（nextest 8515 passed） | 記録 ok |
| bb772ffca8ec | 「進捗: rejected-worker-decisions」2026-11-19T00:08:00Z | 2026-11-19T01:25:29Z、from 1c69c955f0fb | B〜E | 記録 ok（nextest 8517 passed） | 記録 ok |
| 7014d24222c4 | 「Merge commit '…' into celeris/01M7MZTKB8A4NJQEEA35Q4G5JV」2026-11-19T04:30:48Z | 2026-11-19T05:35:55Z、from bb772ffca8ec | B〜F | 記録 ok（nextest 8525 passed） | 記録 ok |
| 1680de5cd1b4 | 「integrate wu/close (phase close)」2026-11-19T10:37:13Z | 2026-11-19T11:30:18Z、from 7014d24222c4。いまの current | B〜G | 記録 ok（nextest 8525 passed） | 記録 ok |

残っている 4 release（1c69c955f0fb・bb772ffca8ec・7014d24222c4・1680de5cd1b4）の記録の中身（4 つとも同じ形）:

- manifest.json: `gate_ok: true`、`schema_version: 126`、web の 7 段が ok、browser_ledger ok。
- gate.json: `ok: true`・`failed_step` 空。17 段（cargo-workspace-clean・cargo-fmt-check・control-ts-install・control-ts-bundle・script-bundle-verify・cargo-test・
  script-bundle-tests・cargo-clippy・source-size-report・cargo-build・web の 7 段）がどれも exit 0。cargo_test は `nextest_exit 0`・`doctest_exit 0`・`failed 0`・`userns: true`。
  nextest は gate-logs/cargo-test.log の Summary 行で 8515〜8525 passed・22 skipped・fail 0。
- verify.json: `ok: true`・`live_ok: true`。検査 1（start-and-migrate）・2（counts-match）・3（main-gets）・5（n-1-compat）・6（smoke）・4d（web-app-start）の 6 件がどれも ok。
- promoted.json: `status: succeeded`・`exit_code 0`・`mode: live`。actor は 4 件とも `external_operator:claude-code-ops-session`。

B・C・D の最初の release の gate・verify が未確認の理由: release の dir と `releases/.build/<sha12>/` がどちらも無く、gate.json・verify.json を読めない。
`release.sh` の掃除が current・previous・検証済みの新しい 3 件だけを残すので、後の release を作ったときに消えたとみられる。
promote ログに 3 件とも `promoted <sha12> (mode=live)` があり、`promote.sh` は verify.json の `ok` が真でなければ止まり、`verify.sh` は gate.json の `ok` が
真でなければ止まるので、gate と verify は ok だったと推定できる（記録そのものは読めていない）。B・C・D は後の 4 release にも含まれ、その gate・verify・昇格は記録で確かめた。

気づいた点（safety.md。直していない）: gate.json の `cargo_test.passed` は 4 release とも 3、`ignored` は 24 で、nextest の通過数を含まない
（doc test の通過数 3 と、nextest の skipped 22 と doc test の ignored 2 の和に一致する）。nextest の件数は gate log の Summary 行にだけある。
replan の後: R6 がこの原因（`scripts/dev/test-parallel.sh` が注記付きの Summary 行の `passed` の組を取れなかった）を直し、`cargo_test` に `nextest_passed`・`nextest_skipped`・
`doc_passed`・`doc_ignored` を足した。R6 を含む release はまだ無いので、本番の gate.json では確かめていない。

## 再現コマンド

7 つの WorkUnit の md の再現の節の command を写した。この file のためには実行していない（数は各 md の実行の記録から写した）。
repo の root で実行する。`$TMPDIR` は run の一時 dir、生の出力は各 WorkUnit の run の成果物 dir（repo の外）にある。
本番 DB を読む prompt-audit と review-prod の command は、DB を `mode=ro` の URI でだけ開く。release の記録を読む safety の command は読むだけで、
`release.sh`・`verify.sh`・`promote.sh` は含まない。

### struct-deps

[struct-deps.md](struct-deps.md) §5。`ART` はその run の成果物 dir、`G` は G の scope の成果物の `raw/`（`c2.py` の元）。

```sh
B0=bc665168c9caf1898adc4c611404bbee056daf9a; H=$(git merge-base HEAD main); O=$TMPDIR/out; mkdir -p $O
R=$TMPDIR/rh-B0; mkdir -p $R
git archive $B0 scripts/dev/repo-health scripts/dev/source-size-report.py scripts/dev/source-size-report.toml | tar -x -C $R
# C1（A §7 C1。約 1 分 36 秒ずつ。3 本を同時に走らせた）
python3 scripts/dev/repo-health/repo_health.py --repo . --base $B0 --head $B0 --policy-version 1 --out $O/c1-head-B0.json
python3 scripts/dev/repo-health/repo_health.py --repo . --base $H --head $H --policy-version 1 --out $O/c1-head-H.json
python3 $R/scripts/dev/repo-health/repo_health.py --repo . --base $B0 --head $B0 --policy-version 1 --out $O/c1-b0-B0.json   # arch-map.* は checker_missing
python3 -c "import json,collections,sys;r=json.load(open(sys.argv[1]));c=collections.Counter(f['rule_id'] for f in r['findings']);print({k:c[k] for k in ('deps.cycle','size.responsibility-growth','dup.contract','arch-map.wrong_owner')})" $O/c1-head-H.json
#   scope の集合の比較: findings を rule_id ごとに json.dumps(f['scope'],sort_keys=True) の Counter にして before と after の差を取る（差 0）
# C2（A §7 C2 の snippet = G の raw/c2.py。第 4 引数で profile を読む rev を指定できるようにした写し）
sed "s/^snap=snapshot.resolve('.',B,B)/PB=sys.argv[4] if len(sys.argv)>4 else B; snap=snapshot.resolve('.',PB,B)/" $G/c2.py > $TMPDIR/c2.py
python3 $TMPDIR/c2.py scripts/dev/repo-health $B0 $O/edges-head-B0.json
python3 $TMPDIR/c2.py scripts/dev/repo-health $H  $O/edges-head-H.json
python3 $TMPDIR/c2.py $R/scripts/dev/repo-health $B0 $O/edges-b0-B0.json
python3 $TMPDIR/c2.py $R/scripts/dev/repo-health $H  $O/edges-b0-H.json $B0      # profile は B0 から読む（§2 の 1）
#   出力の 'cycles' の数 = module_cycles、'crates/task-core/src/store' を含む SCC = tc_scc_*、'crates/task-api/src/handlers' を含む SCC = ta_scc_*、
#   'handlers->cos'・'cos->handlers' の行 = repo-health の辺。行の辺の差は (src, dst_unit) ごとの Counter の差で見た
# C5（A §7 C5）
X='(^|/)(tests?|benches|examples)/|(^|/)(tests|[^/]*_tests?|test_support)\.rs$|_fixture\.rs$'
for B in $B0 $H; do
  git grep -nE 'crate::cos::' $B -- crates/task-api/src/handlers crates/task-api/src/handlers.rs | grep -vE "$X" | wc -l        # 22 / 19
  git grep -nE '(crate|super(::super)*)::handlers' $B -- crates/task-api/src/cos | grep -vE "$X" | wc -l                         # 52 / 50
  git grep -nE 'crate::handlers' $B -- crates/task-api/src/cos | grep -vE "$X" | wc -l                                         # 48 / 46
  git grep -nE 'super(::super)*::handlers' $B -- crates/task-api/src/cos | grep -vE "$X" | wc -l                               # 4 / 4
  git grep -n '#\[path' $B -- crates | grep -vE "$X"     # path_attr_files の元の一覧（§2 の 4 の規則で分ける）
done
# C7（cargo metadata。workspace_members の dependencies のうち path のある workspace の crate を kind ごとに数え、normal の辺で強連結成分を取る）
git diff --stat $B0 $H -- Cargo.toml Cargo.lock 'crates/*/Cargo.toml'        # 差分 0
cargo metadata --format-version 1 --offline > $O/meta-H.json                   # members 15、edges 49（normal 35・dev 14）、crate_cycles 0
# 地図の確認（§3）
for f in design_gate design_verdict plan_adoption policy_review worker_profile check_item review_limits inventory; do
  echo "$f $(git show $H:docs/architecture-map.md | grep '#\[path\]' | grep -c $f)"; done                      # 後ろの 3 件が 0
# profile の差（§2 の 1）
python3 -c "import subprocess,tomllib; ..."   # B0 と after の celeris.toml を tomllib で読み、差のある key を出す（review.* だけ）
```

### struct-store

[struct-store.md](struct-store.md) §6。`B` を before（B0）と after（`git merge-base HEAD main`）の sha に替えて 2 回走らせる。

```sh
B=bc665168c9caf1898adc4c611404bbee056daf9a   # after は B=$(git merge-base HEAD main)
X='(^|/)(tests?|benches|examples)/|(^|/)(tests|[^/]*_tests?|test_support)\.rs$|_fixture\.rs$'; P='^crates/(campaign|routing-eval|jev-classifier)/'
# C4 rusqlite・SQL 文字列（task-core/src/store/ を除く production file）
git grep -l 'rusqlite' $B -- 'crates/*.rs' | sed "s/^$B://" | grep -vE "$X" | grep -vE "$P" | grep -v '^crates/task-core/src/store/' | wc -l
git grep -lE '"[^"]*\b(SELECT .* FROM|INSERT (OR [A-Z]+ )?INTO|UPDATE [a-z_]+ SET|DELETE FROM)\b' $B -- 'crates/*.rs' | sed "s/^$B://" | grep -vE "$X" | grep -vE "$P" | grep -v '^crates/task-core/src/store/' | wc -l
# C6 &Connection の引数（crate ごと。store/ は ^crates/task-core/src/store/ で絞る）と Fn の closure
git grep -nE '\b[a-z_]+: &(mut )?(rusqlite::)?(Connection|Transaction)\b' $B -- 'crates/*.rs' | sed "s/^$B://" | grep -vE "$X" | grep -vE "$P" > $TMPDIR/ca
for c in task-core task-api task-ops task-core/src/store; do echo "$c files=$(grep "^crates/$c/" $TMPDIR/ca | cut -d: -f1 | sort -u | wc -l) occ=$(grep -c "^crates/$c/" $TMPDIR/ca)"; done
git grep -nE 'Fn(Once|Mut)?\(&(mut )?(rusqlite::)?(Connection|Transaction)' $B -- 'crates/*.rs' | sed "s/^$B://" | grep -vE "$X" | grep -vE "$P" | cut -d/ -f2 | sort | uniq -c
# C6 SqliteStore: impl block と pub method は A の baseline.md §7 C6 の git grep と python の snippet をそのまま使う（path 付きは同 §7 の注の正規表現）
# C7 逆依存: cargo metadata --format-version 1 --offline の workspace_members の normal 依存を逆にたどる（before は git archive した tree で実行）
# C3・E1 dispatcher: E の measure-commands.md の C3（request と logical.txt）と E1 の snippet をそのまま使う
# Applied の形（D の付記）
git grep -l 'Option<&OperationAudit>' $B -- crates/task-api/src | wc -l
git grep -lE "Option<&('[a-z_]+ )?(crate::cos::operations::)?OperationAudit>" $B -- crates/task-api/src | wc -l
# 補助: file ごとの関数（type Audit の行は除く）
for f in $(git grep -lE "Option<&('[a-z_]+ )?(crate::cos::operations::)?OperationAudit>" $B -- crates/task-api/src | sed "s/^$B://"); do
  git show $B:$f | awk -v f=$f '/^[[:space:]]*(pub(\([a-z]+\))? )?(async )?fn [a-z_0-9]+/ {match($0,/fn [a-z_0-9]+/); fn=substr($0,RSTART+3,RLENGTH-3)}
    /^type Audit</ {next}
    /Option<&('"'"'[a-z_]+ )?(crate::cos::operations::)?OperationAudit>/ || /: Audit<'"'"'_>/ {print f, fn}'; done | sort -u | wc -l
```

### loc-cost

[loc-cost.md](loc-cost.md) §5。`B=bc665168c9caf1898adc4c611404bbee056daf9a`、`H=HEAD`。`c7b.py`・`c10.py` は A の baseline.md §7 の snippet の写し。

```sh
# (6) LoC（A §7 C12。分類規則は base に固定）
git show $B:scripts/dev/source-size-report.py > $TMPDIR/ssr.py; git show $B:scripts/dev/source-size-report.toml > $TMPDIR/ssr.toml
for r in $B HEAD; do python3 $TMPDIR/ssr.py --root . --config $TMPDIR/ssr.toml --exclusive-classes --rev $r --format json --out $TMPDIR/ssr-$r.json; done
#   views.product_only.lines / rust_lines、class_totals.external_test + inline_test、views.own_total_with_tests.lines / rust_lines、
#   言語別は matrix.<class>.<lang>.lines。config_sha256 が 2 つで同じことを確かめる

# (4) 変える file・単位・crate
. agent-docs/progress/2026-11-19-arch-v4-g/scenarios.env   # S1_FILES（4 file）・S2_FILES（4 file）
#   単位 = crates/<c>/src/<第 1 要素>（.rs を除く）、crate = crates/<c>

# (4) C7b（A §7 C7b の snippet を $TMPDIR/c7b.py に写したもの。head の検査器）
python3 $TMPDIR/c7b.py HEAD S1h:crates/task-api/src/org_write,crates/task-api/src/handlers,crates/task-api/src/cos,crates/task-core/src/store \
  S1:crates/task-api/src/handlers,crates/task-api/src/cos,crates/task-core/src/store S2:crates/task-dispatch/src/dispatcher,crates/task-dispatch/src/run_settlement
python3 $TMPDIR/c7b.py $B  <同じ引数>          # before'（head の検査器 × B0）
# b0 の検査器: B0 の検査器と source-size-report を出し、sys.path を差し替え、snapshot.resolve('.', B0, 測る rev) にした写し（c7b-b0.py）
mkdir -p $TMPDIR/rh-B0 && git archive $B scripts/dev/repo-health scripts/dev/source-size-report.py scripts/dev/source-size-report.toml | tar -x -C $TMPDIR/rh-B0
sed "s#sys.path.insert(0,'scripts/dev/repo-health')#sys.path.insert(0,'$TMPDIR/rh-B0/scripts/dev/repo-health')#; s#snap=snapshot.resolve('.',B,B)#snap=snapshot.resolve('.',B.split('@')[-1],B.split('@')[0])#" $TMPDIR/c7b.py > $TMPDIR/c7b-b0.py
python3 $TMPDIR/c7b-b0.py $B <同じ引数>; python3 $TMPDIR/c7b-b0.py HEAD@$B <同じ引数>   # 引数の「測る rev@profile の rev」

# (4) test-affected の対象（comment の probe。G の probe-comment.sh の写し）
bash probe-comment.sh $S1_FILES; git diff --stat -- crates; bash scripts/dev/test-affected.sh --list; git checkout -- $S1_FILES
bash probe-comment.sh $S2_FILES; git diff --stat -- crates; bash scripts/dev/test-affected.sh --list; git checkout -- $S2_FILES
git status --porcelain -- crates                              # 空であること

# 修正の波及（A §7 C10 の snippet を baseline.md から $TMPDIR/c10.py に写したもの。約 60〜85 s と 1〜2 s）
for m in no-merges with-merges; do python3 $TMPDIR/c10.py $TMPDIR/ssr.py $TMPDIR/ssr.toml 2026-10-28T00:00:00Z $m $B; done          # A の窓
for m in no-merges with-merges; do python3 $TMPDIR/c10.py $TMPDIR/ssr.py $TMPDIR/ssr.toml 2026-11-17T00:00:00Z $m "$B..HEAD"; done   # B0 の後
#   commit ごとの内訳は c10.py の集計の直前に print を 1 行足した写しで出した

# (5) G の測定の tree と HEAD の source の差分（0 行なら G の値を写す）
G=$(git log --format=%H --grep='^arch-v4 G scope' -1 main)^
git diff --name-only $G HEAD -- crates scripts web Cargo.lock Cargo.toml | wc -l      # 0
git rev-parse $G:crates HEAD:crates $G:scripts HEAD:scripts $G:web HEAD:web           # 組ごとに同じ値
# G の TSV の after の中央値
python3 -I -c "import csv,statistics,collections;d=collections.defaultdict(list)
for f in ['scope','build-check','build-test','ta-s1','ta-mix','ta-s2']:
  for r in csv.DictReader(open(f'agent-docs/progress/2026-11-19-arch-v4-g/{f}.tsv'),delimiter='\t'):
    if r['rev']=='after' and r['valid']=='yes': d[(r['metric'],r['scenario'])].append(float(r['seconds']))
[print(k,statistics.median(v),v) for k,v in sorted(d.items())]"
```

### gate

[gate.md](gate.md) §7。probe はそれぞれ検査器を走らせた直後に `git checkout --` で戻す（commit しない）。

```sh
cargo nextest run -p task-dispatch -E 'test(/boundary_case_/)' --status-level pass --final-status-level pass
cargo nextest run -p task-worker -E 'test(/prompt_boundary_/)' --status-level pass --final-status-level pass
python3 -I -m unittest discover -s scripts/tests -p 'test_repo_health_*.py'
B=$(git merge-base HEAD main)
F=crates/task-api/src/lifecycle.rs
# probe a
sed -i 's/^use crate::human_epoch::HumanEpoch;$/&\nuse crate::knowledge::KnowledgeItem;/' $F
python3 scripts/dev/repo-health/repo_health.py --repo . --base "$B" --head worktree --policy-version 1 --out $TMPDIR/a.json; echo $?   # 1
git checkout -- $F
# probe b
sed -i 's|^use crate::human_epoch::HumanEpoch;$|&\n// probe: comment only (gate-cases)|' $F
python3 scripts/dev/repo-health/repo_health.py --repo . --base "$B" --head worktree --policy-version 1 --out $TMPDIR/b.json; echo $?   # 0
git checkout -- $F
```

### prompt-audit

[prompt-audit.md](prompt-audit.md) の「再現」。`audit.py` の本文（66 行の python）は同じ節にあり、作業場所の外（`$TMPDIR`）に写して使う。

```sh
PA_DETAIL=$TMPDIR/detail.json python3 -I "$TMPDIR/audit.py" > agent-docs/progress/2026-11-19-arch-v4-h/prompt-audit.tsv   # exit 0、201 行と header
# kind ごとの集計
awk -F'\t' 'NR>1{k[$3" "$5]++; if($5!="missing"){t[$3]+=$6; f[$3]+=$7}} END{for(x in k) print x, k[x]; for(x in t) print x, "acceptance", f[x]"/"t[x]}' \
  agent-docs/progress/2026-11-19-arch-v4-h/prompt-audit.tsv | sort
```

### safety

[safety.md](safety.md) の不変条件ごとの節と「release gate」節。安全不変条件の 11 行（safety.tsv の command 列。§安全不変条件 の code block と同じ）:

```sh
# sqlite_source（169）
cargo nextest run -p task-core -E 'test(/^store::(tests|migration_harness_tests|client_open_tests|legacy::tests)::/) | test(/^transition::tests::/)' && cargo nextest run -p task-worker -E 'test(/^db_guard::tests::/)'
# events_append_only（13）
cargo nextest run -p task-core -E 'test(/^store::tests::append_event_/) | test(/^store::tests::apply_transition_.*appends/) | test(/^store::tests::create_task_inserts_task_and_events_atomically$/) | test(append_only)'
# lease_atomicity（51）
cargo nextest run -p task-core -E 'test(/^store::lease_reclaim_tests::/) | test(/^store::tests::(acquire_lease_|acquire_work_unit_lease_|renew_lease_|apply_transition_releases_lease_)/)' && cargo nextest run -p task-dispatch -E 'test(/^dispatcher::tests::(cex_lease_restart|cex_same_tick|ownerless_runs|orphan_takeover)::/) | test(/^run_supervisor::tests::/)'
# replay（14）
cargo nextest run -p task-ops -E 'test(/^replay::tests::/)'
# project_controller_stop（14）
cargo nextest run -p task-dispatch -E 'test(/^dispatcher::tests::project_wake::effects::project_wake_pending_/) | test(/^dispatcher::tests::project_wake::project_wake_(human_races_|off_is_inert$)/) | test(/project_recovery_k37_/) | test(/^dispatcher::tests::project_wake::replay::project_controller_replay_(paused_project|cancelled_project|invalid_proposals_stop)$/)'
# rust_ts_canary（16）
cargo nextest run -p task-dispatch -E 'test(canary)'
# acl（28）
cargo nextest run -p task-api -E 'binary(auth_and_guards) | (kind(lib) & test(/^(principal|middleware)::tests::|^handlers::tests::resolve_principal_/)) | (binary(browser_eval_surface_suite) & test(/^api_principals::/))'
# credential（78）
cargo nextest run -p celeris-credentiald -E 'binary(broker) | binary(injection_peer_uid) | binary(prod_admission) | binary(injection_ipc)' && cargo nextest run -p task-worker -E 'test(browser_credential)' && cargo nextest run -p task-api -E '(kind(lib) & test(/^secrets::tests::/)) | (binary(run_worker_suite) & test(/^secrets_admin::/))'
# cos_audit（16）
cargo nextest run -p task-api -E 'binary(cos_operations) | (kind(lib) & test(/^cos::operations::/)) | (binary(cos_delivery_suite_a) & test(/^cos_auth::/))'
# selfdeploy_authority（83）
cargo nextest run -p celeris -E '(kind(lib) & test(/^(instance|releases)::/)) | binary(unpromoted_release) | binary(instance_handoff) | (binary(cos_delivery_suite) & test(/^releases_api::/))' && cargo nextest run -p task-api -E '(kind(lib) & test(/^releases::tests::/)) | (binary(browser_eval_surface_suite) & test(/^api_principals::promotion_audit_/))'
# arch_v4_counterexamples（81）
cargo nextest run -p task-api --test cos_delivery_suite org_write_contract && cargo nextest run -p task-api --test cos_delivery_suite tx_boundary_contract && cargo nextest run -p task-dispatch -E 'test(store_access_guard) | test(structure_guard) | test(cex_) | test(write_set_)'
```

release の記録（読むだけ。safety.md の「読んだ command と出力の要点」と「人が確かめる読み取りだけの command」）:

```sh
ls -la ~/.local/celeris/releases/
readlink /local/celeris/state/current /local/celeris/state/previous
python3 -I -m json.tool ~/.local/celeris/releases/<sha12>/{manifest,gate,verify,promoted}.json
ls ~/.local/celeris/releases/*/promote_failed.json
ls -lt ~/.local/celeris/releases/.build/
grep -E 'release=|current  ->|promoted [0-9a-f]{12}' /local/celeris/state/logs/promote-2026111{7,8,9}-*.log
ls /local/celeris/state/logs | grep rollback
grep -E '^\s+Summary \[' ~/.local/celeris/releases/<sha12>/gate-logs/cargo-test.log
git merge-base --is-ancestor <B〜G の最後の commit> <release の commit>; echo $?   # 0 なら含む
# B・C・D の最初の release（人が確かめる）
ls -la ~/.local/celeris/releases/48b700ede57f ~/.local/celeris/releases/f85df0209504 ~/.local/celeris/releases/eb1244bb5e8d ~/.local/celeris/releases/.build/
grep -vE 'preview:' /local/celeris/state/logs/promote-20261117-222401.log /local/celeris/state/logs/promote-20261118-121753.log /local/celeris/state/logs/promote-20261118-210838.log
git -C ~/workspace/agent-platform show 48b700ede57f:scripts/selfdeploy/promote.sh | grep -n 'refusing to promote'
```

### review-prod

[review-prod.md](review-prod.md) §2。道具（`review_prod.py`・`tsv_gen.py`）はその run の成果物 dir の `raw/` にある使い捨てで、query の SQL は同じ節にある。
A と同じ長さの窓での比べ直しは、2026-11-25T06:00Z の後に第 2 引数を `2026-11-25T06:00` にして走らせる（同 §7.1）。

```sh
ART=/local/celeris/data/workspaces/01M7MZTKBPZPQE4XN451F8X84F/wu/review-prod/artifacts
python3 -I $ART/raw/review_prod.py /local/celeris/data/db/celeris.sqlite3 2026-11-19T12:00 2026-11-18T12:18:40 $ART/raw/review-prod.json
python3 -I $ART/raw/tsv_gen.py $ART/raw/review-prod.json > agent-docs/progress/2026-11-19-arch-v4-h/review-prod.tsv
```
