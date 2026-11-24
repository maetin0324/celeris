---
title: 戦略層 v1 子 7 close-verify — 提出時の HEAD での全試験の再実行と task の進捗の確定
tasks: [01M7XQ9R1EHA8GQ0TZVRM4FBVX]
wu: close-verify
status: done
updated: 2026-11-24
completed: 2026-11-24
---
# close-verify（WorkUnit。実装しない。変えたのは進捗の 2 file だけ）

完了日: 2026-11-24。task の進捗は [2026-11-23-strategy-v1-7](../2026-11-23-strategy-v1-7.md)。前の WU は [close-audit](close-audit.md)。
基準は task 全体の差分が `git merge-base HEAD main`、この WU の差分が `$CELERIS_WU_BASE`。

## したこと

- close-audit の後の HEAD で (a)〜(f) を foreground で実行した。すべて exit 0。
- task の進捗の「## 測定」「## 試験」（提出時の HEAD の全試験）「## 回帰」「## 受け入れ条件ごとの証拠」に結果を書き足し、front matter を `status: done`・`completed: 2026-11-24` にした。
- commit: `strategy-v1 7 close: 提出時の HEAD で全試験を再実行し、task の進捗を done にする`。

## 実行した command と結果（提出時の HEAD で実行）

- (a) `cargo nextest run -p task-core -p task-ops -p task-api -p celeris-mcp -E 'test(/publish_manifest_|publish_signoff_|proposal_review_|project_overview_|cos_ops_registry_/)' --no-tests=fail`
  → exit 0、`79 tests run: 79 passed, 3249 skipped`。内訳（`cargo nextest list` の同じ filter）: `publish_manifest_` 15・`publish_signoff_` 17・`proposal_review_` 23・
  `project_overview_` 19・`cos_ops_registry_` 5。
- (b) `corepack pnpm@12.6.0 -C web install --frozen-lockfile --offline && corepack pnpm@12.6.0 -C web exec playwright test e2e/work/project-overview.spec.ts e2e/chat/proposal-review.spec.ts e2e/work/publish-manifest.spec.ts`
  → exit 0、`14 passed`（project-overview 2・proposal-review 7・publish-manifest 5）、failed 0・skipped 0。偽 daemon に対する実行。
- (c) `cargo fmt --all -- --check` → exit 0、出力 0 行。
- (d) `cargo clippy --workspace -- -D warnings` → exit 0。warning は依存 crate `deno_core` の deprecated（`TcpStream::set_linger`）1 件だけ（workspace の外）。
- (e) `bash scripts/dev/test-affected.sh` → exit 0、`test-affected: 試験対象の変更なし（docs のみ等）`。repo-health は `mode=full`・基点 `git merge-base HEAD main` で
  `verdict=pass exit=0 fail_grade=0`（findings 727: new 5・worsened 2・existing 720。required 15 項目・optional 6 項目すべて pass）。
  `repo_health_gate: all checks passed`、repo-health の自己試験 126 件 OK。
- (f) `sh scripts/dev/check-doc-links.sh && sh scripts/dev/check-adr-numbers.sh && sh scripts/dev/progress-index.sh --check`
  → exit 0（`check-doc-links: ok`・`check-adr-numbers: ok (481 files)`・`progress-index --check: ok`）。commit の後にも実行した。
- task の受け入れ条件 1 の式（ADR の保証の表の試験名・3 つの spec・migration の差分 0）→ exit 0、名前 74 件、欠け 0。

## 範囲

- 変えた file: `agent-docs/progress/2026-11-23-strategy-v1-7.md` とこの file。crates・web・scripts・docs の差分は 0（`sh "$CELERIS_WU_SCOPE_PATHS"` で確かめた）。
- e2e の実行が作る `web/dist/` 等は追跡外で、`git status --short` は空のまま。

## 未確認・引き継ぎ

- (e) はこの WU の差分（進捗だけ）を基点にするので、task 全体の差分の Rust・web の試験は流していない。task 全体の差分の試験は各 WU の test-affected
  （task の進捗の「## 回帰」）と final review の再実行に任せる。
- ほかは task の進捗の「## 未確認」。

## 提案

- 無し。
