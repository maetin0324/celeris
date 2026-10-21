---
title: continuation 上限を進捗に合わせる（ADR-0072 D18 の改定）
updated: 2026-10-20
tasks: [01M5C6M6JCKFNDGHP3ZXBVPMR2]
status: running
---
# continuation 上限を進捗に合わせる

ADR: `agent-docs/adr/2026-10-20-continuation-limit-by-progress.md`

## 工程
- [x] fix-docref（統合済み）
- [x] adr-b: ADR を書く（2026-10-20。コードは触らない）
- [x] impl-b: 判定・config・試験の実装（ADR の D1〜D5 と「試験」節。2026-10-20 完了）
- [ ] 統合

## adr-b の結果（2026-10-20）
- 現状を base 669c00ab のコードで確認して ADR の「現状」節に書いた。要点:
  - task 経路 `worker_finish.rs:1094` と WU 経路 `execution_scheduler.rs:195` の両方で、回数の上限（既定 3）が進捗を見ずに
    人に聞く。文言はどちらも「進みません」。
  - WU の `wu.continuations` は人の回答で 0 に戻らない（D18「回答の時点から数え直す」と食い違い）。「続ける」と答えても次の
    continuable な終わりで再び止まる。
  - WU の `blocked(limit)` は replan に回る（「made no progress」）。回数で止まっただけの WU も replan される。
- 決定: 回数だけの上限を消し、進捗なし（`no_progress_limit`、既定 2）と固い上限（新欄 `continuation_hard_limit`、既定 12）
  だけで人に聞く。固い上限の文言は回数・進捗あり回数・commit 数・変更 file 数を示す。WU は新しい
  `WorkUnitBlockedReason::ContinuationCap`（replan しない、回答で `needs_continuation` から再開）。旧欄
  `max_continuations_per_work_unit` は読み込みエラー。

## 証拠
- `git diff --name-only $CELERIS_WU_BASE` → ADR と本ファイルの 2 件だけ（コードの変更なし）。
- 本番 config の確認（読み取りのみ）: `grep -n "^\[execution\]" -A12 ~/.config/celeris/config.toml` → `[execution]` は
  `parallel` と `gate` だけ。`max_continuations_per_work_unit` も `no_progress_limit` も書いていない。

## 本番への移行
- impl-b を含む release を昇格する前に、運用者が `grep -n max_continuations_per_work_unit ~/.config/celeris/config.toml` を
  実行し、何も出ないことを確かめる（2026-10-20 時点では出ない）。出たらその行を消す（残すと読み込みエラーで daemon が起動しない）。
  旧い値を固い上限として残したいときだけ `[execution] continuation_hard_limit = <n>`（1 以上）を書く。
- DB の migration は無い（`work_units.blocked_reason` は TEXT 列。新しい値 `continuation_cap` が入るだけ）。

## 未解決事項
- 固い上限の既定 12 は運用の実績から決めた値ではない（1 run の予算 × 13 run ≈ 9 時間弱で暴走を止める見積もり）。運用で
  上限到達の質問が多ければ見直す。
- tokens の費用上限は足していない（ADR「採らなかった案」）。

## 提案
- ADR-0072 D18 の表と ADR-0090 の `max_continuations_per_work_unit` への言及に、本 ADR で置き換えた旨の付記を impl-b で足す。

## adr-b attempt 2: check 不合格の原因と修正
- 原因: `sh scripts/dev/progress-index.sh --check` が base（main と同じ）から引き継いだ他 task の進捗ファイル 2 件の front matter で落ちていた（本 ADR・本進捗ファイルは無関係）。
  - `2026-10-19-dev-profile-debuginfo.md`: `task:`/`completed:` の旧形式 → `title`/`tasks: [01M5AH1A1F6YMPCCJTCGBC84H5]`/`status: done`/`updated: 2026-10-19` に直した（本文は不変）。
  - `2026-10-20-research-eval-final-check.md`: `status: partial` は許可外 → 実 pilot が人の操作（H1 昇格等）待ちなので `blocked` に直した。
- 証拠: `sh scripts/dev/check-doc-links.sh agent-docs/adr/2026-10-20-continuation-limit-by-progress.md agent-docs/progress && sh scripts/dev/check-adr-numbers.sh && sh scripts/dev/progress-index.sh --check` → exit 0（`check-doc-links: ok` / `check-adr-numbers: ok (266 files)` / `progress-index --check: ok`）。

## impl-b の結果（2026-10-20）
- task 経路（`worker_finish.rs`）と WU 経路（`execution_scheduler::continuation`）を ADR D2 に置き換えた。進捗なし連続（`no_progress_limit`）を先に、
  次に固い上限（`continuation_hard_limit`、既定 12、最後の `answer` 以降の continuation 回数）で聞く。文言は
  `continuation_cap_question` / `no_progress_question`（`execution_scheduler.rs`）の 1 か所。
- 固い上限の WU は新 `WorkUnitBlockedReason::ContinuationCap`（reason `continuation_cap`）。replan に回さず、回答で `needs_continuation` から再開。
  `settle_phase`・`runnable_in_phase`・`resume_after_answer`・inbox/gate/inbox_notifications・liveness・replay の写像に追加。
- `RepoState.ahead`（`base..HEAD` の commit 数、`#[serde(default)]`）を足し、checkpoint.rs が入れる。`task_ops::derive` に
  `wu_continuations_since_answer`・`progress_checkpoints_since_answer`。
- config: `max_continuations_per_work_unit` を削除（`deny_unknown_fields` で読み込みエラー）、`continuation_hard_limit`（0 は Invalid）を追加。
  旧欄を使っていた既存試験の行（上限を逃がす設定）は消した。ADR-0072 の表・ADR-0090 に置換の付記。
- schema 再生成: `UPDATE_SCHEMA=1`（task-core・task-api）→ `docs/api/v1`・`docs/protocol` と `web/api/generated`（`pnpm gen:types`）。

## impl-b の証拠（コマンドと結果）
- `cargo test -p task-dispatch -p task-ops -p task-core`（UPDATE_SCHEMA 後）→ 失敗 0。
- 新しい試験（`crates/task-dispatch/src/dispatcher/tests/continuation_cap.rs`、`execution_scheduler/tests.rs`）→ 通過:
  - 進捗ありなら旧上限 3 を超えて続き、固い上限（試験では 6）で初めて聞く（task・WU）。文言は「上限」「continuation 6 回」「進捗あり」を含み「進みません」を含まない。
  - 進捗なしが 2 回続くと聞く（文言「進捗なしが 2 回続きました」）。
  - 固い上限の WU は `blocked(continuation_cap)`、plan は 1 件のまま（replan しない）、回答後に窓が 0 から数え直される（`wu.continuations` 12、窓 6）。
- `cargo test -p celeris --lib config` → 169 通過。`execution_continuation_hard_limit_default_validation_and_old_field_is_an_error`
  が既定 12・0 は Invalid・旧欄 `unknown field` エラーを確かめる。
- `cargo clippy --workspace --all-targets -- -D warnings` exit 0、`cargo fmt --all -- --check` exit 0、`sh scripts/dev/progress-index.sh --check` ok。
- `cargo test -p task-api` のうち `browser_live_cross`（3 件）は `bind: path must be shorter than SUN_LEN`（作業ディレクトリのパスが長い環境要因。本変更と無関係）で落ちる。workspace 全体の試験は走らせていない（release 時のみ）。
