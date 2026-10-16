---
task: k-cases (監査修正5)
status: 一部実施（実 build・cluster 実行は未実施／承認待ち）
completed: 2026-10-16
---
# K1〜K4 の grade-case/1 と固定（k-cases）

## 実装済み（crates/campaign）
- `fixtures/grade-cases/<split>/<id>/<id>.json`（`celeris.grade-case/1`。`campaign-case-private/1` とは別物）を K1〜K4 の 4 件に用意。
  - K1・K2: `grade_case_gen::generate` の出力（`GenSource.wrapper` を追加）。実装検査 + 実験条件（nodes=3）。隠しテストは
    `hidden/.grader/` の grader 側 file。BenchFS の試験は実装内の `#[cfg(test)] mod` なので file 単位で重ねられず、
    `sh .grader/run.sh cargo test ...` が末尾へ `#[path]` の子 module を足してから評価 command を走らせる（成果物が何を書き換えても足せる。
    成果物側に隠しテストが無いことは削除判定にしない）。隠しテストの本体は既存資産（K1=086c4618 の 3 試験、K2=p0-final.patch の 1 試験）の写しで、新規の模範解答は作っていない。
  - K3・K4: `fixtures/grade-cases/tools/gen_recompute.py` が原票 tgz から作る recompute（固定入力 sha256 + 指標）。K3=条件×size×op の平均帯域 30 指標、K4=cell.json の件数 5 指標。
- `case.json` に `grade_case_ref`（path+sha256、real case は必須）を追加。欠ける・path が違うと `CaseSet::load` が拒否する。
- submodule を `<path>@<40 桁 commit>` に固定（K1・K2 とも `lib/pluvio@9ecd51ee9b5f…`・`lib/locusta@68f8b2104d33…`・`ior_integration/ior@7054224dd8ab…`）。短縮 hash は loader が拒否。
  gitlink が初期 commit の tree と一致することを bundle で確認済み（下の試験）。
- 資料・初期 KB は既存の `snapshot.materials_sha256`・`snapshot.kb_snapshot`（case ごとの hash_tree、loader が検査）と K1/K2 の初期 commit/tree/archive sha256 で固定済み。archive sha256 は bundle から再計算して一致を確認。

## 検証（モデル・ネットワーク無し）
`cargo test -p celeris-campaign --test k_cases_grade`（7 passed, 1 ignored=作り直し用）。
- 全 real case に pin 済みで読み込める grade-case が在る／欠落・短縮 submodule で loader が落ちる／固定 JSON が現在の case から再生成した物と一致。
- K1・K2 の既知の誤実装=不合格・修正版=合格（隠しテスト挿入つき、実装検査のみ）。**cargo は textual stub**（挿入した module の宣言と、修正版だけにある記述を見る）。実コンパイルではない。
  同じ確認を、bundle から取り出した実 BenchFS の木で実施（K1: 4115dbd8=誤、086c4618=修正／K2: 086c4618=誤、+p0-final.patch=修正）。bundle・patch が無い環境では SKIPPED と出て通る（実行不能であり合格ではない）。
- K3・K4: 実原票から素朴に計算した報告=recompute 合格、先頭指標を 6% ずらした報告=不合格（tgz の sha256 も固定値と一致を確認）。

## 未実施・承認待ち（実行できなかった項目）
- **BenchFS の実 build と `cargo test`（K1・K2 の隠しテストの実走）**: submodule の実体（pluvio・locusta・ior）が bundle に無く（asset store への配置は承認待ち／未実施）、UCX も要るため本 repo では build できない。stub は実走の代替ではない。
- K1・K2 の実験（Sirius の node-h 実 job）: 承認待ち／未実施。grade-case は `evidence/jobs.json` の条件（nodes=3、min_jobs K1=4・K2=1）だけを見る。
- K2 の隠しテストは補助関数 `lookup_metadata`（修正が導入した名前）を直接呼ぶ。同じ効果を別名で実装した正しい解は**コンパイルできず不合格**になる（private の `portability_unverified` と同種）。公開 API 越しの試験への書き換えは人の判断（提案）。
- K3・K4: recompute は原票の件数・平均帯域だけ。K3 の「反復ごとの比の平均」（暫定）と K4 の有効性判定（fsync 実効値・qstat・分母）の独立再計算、`review` 工程の grade-case は未作成。
- 報告の `metric <name> = <value>` 行の規約が K3・K4 の task.md に書かれていない（grader.md の想定は問題文に書く）。task.md を変えると final.json の `materials_sha256` が動くため、final-fix 側で決める必要がある。
- K1/K2 の公開テスト（実装内の既存試験）は成果物 repo のソースの一部で改変検知の file 固定ができない（`tests` は空）。
- final.json / final-input-manifest.json との結合（問題 → grade-case の対応・定義 hash）は final-fix・preflight の担当。K2 case の `walltime_secs`/`max_wall_secs` 7200 と final.json の 1800 の不整合も未解決（人の回答に従う別 WU）。

## 提案
- 結合キーには grade-case の sha256（`case.json` の `grade_case_ref.sha256`）と hidden の各 sha256 を含める。
- 再作り直し: `CAMPAIGN_UPDATE_CASES=1 cargo test -p celeris-campaign --test k_cases_grade -- --ignored`（K3/K4 は先に `gen_recompute.py`）。
