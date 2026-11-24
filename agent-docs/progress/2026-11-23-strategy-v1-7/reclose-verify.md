---
title: 戦略層 v1 子 7 reclose-verify — 修正後の提出時の HEAD での全試験の再実行と task の進捗の確定
tasks: [01M7XQ9R1EHA8GQ0TZVRM4FBVX]
wu: reclose-verify
status: done
updated: 2026-11-24
completed: 2026-11-24
---
# reclose-verify（WorkUnit。実装しない。変えたのは進捗の 2 file だけ）

完了日: 2026-11-24。task の進捗は [2026-11-23-strategy-v1-7](../2026-11-23-strategy-v1-7.md)。前の WU は [reclose-audit](reclose-audit.md)。
基準は task 全体の差分が `git merge-base HEAD main`、この WU の差分が `$CELERIS_WU_BASE`。

## したこと

- reclose-audit の後の HEAD（修正の段 fix-backend・fix-surface の統合を含む）で (a)〜(f) と ADR の保証の表の check を foreground で実行した。すべて exit 0。
  落ちた試験は無い。
- task の進捗の「## 試験」の「提出時の HEAD の全試験」と「## 受け入れ条件ごとの証拠」を、v3 の close-verify の値から提出時の HEAD の値に書き直した。
  「## 測定」「## 回帰」の該当行と冒頭の状態の段落・WU の一覧も合わせた。front matter は `status: done` のまま、`updated`・`completed` は実行した日（2026-11-24）。
- commit: `strategy-v1 7 reclose-verify: 提出時の HEAD で全試験を再実行し、task の進捗に結果を書く`。

## 実行した command と結果（提出時の HEAD で実行）

- (a) `cargo nextest run -p task-core -p task-ops -p task-api -p celeris-mcp -E 'test(/publish_manifest_|publish_signoff_|proposal_review_|project_overview_|cos_ops_registry_/)' --no-tests=fail`
  → exit 0、`83 tests run: 83 passed, 3250 skipped`。内訳（`cargo nextest list` の同じ filter）: `publish_manifest_` 15・`publish_signoff_` 17・`proposal_review_` 23・
  `project_overview_` 23（`project_overview_drilldown_` 4 を含む）・`cos_ops_registry_` 5。
- (b) `corepack pnpm@12.6.0 -C web install --frozen-lockfile --offline && corepack pnpm@12.6.0 -C web exec playwright test e2e/work/project-overview.spec.ts e2e/chat/proposal-review.spec.ts e2e/work/publish-manifest.spec.ts`
  → exit 0、`14 passed`（project-overview 2・proposal-review 7・publish-manifest 5）、failed 0・skipped 0。偽 daemon に対する実行。
- (c) `cargo fmt --all -- --check` → exit 0、出力 0 行。
- (d) `cargo clippy --workspace -- -D warnings` → exit 0。warning は vendor の `deno_core`（workspace の member でない）の deprecated（`TcpStream::set_linger`）1 件だけ。
- (e) `CELERIS_WU_BASE= bash scripts/dev/test-affected.sh`（基準 `git merge-base HEAD main`。task 全体の差分）→ exit 0。Rust は 14 crate で
  `8902 tests run: 8902 passed (9 slow), 34 skipped`、doc test 14 個の `test result: ok`（FAILED 0）。web は `tsc -b` exit 0・vitest `Test Files 108 passed`・
  `Tests 773 passed`・`node --test` `pass 110`・`fail 0`。`test-ts-verify: 20 passed, 0 failed`、script bundle の SKIP 0 件。
  repo-health `verdict=pass exit=0 fail_grade=0`（findings 725: new 3・worsened 2・existing 720。required 15 項目・optional 6 項目すべて pass）、
  `repo_health_gate: all checks passed`。所要は約 15 分（07:28Z〜07:43Z）。
- (f) `sh scripts/dev/check-doc-links.sh && sh scripts/dev/check-adr-numbers.sh && sh scripts/dev/progress-index.sh --check`
  → exit 0（`check-doc-links: ok`・`check-adr-numbers: ok (481 files)`・`progress-index --check: ok`）。進捗を書き直した後と commit の後にも実行した。
- ADR の保証の表の check（task の進捗の「## 再現」の 1）→ exit 0。名前 78 件、`missing` 0 件、3 つの spec は表にあり file も在る。
  `git diff --quiet $(git merge-base HEAD main) -- crates/task-core/migrations/` → exit 0。

## 範囲

- 変えた file: `agent-docs/progress/2026-11-23-strategy-v1-7.md` とこの file。crates・web・scripts・docs の差分は 0（`sh "$CELERIS_WU_SCOPE_PATHS"` で確かめた）。

## 未確認・引き継ぎ

- (e) の log の末尾に `rm: cannot remove …/own-eval: Permission denied` が 2 行出た。scratch の掃除の失敗で、exit code と試験の結果には影響しない。
- web の e2e は偽 daemon に対して流した。本物の daemon・gateway での画面と本番の大きさでの概要の応答時間は未確認（task の進捗の「## 未確認」）。

## 提案

- 無し。
