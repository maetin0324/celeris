---
title: Dispatcher の continuation・WU失敗伝播・review結果の等価性調査
tasks: [01M74VAMWFCC4JGZY4ENA92HJJ]
status: completed
updated: 2026-11-11
---
# Dispatcher の3経路の等価性調査

完了日: 2026-11-11。基準は `$CELERIS_WU_BASE`。ソースの行番号は提出時の HEAD のもの（コード差分なし）。

3件とも経路全体は `different`。一部に共有済みの実装と、共通化を検討できる判定の繰り返しがある。関数名が似ていることだけで等価とは判定していない。各JSONの `shared_contract` に入力・前提・状態/event・失敗時・共有可能な範囲を記録した。

## 調査範囲とbrief

- 根本原因: 類似した処理が同じ不変条件を重複実装しているか、異なる契約なのかを区別するための調査。障害の修正は行わない。
- 責務owner: continuation_session、worker_finish、cos_chat、work_units、tree_units、child_tasks、phase_integration、review_verdict。owner索引は `docs/architecture-map.md` のWorkUnit、委譲/承認の子task、planner/reviewerの行。
- 再利用先: session解決、既存のfailure_signature/stalled_replans、spawn_review/on_review_finished、WU checkからworker結果処理への呼び戻し。
- 選択肢: 全経路を同じ処理と数える案は採らず、共通部分と異なる副作用を分けて判定した。
- 新しい状態・分岐・fallback・port: 無し。
- 撤去対象: 無し。設計の変更・実装・試験追加は行わない。
- 不変条件: コード不変、SQLiteを正本とすること、自己申告だけで完了させないこと、event追記のみ。JSONと参照の検査で測定成果の形式・位置を確認する。

## continuation-resume

[JSON](equivalence/continuation-resume.json) — `different`。

| 比較軸 | task / WorkUnit | CoS chat / eval trial |
| --- | --- | --- |
| 入力・前提 | task、任意WU、直近worker終端、checkpoint、roleと実行面。`continuation_session.rs:31` は両方を扱う共通実装 | CoSはthreadのkey、session、拒否/context枯渇、配送cursor（`cos_chat/rollover.rs:123`）。evalは常にfresh（`eval_trials.rs:885`） |
| 状態・event | sessionのretire/create、RunExtras、WorkerProgress、CheckpointSavedとWU/taskの遷移 | CoSはchat session/runの記録とsummary・配送水位。evalはDoneをscoringへ、それ以外をerrorへ |
| 失敗時 | sinkの拒否印とretireを受け次dispatchでfresh＋checkpoint。継続上限は質問へ | CoSはRunningを確認し同じrun・同じ入力でfreshを1回試行、再失敗はFailed。evalはcontinuationしない |

WUの継続は `worker_finish.rs:751` からschedulerへ渡す一方、atomic taskの進捗なし・上限判定は同fileの1129行以降にある。session解決そのものは共有済みで、継続の判断全体が重複しているわけではない。共有候補は互換性・拒否・context上限の事実からResume/Freshを返す純粋な判定、及び進捗なし・上限の判定まで。auto_leaf、子待ち、chat入力の再配送防止は固有の契約として残る。

## wu-failure

[JSON](equivalence/wu-failure.json) — `different`。

| 比較軸 | worker完了・遅延WU・統合 | 木の子task・委譲した子task |
| --- | --- | --- |
| 入力・前提 | WuDecision/check失敗、保存WU/直近event、統合WU/理由。各々に同一失敗の停滞とreplan上限の判定がある | 木はschema v3のRunningなkind Task WUと子の状態。委譲はDelegatedとglobal event idで未処理の失敗を抽出 |
| 状態・event | AutoReplanRequested又は質問。統合は先にWUのleaseを消しFailed/Blockedを記録してからtaskを遷移 | 木はWorkUnitTransitionedで子を写す。infraはChildTaskCreatedで作り直すかBlocked(Infra)。委譲は親にChildFailed |
| 失敗時 | healthの停止理由は統合のreplanを禁止。WU上限はtree有効な節点なら決定要求、それ以外は質問。merge repair失敗はCancelled、統合をPendingへ戻す | 木のinfra再作成上限では通知して段を停止、兄弟は続行。委譲は親attempts/max_retriesで再実行か質問。Ignoreと会話taskは除外 |

`worker_finish.rs:907`、`work_units.rs:1283`、`phase_integration.rs:1327` は共通化候補のある箇所。ただし署名の材料、health停止、上限時の決定要求が違う。failure_signatureとstalled_replansは再利用済み。事実からReplan/Askと理由を返す判定までを候補とし、WU更新・子の作り直し・親attempts・承認/通知は分ける。worker完了のWU自身/依存WUの遷移書き込み失敗はwarnして継続する（`worker_finish.rs:1316`、1336）。他の照合・判定のstore errorは原則伝播するが、統合のInvalidTransitionは無視、委譲経路はfalseを返す。経路全体を同一関数へ集める根拠はない。

## review-result

[JSON](equivalence/review-result.json) — `different`。

| 比較軸 | task最終review | WU check・統合 |
| --- | --- | --- |
| 入力・前提 | ReviewOutcomeとReviewing task、target snapshotの鮮度 | WUはRunOutcome/WorkUnitCheckRunとlease。統合はIntegrationRunとRunning task/統合WU |
| 状態・event | reviewer run/quotaを閉じReviewVerdict/ReviewCommandChecked。子待ち・集約・repairを経てReviewPass/ReviewFail等 | WU不合格はretryable Errorへ変換してfinish_worker_result_withへ。統合成功はWU Done/PhaseIntegratedから次段・PhaseGate・最終reviewへ |
| 失敗時 | staleは延期、供給/infraは上限内又はAuthFailedならattempts不変で延期。不合格はrepair/blocked/fail | WUのstale leaseと統合の非Running結果は破棄。WU check失敗はworker retry、統合失敗はmerge/check repair又は質問 |

`work_units.rs:360` はreviewerの結果適用ではなくcheck完了の入口。`review_spawn.rs:196` の最終reviewと `phase_integration.rs:1543` の統合後の最終reviewはともにspawn_reviewへ接続し、結果はon_review_finishedで処理する。ここを重複実装として数えない。途中のPhaseGateは最終reviewと別の人の確認である。

共有候補は決定的check不合格を成功にしない判定、失敗一覧の構築まで。check実行は既にreview側を再利用する。leaseとsnapshotの鮮度条件、event型、retry/repairと完了対象は異なる。reviewer合格でも決定的checkの不合格を優先する契約を維持する。

## 証拠

提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`。

- 条件0（JSON3件、sites/evidence各2件以上）: run成果物の `validate.py` を `python3` で実行 → exit 0。continuation-resumeは7/15件、wu-failureは7/18件、review-resultは6/18件（sites/evidence）。全siteの関数名・参照行の実在とHEADソースとの一致を確認。
- 条件1（比較根拠）: 同validatorで各shared_contractの入力・前提・状態・失敗の記述を検査 → exit 0。本文の3表とソースを照合し、共有範囲と異なる副作用を記録。
- 範囲: 同validatorと `sh "$CELERIS_WU_SCOPE_PATHS"` → exit 0、指定4ファイルのみ。`git diff "$CELERIS_WU_BASE" -- crates/ scripts/` は差分ゼロ。
- `bash scripts/dev/test-affected.sh` → exit 0。doc-onlyで試験対象crateなし。repo-healthはpass、新規/悪化findingは0（既存141）。
- `cargo clippy --workspace -- -D warnings` → exit 0。依存deno_coreのdeprecated警告1件は出たが検査は成功。
- `cargo fmt --all -- --check` → exit 0。
- `sh scripts/dev/progress-index.sh --check` → exit 0、ok。
- `git diff --cached --check` → exit 0。

## commit

- `phase 1: record dispatcher flow equivalence measurements`

## 未解決事項

コードの呼び出しと副作用を静的に比較した結果であり、実行時の等価性を証明する測定ではない。dispatcher外の判定関数は呼び出しの契約までを扱い、その内部実装の監査は範囲外。

## 提案

後続の設計では3JSONの共有候補を検討する。経路全体の統合は前提としない。本番への移行は不要。
