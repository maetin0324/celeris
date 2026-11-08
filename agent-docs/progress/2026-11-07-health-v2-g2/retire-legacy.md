---
title: 健全性v2 G2 retire-legacy — prompt.rs の旧い写し（D7.1 の書く側の 1 文、D7.4 の 2 箇条と本番 host の planner 用の節）を消す
tasks: [01M6XH68AB1AFP72M9N94JFSSY]
status: done
updated: 2026-11-08
completed: 2026-11-08
---
# 健全性v2 G2 retire-legacy — prompt.rs の旧い写し（D7.1 の書く側の 1 文、D7.4 の 2 箇条と本番 host の planner 用の節）を消す

完了日: 2026-11-08

WorkUnit `retire-prompt`（task 01M6XH68AB1AFP72M9N94JFSSY）。[ADR 2026-11-07 health-review-integration](../../adr/2026-11-07-health-review-integration.md) の (7) D7.1・D7.4 を、付記「二度目の final review の差し戻しへの方針」の (e) E2〜E4 のとおりに撤去した。ADR は編集していない。基準は `$CELERIS_WU_BASE`（統合後は `git merge-base HEAD main`）。

## 変更
- commit の件名: 「G2 retire-legacy: prompt.rs から commit 参照の書く側の 1 文・check の書き方の 2 箇条・本番 host の planner 用の写しを消す」、「G2 retire-legacy: 進捗に変更・検査の結果・目的文と付記の違い・提案を書く」。
- `crates/task-worker/src/claude_code/prompt.rs`
  - D7.1（E2）: reviewer prompt の「Commit references in progress files and reports」節から、書く側の規則を写した 1 文（"Progress files and reports name commits by subject and the base as `git merge-base HEAD main` (`$CELERIS_WU_BASE` for a work unit), and record checks as run on the HEAD at submission."）を消した。見出しと判定の規則（hash の食い違いで不合格にしない、件名を `git log $(git merge-base HEAD main)..HEAD` で照合する、検査は再実行か決定的な check で判定する）は残した。節の文は子 module `claude_code/prompt/commit_refs.rs` の定数 `REVIEW_SECTION` に移し、`build_review_prompt` はそれを出す（理由は下の「目的文・付記との違い」）。
  - D7.4（E3）: `PLANNER_CHECK_GUIDANCE` から「Records follow ADR-0128 …」（記録の場所・commit を件名で書く・検証 SHA の記録を条件にしない）と「Whole-workspace tests … run only in the release gate …」の 2 箇条を消した。`CELERIS_WU_*`・scope check の箇条と「Do not run CPU-burning load scripts …」の箇条は残した。
  - D7.4（E3）: 本番 host の planner 用の定数（`### 本番 host の操作` の節）と、`build_execution_plan_prompt` のその呼び出し・直前の注釈を消した。planner の prompt には `prompt_header` → `preamble::render` → `preamble::fixed_notes` の「## 本番 host の操作 (production host changes)」節が入り、計画では人の決定・人の check を置くことまで書いている。消す前に、新しい試験 `change_rules_retire_planner_prompt_has_the_production_host_note_once` を走らせ、v2 の prompt に `production_host_note()` の文がちょうど 1 回あること（その assert は通り、planner 用の写しの `###` 見出しが残っていた所で落ちる）を確かめてから消した。
  - 行数: 1,886 行から 1,859 行。
- `crates/task-worker/src/claude_code/prompt/commit_refs.rs`（新規、15 行）: reviewer prompt の commit の参照の節。判定の規則だけを持ち、書く側の規則は repo の必須規則が持つことを注釈に書いた。
- `crates/task-worker/src/claude_code/tests.rs`
  - 試験 3 件（prefix `change_rules_retire_`）:
    - `change_rules_retire_review_prompt_keeps_only_the_commit_judging_rule`: review context の有無の 2 通りで、reviewer prompt に節の見出しが 1 回あり、節に判定の規則の 4 文があり、書く側の規則の文（件名で書く・base の書き方・`$CELERIS_WU_BASE`・提出時の HEAD）が無い。
    - `change_rules_retire_planner_prompt_has_the_production_host_note_once`: v2・v3 の planner の prompt に `preamble::production_host_note()` の文がちょうど 1 回あり（`fixed_notes` に含まれることも確かめる）、`## 本番 host の操作` が 1 回で、`### 本番 host の操作` が無い。
    - `change_rules_retire_planner_check_guidance_drops_the_repo_rule_copies`: 「check の書き方」節に消した 2 箇条の文が無く、scope check・`CELERIS_WU_*`・`CELERIS_WU_SCOPE_PATHS`・CPU 負荷の箇条がある。
  - 既存の `planner_prompt_declares_production_host_changes_as_a_human_procedure`（planner 用の写しの `###` 見出しを固定していた）は消し、上の 2 件目に置き換えた。
  - 既存の `planner_prompt_has_the_check_writing_section` の needles から、消した 2 箇条の文 6 件（ADR の置き場所、`agent-docs/PROGRESS.md`、commit を件名で書く、検証 SHA、release gate、`test-affected.sh` の説明）を外した。land 系 check の 3 本の needle と注釈は残した。
  - 既存の `review_prompt_includes_human_decisions_and_check_results` は残した文（見出しと判定の規則）だけを確かめているので変えていない。

## 目的文・付記との違い
- 目的文（task の Objective）は D7.1 を「節を消す」、D7.4 を「CPU を焼く負荷の禁止も消す」と書く。後から入った付記 E2・E3 が決定を改めたので、付記に従った。
  - E2: 判定の規則（古い hash を理由に不合格にしない）は reviewer prompt のこの節にしか無く、review 前の rebase（ADR-0118）はどの repo の task にも起きる。ほかの repo の `CLAUDE.md` にこの規則は無い。そのため見出しと判定の規則を残し、書く側の 1 文だけを消した。
  - E3: CPU 負荷の箇条は人が決めた共用 host の規則（KB `user/stress-shared-host-rules.md`）で、ほかの repo の planner に届く写しはこの箇条だけ。消すと届かなくなるので残した。
- task の受け入れ条件 3（`! grep -n 'Commit references in progress files' crates/task-worker/src/claude_code/prompt.rs`）は元の D7.1 に合わせて書かれ、付記 E2（節を残す）と食い違う。両方を満たすため、節の文を prompt.rs から子 module `claude_code/prompt/commit_refs.rs` に移した。reviewer prompt に出る文は、消した 1 文を除いて変わらない。prompt.rs は 2,000 行の上限に近いので、行数も減る。reclose が ADR に写すとき、節の置き場所がこの file に変わったことを書く必要がある。
- 付記 E4 の試験は「`PLANNER_CHECK_GUIDANCE` に消した 2 箇条が無い」を挙げる。この実装は定数を直接読まず、`build_prompt` で作った planner の prompt の「check の書き方」節で確かめた。
- 付記 E3 は本番 host の節の 1 回の確認を「/3 の execution plan の prompt」と書く。試験は v2（/2）と v3（/3）の両方で確かめた。

## 検査（提出時の HEAD で実行）
- `bash scripts/dev/test-affected.sh`: exit 0。先頭の check-health は `verdict=pass exit=0 … new=0 worsened=0`。対象 crate は celeris・celeris-campaign・celerisctl・llm-proxy・task-api・task-dispatch・task-worker。nextest は 5,717 件合格・22 件 skip。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `cargo fmt --all -- --check`: exit 0。
- `cargo nextest run -p task-worker -E 'test(/change_rules_retire_/)'`: 3 件合格。`cargo nextest list … | grep -c change_rules_retire_`: 3。
- 本番 host の planner 用定数の名前を `crates/` で grep: 0 件（exit 1）。
- `grep -nE 'Records follow ADR-0128|Whole-workspace tests|name commits by subject and the base|### 本番 host の操作|Commit references in progress files' crates/task-worker/src/claude_code/prompt.rs`: 0 件。
- 範囲 check（`$CELERIS_WU_SCOPE_PATHS`）: 範囲外の path なし。
- `sh scripts/dev/check-doc-links.sh`・`sh scripts/dev/progress-index.sh --check`: exit 0。

## 未解決事項
- task の受け入れ条件 3 と付記 E2 の食い違いは、節を子 module に移して両方を満たした。reclose は ADR の付記に、節の正本の置き場所（`claude_code/prompt/commit_refs.rs`）を書く。
- ADR の状態行と D7.1・D7.4 の付記への反映は reclose が行う（この unit は ADR を編集していない）。

## 提案
- E3 の 1: 共用 host の規則（CPU を焼く負荷の禁止）を `preamble::fixed_notes` の 1 か所に移すかは人（operator）が決める。移せば全 harness・全 run の前置きに届き、planner 用の箇条と `CLAUDE.md` の写しを同じ変更で消せる（付記 E3 の撤去条件）。移さなければ今のまま planner の「check の書き方」の箇条が、ほかの repo に届く唯一の写しになる。
- E3 の 2: `PLANNER_CHECK_GUIDANCE` に残る agent-platform 固有の箇条（land 系 check の `sh scripts/dev/check-doc-links.sh`・`check-adr-numbers.sh`・`progress-index.sh --check`、`corepack pnpm@<version>`、`cargo fmt --all -- --check`、`web/`・`docs/` だけの変更に `crates/` の diff の check を使う箇条）は、ほかの repo では当たらない path を指す。profile（`scripts/dev/repo-health/profiles/celeris.toml`）の `[review]` 節などの repo 固有の置き場所へ移し、planner には worker profile（付記 (b)）の経路で届ける案を出す。移す変更は policy の file を変えるので、D6 の policy の branch と人の承認を通す。
