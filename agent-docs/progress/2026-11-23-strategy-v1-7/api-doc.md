---
tasks: [01M7XQ9R1EHA8GQ0TZVRM4FBVX]
wu: api-doc
status: done
completed: 2026-11-24
---
# 戦略層 v1 子 7 api-doc: API 仕様 docs/api/v1/api.md に子 7 の 3 つの口と新しい拒否を書く

## 結論
- final review（v3）の不合格の理由 (2)「`docs/api/v1/api.md` が未更新」を直した。変えたのは `docs/api/v1/api.md` とこの進捗だけで、`crates/` の差分は 0。
- 書いたこと:
  - 冒頭の改訂の記録に「改訂: 2026-11-24（ADR 2026-11-23-project-observability-publish）— 追加。v1 のまま。」の項。
  - §2 の表に 246 `GET /projects/{id}/overview`・247 `POST /chat/threads/{t}/messages/{m}/review`・248 `GET /tasks/{id}/publish/{manifest_id}`（表の最後の番号 245 に続けた）。§3.128 の chat の表に review の行。
  - 新しい節 §3.135（3.135.1 概要: 応答の型・14 項目と導く元・印 4 語・警告の code・link 7 種と、実験の task の `log`〈O5・O9・O10〉・`notebook`〈O5〉・`task_id` 付きの `git`〈O9〉・エラー。3.135.2 案の採否: 本文・権限・状態・エラー。3.135.3 manifest: 応答の型・署名の状態・違反の code・エラー）。
  - §3.125.13 の `POST /decisions/{id}/answer`・`revise` に、`publish-signoff-v<n>` の決定への人以外の回答は 403 `publication_requires_human`、人の `sign` で検査の違反は 422 `publish_manifest_invalid`、file の今の sha256 が問いと違えば 409 `publish_manifest_stale`、CoS の operation `decision.answer`・`decision.revise` と MCP の `decision_answer` も同じ owner（`task_ops::decision` の `signoff_actor`）を通ること。

## 実装前 brief
1. 根本原因: 子 7 の backend の WU が route を足したとき、API 仕様の正本 `docs/api/v1/api.md` を更新しなかった。
2. 責務 owner: `docs/api/v1/api.md`（§2 の表・§3 の本文・冒頭の改訂の記録）。
3. 既存の再利用先: 子 3・子 4 が同じ文書に足した書式（表の行・`ADR … D<n>` の書き方）。code は crates の実装（`problem.rs`・`publish.rs`・`chat/proposal_review.rs`・`task_core::proposal_review`・`task_core::publish`・`task_ops::project_overview`）から grep で写した。
4. 選択肢: 既存の §3.47 や §3.128 に散らして書く案と、1 つの節 §3.135 にまとめ表と §3.125.13 から参照する案。ADR 1 本の 3 口なので後者にした。
5. 新しい状態・分岐・fallback・port: 無し（文書だけ）。
6. 撤去対象: 無し（旧い口の記述は無い）。
7. 不変条件: 文書の code と status は crates の実装にあるものだけ。下の検査で確かめた。

## 証拠（提出時の HEAD で実行）
- `for c in publication_requires_human proposal_verdict_requires_human publish_manifest_invalid publish_manifest_stale proposal_review_stale publish_manifest_not_found proposal_review_not_a_proposal; do grep -c "$c" docs/api/v1/api.md; git grep -c "\"$c\"" -- crates; done` → 7 つの code はすべて文書に 1 件以上、crates の文字列定数に 1 件以上ある。
- 403（`problem.rs` の `PublicationRequiresHuman` → `StatusCode::FORBIDDEN`、`proposal_review.rs` の `reviewer`）、422・409（`publish.rs` の `check_before_sign`）、409・422（`proposal_review.rs` の `review_problem`）、404（`manifest_status` の `not_found`）、503 `cos_unavailable`（`chat/mod.rs` の `require_cos_enabled`）を実装で確かめた。
- `grep -qF` で 3 つの口・`| 246 |`〜`| 248 |`・改訂の記録の項 → すべて ok。
- `git diff --quiet "$CELERIS_WU_BASE" -- crates/` → exit 0。
- `bash scripts/dev/check-doc-links.sh` → exit 0（`check-doc-links: ok`）。`sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` → exit 0。`sh scripts/dev/progress-index.sh --check` → exit 0。
- `bash scripts/dev/test-affected.sh`（基点 `$CELERIS_WU_BASE`。対象は docs/api/ の変更による web の typecheck と unit だけ）→ exit 0（tsc -b 通過、vitest 108 file・771 passed、node --test 110 pass・0 fail、repo_health_gate all checks passed）。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。

## commit
- 「strategy-v1 7 api-doc: API 仕様に概要・案の採否・manifest の口と署名の拒否を書く」
- 「strategy-v1 7 api-doc: 提出時の HEAD の検査の結果を進捗に書く」

## 未確認
- 受信箱の answer が `POST /decisions/{id}/answer` に委ねる経路は、この WU では grep で確かめられなかったので文書に書いていない。
- `docs/api/v1/api-v1.schema.json` の `$defs` は backend の WU が生成した物をそのまま使い、この WU では照合していない。
