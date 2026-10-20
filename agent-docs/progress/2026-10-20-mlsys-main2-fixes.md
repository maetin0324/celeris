---
title: "最小 pilot（mlsys-main-2）の不具合: 予算切れ後の再開・B 成果物・decision-log の欠け・runbook"
tasks: [01M5C8KF8Y3MB6TT064AYV2F23]
status: done
updated: 2026-10-20
completed: 2026-10-20
---
# mlsys-main-2 不具合修正の記録（close）

決定は `agent-docs/adr/2026-10-20-mlsys-main2-fixes.md`（D1〜D5）。WU ごとの記録は `2026-10-20-mlsys-main2-fixes/{budget,declog,materials}.md`。
試験は LLM を使わない（stub worker・偽 transport）。本番 host は触っていない。

## 項目ごとの実行コマンドと結果（統合後 HEAD で再実行）

| # | 項目 | コマンド | 結果 |
|---|---|---|---|
| 1 | 予算切れ後の再開の抑止・途中観測（D1・D5） | `cargo test -p task-dispatch token_budget` | 3 passed（`token_budget_stops_run_midway`、`token_budget_exhausted_no_continuation_or_review` ほか） |
| 1 | 失敗 reviewer の usage（D5） | `cargo test -p task-dispatch failed_reviewer_run_records_usage` | 1 passed |
| 1 | 累積 usage（UsageProgress）（D1-3） | `cargo test -p task-worker usage` | 10 passed |
| 1 | Budget.max_tokens | `cargo test -p task-core budget_max_tokens` | 1 passed |
| 1・2 | per_case→max_tokens、trial workspace・repos 空、no_artifact（D1-2・D2） | `cargo test -p celeris-campaign --test celeris_exec` | 20 passed |
| 3 | decision-log 欠け（D3） | `cargo test -p celerisctl --test campaign_connected` | 9 passed |
| 4 | runbook と lock（D4） | `cargo test -p celeris-campaign --test lock_cli` | 9 passed |
| – | campaign 全体（受け入れ 4） | `cargo test -p celeris-campaign` | exit 0（全 binary ok） |
| – | clippy（受け入れ 5） | `cargo clippy --workspace -- -D warnings` | exit 0 |
| – | fmt（受け入れ 6） | `cargo fmt --all -- --check` | exit 0 |

- build2 統合後の `cargo nextest run -p celeris-campaign` 708 pass / 0 fail、celeris campaign e2e 13 pass（運用者の確認、699e3415）。
- 補足: host の sccache 停止を避けるため `SCCACHE_BIN=/nonexistent` で実行した。
- workspace 全体の `bash scripts/dev/test-parallel.sh` は統合段（integrate-wrapup・release gate）で実行する。

## 各項目の要点
1. **予算切れ後の再開**: per_case を task の `Budget.max_tokens` として渡し（D1-2）、claude-code の累積 usage を run の途中で観測して上限で止める。上限到達後は continuation・retry・reviewer を起こさず task は `failed`（`token_budget_exhausted`）。残り予算の内側の再開は許す。超過は最後の 1 応答ぶんまで。
2. **B 成果物**: B/C の task は trial workspace を作業場所にし repos を空にする（scratch repo は廃止）。変化の無い passed は B/C も `no_artifact`。公開 case 定義を `materials/<case>/case.json` に配る。
3. **decision-log**: `grader_verdict` は `celerisctl campaign export --grades` で結合、reviewer の `proposed_first` は routing 判断の無い run では必須にしない（検査規則を直した。根拠は ADR D3 と `declog.md`）。
4. **runbook**: 出力先を `$RUNS`（OUT の外）にし、lock 判定は変えず、拒否に既存 entry 名を出す。

## 未解決・提案
- `docs/ops/measure-run-final.md` §export（181 行付近）の `--verify-connected` 例に `--grades $OUT/grades.jsonl` が無い（`--verify-connected` は `--grades` 必須）。次の runbook 修正で足すこと（この close の範囲外）。
- codex / opencode の adapter は途中 usage を出さないので、超過は 1 run ぶんまで（B/C は claude-code のみで許容、ADR D1-5）。
- A の `model_failure`（出力契約を読めなかった）は materials 配送（D2-4）で対処。実 pilot での再確認は次の campaign で。

## 運用者が行う release・昇格手順（本番 host は触っていない）
1. 統合後の main（本 task のブランチを取り込んだもの）で `bash scripts/dev/test-parallel.sh` が通ることを確認。
2. release 作成と verify（`agent-docs/` の selfdeploy 手順、`knowledge get projects/agent-platform/selfdeploy-release-verify-procedure.md`）。D1・D2・D3(b)・D5 を含む sha12 を控える。
3. 検証済み release を昇格。測定 daemon（旧 302e28550103）をその release に替える。DB migration は無い。
4. 次の campaign の env-lock に新 release sha12 を入れる。

## 本番への移行（測定の config・manifest）
- **task-template から `repos` を外す**（`check_template` が拒否する）。scratch repo の登録手順は不要。
- **`per_case.max_tokens`**: 人が 1,500,000 まで許可済み。次の campaign の manifest に設定すると、そのまま task の `max_tokens` になる（この task では設定しない）。
- **input manifest の prompt を `materials/<case>/case.json` 参照に直した新 manifest を作る**（problem の sha256 と manifest の `manifest_hash` を作り直し、新 campaign id で再 freeze: validate・dry-run・offline-lock・preflight・campaign sha256 の承認をやり直す）。凍結済み `2026-10-16-mlsys-final/final-input-manifest.json` は変えない。`--materials-src` は `<case>/` ディレクトリを含むこと。
- export は `--grades $OUT/grades.jsonl --verify-connected` を付ける。watch・usage・export の出力は `$RUNS=$(dirname "$OUT")/runs` へ（`$OUT` に置かない）。
- 既存の config 欄の削除はなく、読み込みエラーになる旧欄は template の `repos` と `max_tokens` のみ。

## 検査の追補（close 再実行）
- `check-doc-links.sh` が campaign の試験 2 件の素の `docs/…` 文字列（celeris_exec.rs・lock_cli.rs）を壊れた参照と判定したため、path を `join` の分割に直した（挙動は同じ。`cargo test -p celeris-campaign --test lock_cli --test celeris_exec` 20+9 passed）。
- `progress-index.sh --check` が他 task の進捗 2 件の front matter 不備（dev-profile-debuginfo の title/tasks/updated、research-eval-final-check の status: partial）で落ちていたため、front matter だけ直した（本文は不変。後者は done 扱い）。
