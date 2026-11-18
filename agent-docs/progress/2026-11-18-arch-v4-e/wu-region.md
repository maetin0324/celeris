---
title: アーキ v4 E — WU 領域（write_set_gate・write_set_record を store・設定に触らない関数にする）
tasks: [01M7Q5ZABN9NFQCH6QBTTP3EZ8]
status: running
updated: 2026-11-18
---
# アーキ v4 E — WU 領域: write_set_gate・write_set_record

設計は [ADR 2026-11-18 dispatcher-store-access-pilot](../../adr/2026-11-18-dispatcher-store-access-pilot.md) の「第 1 領域: WU」の表の
`write_set_gate.rs`・`write_set_record.rs` の行。計測コマンドは [measure-commands.md](measure-commands.md) の C3・E3。
基準は `git merge-base HEAD main`（WorkUnit は `$CELERIS_WU_BASE`）。値は C3 の 1 行の数と E3 の token の数。

## 結論

- 2 file とも `impl Dispatcher` と `use super::*;` を消し、E3 の 7 token は comment を含めて 0 になった。
- dispatcher/ の self.store を書く file は 44 → 42（−2）、crate 全体も 53 → 51（−2）。self.config・SqliteStore・非試験 file 数 69・`#[path]` は変わらない。
- **ADR の表との違いが 1 つある**: `record_run_write_sets` の入口を `work_units.rs` ではなく `dispatch_run.rs` に置いた。
  理由は下の「ADR の表との違い」。人の決定 `record-run-write-sets-home`（2026-11-18）は「`dispatch_run.rs` に置いたまま進める。
  ADR の表は close の段で付記して直す」。この WorkUnit の受け入れ条件 1 の「record_run_write_sets は work_units.rs」はこの決定で置き換わる。

## 2 file の before/after

| file | 行数 | C3 self.store（file 内の出現） | C3 self.config | E3（self.store self.config SqliteStore Dispatcher CosChatLaunch DispatchConfig `use super::*`） |
|---|---|---|---|---|
| `dispatcher/write_set_gate.rs` | 122 → 89 | 2 → 0 | 0 → 0 | `2 0 0 1 0 0 1`（計 4）→ `0 0 0 0 0 0 0`（計 0） |
| `dispatcher/write_set_record.rs` | 197 → 188 | 2 → 0 | 0 → 0 | `2 0 0 2 0 0 1`（計 5）→ `0 0 0 0 0 0 0`（計 0） |
| `dispatcher/dispatch_run.rs`（入口） | 1683 → 1749 | 30 → 34 | 25 → 25 | — |
| `dispatcher/work_units.rs` | 2060 → 2060（変えていない） | 57 → 57 | 11 → 11 | — |

E3 は comment の行も数える。2 file の doc comment からも `Dispatcher` の語を消した（旧い doc の
[`Dispatcher::capture_run_write_bases`] は [`run_write_bases`] に直した）。

## 移した操作（旧い場所 → 新しい場所）

| 操作 | 旧い場所 | 新しい場所 |
|---|---|---|
| `Dispatcher::admit_write_set`（名・引数は同じ。呼び出し元 `dispatch_one` は変えない） | `write_set_gate.rs` の `impl Dispatcher` | `dispatch_run.rs` の `impl Dispatcher` |
| hint の読み出し `self.store.effective_task_write_paths`・`self.store.work_unit_expected_write_paths`（会話の task と planner run は読まない） | `write_set_gate.rs` の `Dispatcher::write_reservation_for` | `dispatch_run.rs` の `admit_write_set` |
| 予約の組み立て（inherited の判定・`normalize_write_paths`・repo 鍵） | `Dispatcher::write_reservation_for`（消した） | `write_set_gate.rs` の純粋関数 `reservation(task, task_hint, wu_hint)`・`write_set_repo_keys` |
| 判定と待ちの記録（`ExecutionCoordination::admit_write_set`・`Hold` なら `note_dispatch_wait`） | `write_set_gate.rs` の `admit_write_set` | `dispatch_run.rs` の `admit_write_set`（同じ順序） |
| 開始 HEAD の固定 `Dispatcher::capture_run_write_bases`（消した） | `write_set_record.rs` | `dispatch_one` の中で `self.coord.put_run_write_bases(&run_id, write_set_record::run_write_bases(&task, ws))`（planner run と作業場所なしは空で、`put_run_write_bases` は空を置かない） |
| `Dispatcher::record_run_write_sets`（名・引数は同じ。`worker_finish.rs:831` は変えない） | `write_set_record.rs` の `impl Dispatcher` | `dispatch_run.rs` の `impl Dispatcher`（ADR の表では `work_units.rs`。下の「違い」） |
| run の記録の組み立て | `record_run_write_sets` の本体 | `write_set_record.rs` の `run_records(task, run_id, work_unit_id, bases, has_local_site)` |
| `self.store.record_run_write_set`（失敗は warn だけ） | `write_set_record.rs` | `dispatch_run.rs` の `record_run_write_sets`（同じ回数・順序） |
| `Dispatcher::record_work_unit_write_sets`（消した） | `write_set_record.rs` | 組み立ては `work_unit_records(task, wu, ws)`、`self.store.record_work_unit_write_set`（失敗は warn だけ）は `dispatch_run.rs` の `record_run_write_sets` |
| `git_sites`・`write_set_record` | `write_set_record.rs`（private） | `write_set_record.rs` に残す（`git_sites` は入口が「手元に Git worktree があるか」を見るので `pub(super)`） |

振る舞いを変えていない点:

- store への書き込みは run の記録 → WU の snapshot の順で、回数も同じ。失敗はどれも `tracing::warn!` だけで、run の遷移を変えない。
- `task_workspaces_for`（repo を store から読む）は今と同じく、開始 HEAD が無いときと、done の v2 WU のときだけ呼ぶ。
- WU の snapshot の git の読み取りは、全 site を採ってから書く順にした（旧くは site ごとに採って書いた）。書き込みの回数・順序・失敗時の扱いは同じ。
- 旧い `record.owner_id = wu.id.clone();` は、`write_set_record(task, &wu.id, …)` が既に `owner_id` に `wu.id` を入れているので何もしない代入だった。消した。

## ADR の表との違い（record_run_write_sets の置き場所）

ADR は `record_run_write_sets` の入口を `work_units.rs` に置くと決めた。置くと `bash scripts/dev/test-affected.sh` の先頭の
`check-health.sh` が exit 1 になった（提出前の HEAD で実行。`work_units.rs` に 37 行を足した状態）:

- `size.responsibility-growth` FAIL: `work_units.rs` の本体の行 2096 が閾値 2000 を超えた（base で既に 2060。増やすと worsened）。
- `source-size` FAIL: `work_units.rs production_size : value=2097 threshold=2000 baseline=2182 comparison=2060 allowed=2060`。

`work_units.rs` は閾値 2000 を超えた file で、1 行でも増やすと必須 rule が落ちる。ADR の「work_units.rs は入口として残る（減らない）」は
この ratchet を見ていなかった。例外を足すのは policy の変更（`scripts/dev/source-size-report.toml`）で、この WorkUnit の範囲の外。

そこで入口を `dispatch_run.rs`（移す前から self.store 30・self.config 25 の入口。開始 HEAD を置く `put_run_write_bases` も同じ file）に置いた。
開始 HEAD の固定（dispatch）と取り出し・記録（run の終わり）が同じ file に並ぶ。file の数・分母・SqliteStore の数は ADR の見込みと同じ。

同じ ratchet は並行の WorkUnit `target-leaf` の `auto_leaf_decision`（ADR では `work_units.rs` に置く）にも当たる見込み（未確認）。

人の決定は「`dispatch_run.rs` が size の閾値に近づく場合と、self.store・self.config に触る file の数が増える場合は進捗に書く」とした。
提出時の HEAD で次のとおりで、どちらにも当たらない:

- `dispatch_run.rs` の production の行（`scripts/dev/source-size-report.py --exclusive-classes --files --format json` の
  `classes.production`）は base 1673 → HEAD 1739（+66）。閾値 `production_lines = 2000` まで 261 行ある。
- self.store・self.config を書く file の数は増えていない（下の「証拠」の C3: store 44 → 42、config 35 → 35）。

ほかに 2 つの必須 rule に当たり、同じ変更の中で直した:

- `size.responsibility-growth`（`Dispatcher::dispatch_one` の関数の行 1011 → 1012）: 開始 HEAD の固定を 3 行のまま書いた（`let ws = …; let bases = …; self.coord.put_run_write_bases(…);`）。
- `size.responsibility-growth`（module `crates/task-dispatch/src/dispatcher` の本体の行の合計 32338 → 32341）: 上の何もしない代入を消し、WU の snapshot の 2 つの `let … else { return; }` を let-chain にした。
  repo-health の `_logic_lines` で数えて、4 file の合計は 3486 → 3484（−2。`write_set_gate.rs` 82 → 47、`write_set_record.rs` 172 → 149、`dispatch_run.rs` 1453 → 1509、`work_units.rs` 1779 → 1779）。

LoC（全行、comment を含む）: `git diff --numstat` で dispatch_run.rs +69/−3、write_set_gate.rs +34/−67、write_set_record.rs +102/−111（計 +24）。ADR の目安 +80 の中。

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）

- 人の決定の後に、同じ HEAD（コードは前回の commit から変えていない）で全部をやり直した。
- Task の受け入れ条件 0 の snippet（作業ツリーで数える）→
  `files 69->69 store 44->42 occ 633->633 config 35->35 occ 218->218 crate-store 53->51 crate-config 40->40 SqliteStore-files 50->50`。
  WorkUnit の条件（store −2 以上・crate も同数以上・SqliteStore 不増・分母不減）は exit 0。self.config −1 は並行の `target-leaf` の分。
- `git grep -n '#\[path' -- crates/task-dispatch/src | wc -l` → 94（base も 94）。
- E3（上の表）→ 2 file とも 0。
- `git diff $B -- crates/task-dispatch/src/dispatcher/worker_finish.rs crates/task-dispatch/src/health_review crates/task-dispatch/src/execution_coordination.rs crates/task-dispatch/src/dispatcher/tests` → 差分なし。
- 受け入れ条件 4 の grep（新しい trait・Manager・Facade・Port・Service）→ exit 0（該当なし）。
- `bash scripts/dev/check-health.sh --out "$TMPDIR/health.json"` → exit 0（`verdict=pass … new=0 worsened=0`、必須 rule 21 件がすべて pass。
  module `crates/task-dispatch/src/dispatcher` の本体の行の合計は 32338 → 32336）。
- `bash scripts/dev/test-affected.sh` → exit 0（対象 crate: celeris celeris-campaign celerisctl llm-proxy task-dispatch。`3933 tests run: 3933 passed (10 slow), 11 skipped`、doc test は ok、script bundle SKIP 0 件）。
- `cargo nextest run --workspace -E 'package(=task-dispatch) & (test(write_set_gate_) | test(write_set_record_) | test(cex_) | test(structure_guard))'` → `62 tests run: 62 passed`。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。

## 未解決

- `record_run_write_sets` の置き場所（`dispatch_run.rs`）は ADR の表と違う。人の決定 `record-run-write-sets-home` で
  `dispatch_run.rs` のままと決まった。ADR の表の付記は close の段が書く（この WorkUnit では ADR を変えない）。
- `store_access_guard_*` の試験は後の WorkUnit `guard` が足す（この WorkUnit の範囲の外）。

## 提案

- ADR の「第 1 領域: WU」の表の `work_units.rs` の行（入口として受ける）を、「閾値 2000 を超えていて行を増やせない。入口は `dispatch_run.rs` か、
  行を増やしてよい入口 file に置く」と直す（close の付記）。
