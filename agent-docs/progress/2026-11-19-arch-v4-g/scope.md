---
title: アーキ v4 G scope — after の S1・S2 の file を選び直し、変更波及・health 3 回・review の費用を測る
tasks: [01M7MZTKBKXKEVBWS87PS5J9NK]
status: done
updated: 2026-11-19
completed: 2026-11-19
---
# scope（WorkUnit）

after は `git merge-base HEAD main`（D・E・F が入った main。この WorkUnit の `$CELERIS_WU_BASE` と同じ tree）。
before は A の baseline.md（main bc665168、A の run の成果物 `artifacts/baseline.md`）の値。手順と規則は A の §5・§6・§7 をそのまま使った。
測定の共通規則は [rules.md](rules.md)、シナリオの file は [scenarios.env](scenarios.env)、health の各回は [scope.tsv](scope.tsv)。

## 0. 結論

- S1 の file は 4 個（D の 4 file 案）、S2 は A と同じ 4 個のまま。
- 変更波及（§5.2 の表）は、S1 で file と単位が 3 → 4 に増え、ほかの値（crate・import する単位・再 build の crate・`--list`）は A と同じか、
  検査器の変更（B）による差だけだった。S2 は A と同じ（dispatcher の SCC の大きさ 3 → 4 は B の検査器の変更による）。
- health（const の probe、3 回とも開始時の 1 分 load 4 以下）の中央値は S1 93.58 s（A 86.56 s、+8.1 %）、S2 96.09 s（A 81.92 s、+17.3 %）。
  user の CPU 時間の中央値は S1 71.19 s（A 69.45 s、+2.5 %）、S2 71.86 s（A 69.46 s、+3.5 %）。S2 は +10 % を超えたので compare で印が付く見込み。
  原因（コード由来か host 由来か）は未確認で、cause の leaf が絞る。
- review（C11）は A の窓を再現できた（165 run、wall の中央値 1,137.5 s、cost の中央値 $2.047）。after は 2026-11-17T15:00 以降の
  38 時間の部分窓しかなく（28 run、wall の中央値 669.1 s、cost の中央値 $2.267）、**未達/要判断**。同じ長さ（7 日 15 時間）の窓が揃うのは
  2026-11-25T06:00Z。

## 1. after の file の選び直し（A §5.1 の規則）

規則: 「同じ意味の変更を after の HEAD で行うときに変える file を同じ規則で選び直す。file が移ったら移った先を数える」。

### S1「新しい CoS 業務操作を足す」（org の監査つきの書き込み操作を HTTP と CoS の両方から呼ぶ）

| 案 | file | 判断 |
|---|---|---|
| 4 file（採用） | `crates/task-api/src/org_write.rs`・`crates/task-api/src/handlers/org.rs`・`crates/task-api/src/cos/ops/admin.rs`・`crates/task-core/src/store/org.rs` | 採用 |
| 3 file（A と同じ組） | 上から `org_write.rs` を除く | 参考の値だけ出す |

採用の理由:
- D は plan と tx の中の書き込みを `handlers/org.rs` から `org_write.rs` に移した（`OrgWrite`・`plan_create`・`plan_patch`・`plan_delete`・`apply`）。
  新しい操作は `OrgWrite` の variant・`plan_*`・`apply` の分岐を `org_write.rs` に足さないと作れない。A の規則の「移った先を数える」に当たる。
- `handlers/org.rs` には HTTP の handler（`org_write::plan_*` → `store.write_tx(|tx| org_write::apply(..))`）が残るので、HTTP の route を足すには変える。
- `cos/ops/admin.rs` には route 表（`("PATCH", "/api/v1/org/{id}", "org.update")` など）と CoS の分岐が残る。
- `store/org.rs` には `org_upsert_tx`・`org_delete_tx` のような tx の中の書き込みの method がある。新しい書き込みはここに method を足す（A と同じ）。
- 3 file の組は `org_write.rs` を変えないので、after では同じ意味の変更にならない。build の時間を A と同じ file の組で並べたいときの参考にだけ使う
  （D の timing.md は両方の組で warm check を測った: 4 file 43.95 s、3 file 43.58 s）。

### S2「WorkUnit の終了処理を変える」

A の 4 file のまま: `crates/task-dispatch/src/run_settlement/work_unit.rs`（判断）・`crates/task-dispatch/src/run_settlement.rs`（提案の型）・
`crates/task-dispatch/src/dispatcher/worker_finish.rs`（適用）・`crates/task-dispatch/src/dispatcher/work_units.rs`（WU の行の書き込み）。

確かめたこと:
- bc665168 から after までに `run_settlement/`・`run_settlement.rs`・`worker_finish.rs` は変わっていない（`git diff --stat bc665168 HEAD -- <path>` が空）。
- `work_units.rs` は E で +51 −58 行変わったが、変わったのは write-set と target-leaf（`auto_leaf_decision` の入口）で、WU の run の終わり方の適用は
  `worker_finish.rs` の `run_settlement::settle_work_unit`（514 行付近）と `work_units.rs` の WU の行の書き込みのまま。移った先は無い。

## 2. 変更波及（A §5.2 の表）

列: A = A の baseline（旧い検査器 × bc665168）、before' = after の検査器 × bc665168（B の検査器の変更の分を分けるため）、after = after の検査器 × after。

| | S1 A | S1 before' | S1 after（4 file） | S1 after 参考（3 file） | S2 A | S2 before' | S2 after |
|---|---|---|---|---|---|---|---|
| 変える production file | 3 | 3 | **4** | 3 | 4 | 4 | 4 |
| 変える単位（repo-health の単位） | 3 | 3 | **4**（＋`task-api/src/org_write`） | 3 | 2 | 2 | 2 |
| 変える crate | 2 | 2 | 2 | 2 | 1 | 1 | 1 |
| 単位が属する SCC の大きさ | 49・49・55 | 49・49・55 | 1・49・49・55（`org_write` は SCC の外） | 49・49・55 | 3・1 | 4・1 | 4・1 |
| 直接 import する単位（C7b） | 92 | 93 | 93 | 94 | 6 | 6 | 6 |
| 推移的に import する単位 / package（C7b） | 369 / 12 | 365 / 12 | 365 / 12 | 366 / 12 | 14 / 4 | 14 / 4 | 14 / 4 |
| 再 build される crate（C7、通常依存の逆依存＋自身） | task-core の逆依存 11 ＋ 1 | — | 11 ＋ 1（同じ 11 crate） | 同左 | task-dispatch の逆依存 4 ＋ 1 | — | 4 ＋ 1（同じ 4 crate） |
| test-affected の対象（`--list`、comment の probe） | 14 crate ＋ web ＋ ts-verify | — | A と同じ 14 crate ＋ web typecheck+unit ＋ test-ts-verify | 同左 | 5 crate | — | A と同じ 5 crate |

- SCC の大きさの詳細（after）: task-api の SCC は 49 単位・261 組・622 本（A 628 本、before' 632 本）、task-core の SCC は 55 単位・210 組・621 本（A 617 本、before' 617 本）、
  dispatcher の SCC は 4 単位（dispatcher・health_review・review・undeclared_artifacts）・7 組・51 本で before' と同じ。
  A の 3 単位から 4 単位になったのは、B の検査器が `#[path]` の file を論理 module で数え、health_review が入ったため（コードの変更ではない）。
- C7b の S1 の単位ごとの直接の importer（after、4 file）: cos 22・handlers 40・org_write 2・store 61。3 file の参考で 94 になるのは、
  `org_write` が store を import する単位として数えに入るため（D の metrics.md と同じ観察）。
- Cargo の依存（C7）: workspace の 15 crate、通常依存 35 辺（A と同じ）、dev 依存 12 辺（A は 14。数え方の違いか依存の変化かは未確認）。
  task-core の逆依存 11 crate・task-dispatch の逆依存 4 crate は A と同じ。
- 判断: S1 の変更波及は A より減っていない（file と単位は +1、import の単位は検査器の差の範囲）。S2 は変わらない。

## 3. health（A §5.5、const の probe、`repo_health.py --base <after の rev> --head worktree`）

| | run 1 | run 2 | run 3 | 中央値 | user（中央値） | 開始時の 1 分 load | verdict |
|---|---|---|---|---|---|---|---|
| S1 A | 87.82 s | 86.56 s | 81.48 s | 86.56 s | 69.45 s | 0.3〜1.3（A §1） | fail（exit 1）: new 3・worsened 1 |
| S1 after | 97.06 s | 92.78 s | 93.58 s | **93.58 s（+8.1 %）** | 71.19 s（+2.5 %） | 3.91 / 3.50 / 2.56 | fail（exit 1）: new 4・worsened 1 |
| S2 A | 81.76 s | 81.92 s | 85.96 s | 81.92 s | 69.46 s | 0.3〜1.3（A §1） | fail（exit 1）: new 5・worsened 2 |
| S2 after | 98.73 s | 96.09 s | 90.65 s | **96.09 s（+17.3 %）** | 71.86 s（+3.5 %） | 2.02 / 3.55 / 3.93 | fail（exit 1）: new 5・worsened 2 |

- 6 回とも valid（開始時の 1 分 load 4 以下、待った秒 20・0・0・0・0・30）。測った時刻は 2026-11-19T05:20Z〜05:30Z。
  5 分の load は 8.48 → 4.42（並行の WorkUnit と release の build の余波）で、A の測定時より高かった。max RSS は 644〜646 MB（A 611 MB）。
- findings（S1）: `link.unlinked` の new 4（probe の `SCENARIO_PROBE_1〜4`、file ごとに 1）、`size.responsibility-growth` の worsened 1
  （task-core/src/store の module の本体の行の合計 16930、閾値 8000）。fail の rule は `size.responsibility-growth` だけ（A と同じ種類。new が 4 なのは file が 4 個だから）。
- findings（S2）: `link.unlinked` の new 4、`size.responsibility-growth` の worsened 2（`dispatcher/work_units.rs` の本体の行 2055 が閾値 2000 超、
  dispatcher の module の合計 32328 が閾値 8000 超）、`source-size` の new 1（work_units.rs）。fail の rule は `size.responsibility-growth` と `source-size`（A と同じ）。
- 観察（未確認の仮説）: user の CPU 時間の増加（+2.5〜+3.5 %）より wall の増加（+8.1〜+17.3 %）が大きい。wall − user − sys は A の S1 で約 4〜10 s、after は S1・S2 で約 11〜18 s。
  検査器の rule の増加（B・C）でなく host の状態（I/O・並行の build）による部分がある可能性がある。cause の leaf が before を同じ host で測り直して分ける。

## 4. review の時間と費用（A §7 C11、本番 DB を `mode=ro` で読むだけ）

query は A §7 C11 と同じ（agent-platform 案件の reviewer run、`finished_at` が null でなく `started_at` が窓の中）。

| 窓 | run | wall の中央値（p90） | cost_usd の中央値（p90） | cost の合計（cost のある run） |
|---|---|---|---|---|
| A の窓の再現 [2026-11-10, 2026-11-17T15:00) | 165 | 1,137.5 s（1,887.0 s） | $2.047（$4.247） | $280.17（127 run） |
| A の記録（baseline.md §5.4） | 165 | 1,137.5 s（1,928.6 s） | $2.047（$4.255） | $280.17（127 run） |
| after の部分窓 [2026-11-17T15:00, 2026-11-19T05:00) | 28 | 669.1 s（1,425.0 s） | $2.267（$4.128） | $50.23（25 run） |
| 参考: A の窓の末尾の同じ長さ [2026-11-15T13:00, 2026-11-17T03:00) | 21 | 676.2 s（1,335.5 s） | $2.133（$4.360） | $37.94（16 run） |

- A の窓の再現は run の数・中央値・合計が A と一致した。p90 だけ違うのは分位の計算方法の差（ここでは線形補間。A の方法は記録に無く未確認）。
- after の窓は 38 時間（A の窓は 7 日 15 時間 = 183 時間）で、run が 28 件しかない。**比べられない（未達/要判断）**。
  同じ長さの窓 [2026-11-17T15:00, 2026-11-25T06:00) が揃うのは **2026-11-25T06:00Z** の後（その窓に始まった run が終わった後）。
- 部分窓の reviewer run は、アーキ v4 自身の子 task の review を多く含む。シナリオごとの review の値は A と同じ理由（worker は LLM の run を起こせない）で測れない。
- 窓の中で wall_ms の無い run がある（A の窓で 165 run 中 130 run だけ wall_ms がある。after は 28 中 25）。中央値は値のある run で取った。

## 5. 再現コマンド（repo の root で。`ART` はこの run の成果物 dir）

```sh
. agent-docs/progress/2026-11-19-arch-v4-g/scenarios.env
H=$(git rev-parse HEAD); B0=bc665168c9caf1898adc4c611404bbee056daf9a; CK=scripts/dev/repo-health
# C2（SCC。A §7 C2 の snippet。D の raw/c2.py の写し）
python3 $ART/raw/c2.py $CK $H $ART/raw/cycle-edges-after.json
# C7b（A §7 C7b の snippet、第 1 引数を検査器の dir にした D の raw/c7b.py の写し）
python3 $ART/raw/c7b.py $CK $H S1after:crates/task-api/src/org_write,crates/task-api/src/handlers,crates/task-api/src/cos,crates/task-core/src/store \
  S1:crates/task-api/src/handlers,crates/task-api/src/cos,crates/task-core/src/store S2:crates/task-dispatch/src/dispatcher,crates/task-dispatch/src/run_settlement
python3 $ART/raw/c7b.py $CK $B0 S1:… S2:…            # before'（after の検査器 × bc665168）
# C7（cargo metadata の通常依存の逆依存）と --list（comment の probe を入れた状態、終わったら戻す）
cargo metadata --format-version 1 --offline > $TMPDIR/meta.json
$ART/raw/probe-comment.sh $S1_FILES; bash scripts/dev/test-affected.sh --list; git checkout -- $S1_FILES
# health 3 回ずつ（const の probe・waitload・TSV。rules.md の規則）
TSV=$ART/raw/scope.tsv OUT=$ART/raw DEADLINE=<epoch 秒> AFTER_REV=$H S1_FILES="$S1_FILES" S2_FILES="$S2_FILES" bash $ART/raw/c1-health-tsv.sh
# C11（本番 DB を mode=ro で読む。A の query と同じ。窓を [2026-11-10, 2026-11-17T15:00) と [2026-11-17T15:00, 2026-11-19T05:00) に当てる）
#   sqlite3.connect('file:/local/celeris/data/db/celeris.sqlite3?mode=ro', uri=True)
```

生の記録（この run の成果物 dir の `raw/`）: `c2-after.txt`・`cycle-edges-after.json`・`c7-after.txt`・`c7b-after.txt`・`c7b-b0-headchecker.txt`・`ta-list.txt`・
`health-S{1,2}-{1,2,3}.{json,out,time}`・`timing-raw.log`・`scope.tsv`・`review-cost.txt`。測り終えた後に `git status --porcelain -- crates` は空だった。

## 6. 未達・要判断

| 項目 | 状態 |
|---|---|
| review の時間と費用の before/after | 未達/要判断（after の窓が 38 時間。2026-11-25T06:00Z に同じ長さの窓が揃う） |
| health の S2 の wall +17.3 %、S1 +8.1 % | 原因は未確認（compare・control-build・cause の leaf が A と同じ host・同じ時刻帯で before を測り直して分ける） |
| S1 の変更波及 | 減っていない（file・単位 3 → 4。D の集約で `org_write.rs` が増えた分） |
| Cargo の dev 依存 14 → 12 | 数え方の違いか依存の変化かは未確認（通常依存 35 と逆依存の範囲は同じ） |
