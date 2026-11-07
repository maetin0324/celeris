---
tasks: [01M6TYVJSPEYA2P8B7G56HSQ73]
---
# ADR 2026-11-07: 健全性 v2 の基準と G1〜V0 の slice 規則（単一 owner・停止条件・行数の見方）

- 状態: 採用（設計）。この ADR ではコードを変えない。実装は親 task 01M6TYV9DTZESAJJMXRQVGZ56V の G1〜G4・R1〜R6・V0。
- 基準: B0 着手時の `git merge-base HEAD main`（件名「repair: main 取り込みの衝突解消の import を整形する」）。SHA は計測 JSON の `base_sha` 欄にある。
- 根拠の記録（raw は repo、判断材料は B0 の成果物 dir `/local/celeris/data/workspaces/01M6TYVJSPEYA2P8B7G56HSQ73/artifacts/b0/`）:
  - slice の機械向け正本 [slices.json](../progress/2026-11-07-health-v2-b0/slices.json) と検査 `slices-check.py`（同じ dir）
  - 行数 [loc-classes.md](../progress/2026-11-07-health-v2-b0/loc-classes.md)（数値は `loc-baseline.json`）
  - test target [test-targets.md](../progress/2026-11-07-health-v2-b0/test-targets.md)（数値は `test-targets.json`）
  - 既存 task と中止理由 [task-history.md](../progress/2026-11-07-health-v2-b0/task-history.md)
  - capability と owner [capability-map.md](../progress/2026-11-07-health-v2-b0/capability-map.md)、重複候補 [dup-candidates.md](../progress/2026-11-07-health-v2-b0/dup-candidates.md)
  - 計画の本文 `plan.md`（上の成果物 dir）
- 関係: [ADR-0009 P-9](0009-proposal-adoption-and-phase-closure.md)、[ADR-0079](0079-recursive-task-decomposition.md)、
  [2026-10-11 破壊的変更を既定にする](2026-10-11-breaking-changes-by-default.md)、
  [2026-10-20 全体試験は release だけ](2026-10-20-full-tests-at-release-only.md)。

## 結論
- 最終アーキテクチャは「capability ごとに書き手（owner）が 1 つ、正本が 1 つ」。41 capability の正本と担当工程を slices.json の `capability_owners` に固定した。
- 後続 11 工程を、scope・write_set（path の正規表現）・決定的な check・停止条件・依存を持つ縦の slice にする。停止条件に当たったら範囲を広げず止め、repair か replan にする。
- R3・R4 は cancelled の旧健全性 3・4 の objective を残課題として引き継ぐ。旧 2 件の cancel は依存先の一時的な review_fail による自動 cancel で、中身が原因ではない。
- 行数は 4 つの見方の基準値であり、どの工程の受け入れ条件にもしない。最初に行数を動かす工程（R1）の実績で見込みを出し直す。
- 比較指標は review 費用・edit→verify 時間・test target 数の 3 つ。基準値は slices.json の `metrics` に固定した。

## 決定

### D1. 最終アーキテクチャ: capability ごとの単一 owner と正本
- capability-map の 41 capability それぞれに正本（`canonical_owner`）を 1 つ置く。正本は capability-map の owners の先頭の module にした。
- 担当工程の内訳: R2 8・R3 5・R4 4・R6 11・G3 4・G2 2・R1 1・V0 1。どの工程も触らない 5 個（celeris-mcp・web-gateway・web-spa・script-runtime・cron-jobs）は `none`。
- 同じ表を複数の crate が書く状態（capability-map の multi_writer_state 52 件）は、表ごとの書き手を正本の module に寄せて減らす（主に R3）。
- dup-candidates の delete 35・merge 5 はすべて 1 工程に割り当てた（R1 1・R2 4・R3 9・R6 26）。keep 15 は割り当てない。

### D2. slice の切り方
- 1 工程 = 1 slice。各 slice は次を持つ（slices.json の `slices[]`）。
  - `scope`: 正本へ寄せるもの（`consolidate_into_owner`）、消すもの（`remove`）、担当 capability、担当の dup 候補。
  - `write_set`: 変えてよい path の正規表現。各 dup 候補の file が担当工程の write_set に入ることを `slices-check.py` が確かめる。
  - `checks`: 全工程共通で `bash scripts/dev/test-affected.sh`、`cargo clippy --workspace -- -D warnings`、`cargo fmt --all -- --check`、
    `bash scripts/dev/check-health.sh`、文書検査 3 本、write_set の検査
    （`git diff --name-only $(git merge-base HEAD main) | python3 agent-docs/progress/2026-11-07-health-v2-b0/slices-check.py --write-set <KEY> agent-docs/progress/2026-11-07-health-v2-b0/slices.json`）。
  - `baseline_metrics`: その工程で比べる指標の名前。
- 依存順: B0 → G1 → G2 → G3 → G4。R1・R3 は G3 の後、R2・R5 は R1 の後、R4 は R2・R3 の後、R6 は R2〜R5 の後、V0 は G4・R6 の後。
- 全体試験・CPU を焼く負荷は slice の check に入れない（全体試験は release gate だけ）。

### D3. 停止条件の規則
次のどれかに当たったら作業を止め、repair か replan にする。範囲を広げて続けない。
1. 不変条件の試験（capability-map の invariants の evidence_path、check-health・test-affected の試験）が落ちる。直せなければ delivery-repair、設計の誤りなら replan。
2. write_set の外の path を変える必要が出る。plan_issue で申告して replan。
3. 検査器・policy・baseline（check-source-size の baseline、architecture-map の検査、test-affected の対象規則、required checks）を弱めるか除外を足す必要が出る。止めて人の判断を求める。
4. 旧い仕組みを残す互換分岐・deprecated・新旧切替 flag が要る。止めて replan。
- 工程固有の停止条件（例: R3 の「consumer の振る舞いが変わる」、R4 の「時間依存の試験は決定的にする」）は各 slice の `stop_conditions` に足す。

### D4. R3・R4 と cancelled 2 件の照合
- 照合結果: 01M5KWPW976PBWXG4B37TQ55XC（旧健全性 3、TaskStore の consumer 別分割）と 01M5KWQ3WME71Q0ATMPTJA7XE7（旧健全性 4、Dispatcher の判断と副作用の分離）は、
  2026-10-23T10:01:45Z に ready → cancelled（dependency_failed）になった。同じ秒に依存先の健全性 2 が review_fail で failed になっている。人や CoS による cancel ではない。run は 0 本で、引き継ぐ成果物は無い。
- R3 の scope: 旧健全性 3 の objective を引き継ぐ。出発点は再計測値の TaskStore の fn 宣言 193・supertrait 16。task-api が直接書く 3 表（cos_inbox_items・chat_client_requests・browser_task_policies）を task-core の store に寄せ、未参照の store 関数 9 件を消す。
- R4 の scope: 旧健全性 4 の objective を引き継ぐ。出発点は Dispatcher の field 115（親 objective の 113 から増えた）。数える command は slices.json の `structural_metrics`。
- 理由: 中止の原因は中身ではなく P-9 の自動 cancel なので、旧 objective はそのまま有効。数は旧 task の作成時から変わっているので、出発点は B0 の再計測値にする。
- P-9 の今の動き: `cascade_after_transition_tx`（`crates/task-core/src/store/transition.rs`）は failed/cancelled への遷移で後続を cancel し、`from.is_terminal()` なら何もしない。依存先を reopen しても後続は戻らない。戻すのは `retry_task_tx` の付け替えだけ。v2 の R4 は R2・R3 に依存するので同じことが起こりうる。
- 採用（案 A、推奨）: R3・R4 は旧 objective を引き継ぎ、P-9 の対策（依存先の reopen で dependency_failed の後続を戻すか受信箱に知らせる）を G3 の scope に入れる。G3 に入らなければ R6 で扱う。対策が入るまでは、R2・R3 が failed になったら親が R4 の cancel を点検して後継を起票する。
- 代替案 B: R3・R4 は旧 objective を引き継ぐだけにし、P-9 の挙動は変えない。親の closeout で cancelled の子を点検する。変更は小さいが、R3 が一度 failed になると R4 が cancel され、人か CoS が retry するまで止まる。
- G3 と R6 のどちらに入れるかは人が決める（進捗の「提案」節）。

### D5. 行数は基準であって受け入れ条件ではない
基準 commit の 4 つの見方（分類規則 `source-size-exclusive/1`、出典 loc-baseline.json）と、人の見立て 20〜30 万行との差:

| 見方 | 行数 | うち Rust | 20 万行との差 | 30 万行との差 |
|---|---:|---:|---:|---:|
| 本体のみ | 366,177 | 320,064 | +166,177 | +66,177 |
| 自前総量（試験込み） | 967,314 | 828,913 | +767,314 | +667,314 |
| 全追跡行（全言語） | 1,512,205 | 897,685 | +1,312,205 | +1,212,205 |
| 外部分離候補（評価基盤＋vendor） | 256,886 | 189,185 | +56,886 | −43,114 |

- 外部分離候補は repo の外へ出せる候補の量で、他の 3 つと同じ意味の「差」ではない。
- 削減量はどの工程の受け入れ条件にもしない（slices.json の `reduction_target_is_acceptance=false`）。受け入れは D2 の check と停止条件で決める。
- 理由: 削れる量は工程ごとの実測が無いと見積もれない。行数を条件にすると、試験や検査を削って数を合わせる誘因になり、D3 の 3 に反する。

### D6. 目標の更新手順
1. 行数を動かす最初の工程（R1）が review_pass になったら、その commit で `python3 scripts/dev/source-size-report.py` を同じ分類規則で実行し、4 見方を再計測する。
2. 削れた行数・消した dup 候補の数・使った review 費用を並べ、残りの工程の見込みを比例で出し直す。
3. 見込みは親 task の進捗の「提案」節と plan.md の更新で示す。見立て（20〜30 万行）を改めるかは人が決める。
4. slices.json の基準値は書き換えない。再計測値は工程の進捗 file に別に記録する。

### D7. 比較指標と基準値
| 指標 | 基準値 | 出典 |
|---|---|---|
| review 費用 | Claude の review run 517 本（2026-10-24〜11-07）、1 本の USD 中央値 0.45・p90 0.92、tokens 中央値 494,692・p90 1,347,122。GPT 系は tokens 中央値 633,586、USD は記録が無く未計測 | task-history.json の reviewer_window |
| edit→verify 時間 | `src/lib.rs` に 1 行足してから test-affected の対象 crate 全部の nextest --no-run が終わるまで: task-core 142.3 秒・task-dispatch 79.8 秒・task-api 113.5 秒・task-worker 98.1 秒・celeris 85.1 秒 | test-targets.json の round3_after_settle |
| test target 数 | 352 target、予測 test binary 379。主要 5 crate の binary は task-core 3・task-dispatch 2・task-api 114・task-worker 25・celeris 57 | test-targets.json |

- 各工程は slice の `baseline_metrics` に挙げた指標を、同じ手順で着手時と完了時に測る。V0 が全指標を並べる。

## 検討して採らなかった案
- 行数の削減目標（例: 本体 30 万行以下）を V0 の受け入れ条件にする案: D5 の理由で採らない。
- R3・R4 を旧 task の reopen で再開する案: 旧 task は run 0 本で成果物が無く、出発点の数も変わっているため、v2 の子として起票し直した。
- 正本を capability ごとに人が選ぶ案: 41 個を B0 で決めると時間がかかる。owners の先頭を既定にし、担当工程の着手時に確かめる（未確認事項として進捗に残す）。
