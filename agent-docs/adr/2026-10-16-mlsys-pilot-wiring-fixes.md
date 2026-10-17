# ADR 2026-10-16: MLSys 少量 pilot で見つかった campaign 配線の欠けを直す

- 日付: 2026-10-16
- 状態: 採用・実装済み（crates/campaign、task-core `backup_database`）
- 関連: 2026-10-15-mlsys-min-campaign、2026-10-16-campaign-budget-cap、2026-10-16-mlsys-independent-grader、
  2026-10-16-mlsys-final-connect、2026-10-11-breaking-changes-by-default

## 背景
release 5effbca11c70 で運用セッションが少量 pilot（2 case × A/B/C、出力 `/local/celeris/data/scratch/mlsys-pilot/`）を回し、
9 つの欠けが出た（URL の二重 `/api/v1`、相対 `--out`、template の acceptance 形と捨てられた HTTP 本文、max_parallel 無効、
A の permission mode 無しと無変更 passed、materials 未配布、case 間依存、cache tokens 未記録、`db backup` が終わらない、
grade が評価 command を走らせない）。本測定はこれが入るまで行わない。

## 決定
- D1 `--api-url` は server の origin（例 `http://127.0.0.1:8080`）だけを受ける。path を持つ URL（`…/api/v1` を含む）は
  拒否する（正規化して黙って受けない。形は 1 つ）。`--out` は起動時に絶対 path にする（A の exit-code wrapper が
  試行の workspace を cwd にして動くため）。
- D2 `--task-template` の `acceptance` は task-api の形（`{"type": …, "text": …}`）だけを受け、旧い入れ子
  （`{"text", "check": {...}}`）は起動時に拒否する。executor の `Err`（HTTP の status と本文。token は含まない）は
  試行記録の `error` 欄に 2 KiB まで残す。executor が投入前に失敗して workspace が無いときは初期 snapshot から作り、
  hash 計算の失敗が本当の原因を隠さないようにする。
- D3 `budget.max_parallel` は非同期 executor の **in-flight（投入済みで未完了の台帳行）の上限**。上限に達したら新しい
  試行は投入せず `waiting`（`parallel_deferred` に数える）で返す。次の `run` が先の試行を回収して予算・使用量を
  見てから次を出す。runner は 1 回の走査で戻る（待ち続けない）点は変えない。
- D4 A（`claude -p`）は `--permission-mode bypassPermissions` で起動する（試行専用の複製 workspace の中だけで動く。
  対話の承認が無い headless で書き込み・Bash が拒否されるのは「制限」ではなく起動の誤り）。turn・許可リストの
  後付け制限を足さない方針は変えない（2026-10-17: ファイルシステムの隔離だけは ADR 2026-10-17-mlsys-pilot2-fixes D2 で足した）。A の試行が exit 0 でも workspace が初期 snapshot から変わっていなければ
  `outcome=failed`・`reason=no_artifact`（fail-closed）。worker からは起動せず、運用セッションが run を実行する前提は変えない。
- D5 `--materials-src` の資料は各試行の workspace の `materials/` に配る（repo + `materials/` を `snapshot/workspace-seed`
  に作り、これを試行の初期木にする。repo に `materials/` があれば衝突として拒否）。
- D6 case 間依存の拒否: problem の prompt が、**他の** problem の `command` が作る `out/…` path を読むなら
  `check_against_input`（validate・run）が拒否する（試行の workspace は分離されているので満たせない）。fixture の
  `review-numeric-consistency` は `materials/pluvio-report-draft.md` を読む自己完結の case に直した。
- D7 tokens: 試行記録・summary・export に `input / output / cache_read / cache_creation` を分けて残す
  （`token_usage`。cache が取れない harness は null＝欠測）。予算の判定にどれを使うかは manifest の
  `budget.token_basis`（必須。`input_output` / `input_output_cache_read` / `all`）で明示させ、暗黙の既定を置かない
  （どれにするかは人の決定。運用セッションが decision で答える）。`tokens` 欄は基準で数えた量で、基準に要る量が
  欠けた試行は欠測（per_case 上限で保守課金）。
- D8 `task_core::backup_database`（`celerisctl db backup`）は online backup API（書き込みのたびに最初からやり直す）をやめ、
  読み取り専用接続の `VACUUM INTO` で 1 つの読み取り snapshot を写す（db_maintenance と同じ方式）。`<dest>.partial-<pid>`
  に書いて rename するので、途中で止まっても `<dest>` に半端な file は残らない。
- D9 `grade` CLI は評価 command を実際に走らせる（`grade_runner::ProcessRunner`、grade の作業 dir の中だけ）。`NoRunner`
  は消す。pilot の 2 case の grade-case（`fixtures/grader-private/dev/pilot/`）を置き、fixture repo を実際に `cargo test`
  できる最小の package にした。`grade-out` で out の全試行を一括採点し `grades.jsonl` を書く。

## 直さないもの
- Jev smoke の所見（`real_api_verified=false`、Jev の費用・版が記録されない、route decision が shadow / route_defaults）は
  campaign の配線ではなく Jev 側の記録の欠けなので、この ADR では扱わない（進捗の「提案」に書く）。

## 影響
- manifest に `budget.token_basis` が要る（無い manifest は読み込みエラー）。manifest hash は変わる。
- 既存の pilot 出力（trials.jsonl）は新しい欄を持たないので再利用しない（pilot は新しい out で回し直す）。
