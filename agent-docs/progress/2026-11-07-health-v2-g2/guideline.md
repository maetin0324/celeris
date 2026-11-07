---
title: 健全性v2 G2 guideline — 汎用規約・profile の [review] 節・source-health.md
tasks: [01M6VCR06J339SH4JTK3Q8XHEP]
status: done
updated: 2026-11-07
completed: 2026-11-07
---
# 健全性v2 G2 guideline — 汎用規約・profile の [review] 節・source-health.md

完了日: 2026-11-07

task「汎用規約本文と Celeris profile の [review] 節を書く」の記録。根拠は [ADR 2026-11-07 健全性を設計と review に組み込む](../../adr/2026-11-07-health-review-integration.md)（以下 ADR）の (1)・(2)・(6)・D7.3。基準は `git merge-base HEAD main`（WorkUnit は `$CELERIS_WU_BASE`）。unit ごとの詳細は [generic](guideline-generic.md)・[profiles](guideline-profiles.md) にある。

## 3 unit の成果
- generic: `crates/task-worker/src/change_rules/generic.md` を新規に書いた。`## 規約` に 10 項目、`## 実装前 brief` に 7 項目、末尾に repo の必須規則と健全性の guide を参照する 1 文。repo 名・Celeris の固有名を含まない。配線（`change_rules.rs`・`prompt_header`）は U1 の範囲で、まだどこからも読まれない。
- profiles: `scripts/dev/repo-health/profiles/celeris.toml` に `[review]` 節（`owner_map = "docs/architecture-map.md"`、high_risk 7 class: credential・sandbox・store・events・lease・budget・selfdeploy、invariants 13 件）、`profiles/default.toml` に下位項目の無い `[review]`、`health/profile.py` に節の検証、試験 `scripts/tests/test_repo_health_review_policy.py`（8 件）。policy の file だけの branch で、人の承認の後に operator が main に merge した（件名「integrate 01M6VCXJCX: profile の [review] 節（policy の branch、人の承認済み。ADR health-review-integration D6.6）」）。
- guide: 件名「G2 guide: source-health.md を入口と参照先に縮める（front matter を先頭に）」の commit で `docs/guides/source-health.md` を書き直した。
  - front matter を 1 行目に移し、この task の id を足した。
  - 入口は `bash scripts/dev/check-health.sh`（report JSON の path を出す、exit 0/1/2）。base は `--base auto`（`git merge-base HEAD main`）。既存検査の command と上限は `profiles/celeris.toml`。policy の file を変える branch は `policy.changed` で落ち、通し方は ADR の (6)。汎用規約は `generic.md`、review 方針は profile の `[review]` 節。説明は ADR・`check-health.sh` の冒頭注記・`profile.py` の docstring を参照し、写していない。
  - 古くなった文を消した: 比較元を `CELERIS_WU_BASE` から読むという記述（[Celeris profile の ADR](../../adr/2026-11-07-repo-health-celeris-profile.md) D1 で、どの検査も読まなくなった）、導入当時の task 5 と起票の経緯。基準・閾値の変更の「独立レビュー」は ADR (6) への参照にした（D6.6 の最後の文）。
  - 2026-10-23 の変更の記録への link（`../progress/2026-10-23-source-health.md`）は残した。この link は `docs/progress/2026-10-23-source-health.md`（追跡されている）に解決し、壊れていなかった。`check-doc-links.sh` は変更の前も後も exit 0。
  - `CLAUDE.md` は変えていない。guide は `CLAUDE.md` を場所として挙げるだけで、規則の文は写していない。

## generic.md の項目数を 10 にした理由
ADR D1.4 は 7 項目（D5.1 の finding 種別と同じ順・同じ名前）と書く。task の受け入れ条件と計画の leaf の完了条件は「10 項目と brief 7 項目」で、task の目的は 7 項目に 3 項目（移行は撤去の条件と責任者を書く、共通化は同じ意味のものだけ、範囲で根本修正できなければ repair/replan）を足している。final review は task の受け入れ条件で判定するので、generic.md は 10 項目にした。D1.4 の 7 項目は規約 1〜6 と 10 に D5.1 の kind 名を括弧で付け、D5.1 の表と同じ順に置いた。規約 7〜9 には対応する kind が無い。

## check-health の結果（提出時の HEAD で実行）
- `bash scripts/dev/check-health.sh` → exit 0。verdict pass、`fail_grade=0 new=0 worsened=0 existing=142`。mode は doc-only、base は `git merge-base HEAD main`、policy の source は base。
- fail 級の finding は 0 件。`policy.changed` は pass（`policy.changed=false`）。existing の 142 件は arch-map.*（missing_row 1・missing_test 77・wrong_owner 32・missing_invariant 32）で、base にもあり合否に入らない。
- task の目的は「profile の変更で `policy.changed` が出て、main 取り込みまで落ちる」と見込んでいた。実際には profiles の branch が先に main に merge され、基準の merge-base がその先端になったので、この branch の差分（`generic.md`・`source-health.md`・進捗）に policy の file は無い。
- ADR (6) D6.6 のとおり、profile を変えた branch（profiles）は人の承認で通した。`check-health.sh` に D6.2（policy の file だけの branch を exit 0 にする扱い）がまだ無いので、承認だけでは必須検査が通らず、operator が branch を main に merge して通した（[profiles の記録](guideline-profiles.md)）。

## 検査（提出時の HEAD で実行）
- `bash scripts/dev/test-affected.sh` → exit 0。先に呼ぶ check-health は上と同じ pass、続けて「試験対象の変更なし（docs のみ等）」。
- `cargo clippy --workspace -- -D warnings` → exit 0、`cargo fmt --all -- --check` → exit 0（Rust の変更は無い）。
- task の受け入れ条件 0（`generic.md` に固有名が無く、両 profile に `[review]`）→ exit 0。
- task の受け入れ条件 1: `python3 -I -m unittest discover -s scripts/tests -p 'test_repo_health*.py'` → 62 件 OK、`sh scripts/dev/tests/repo_health_gate.sh` → 5 件 OK、exit 0。
- task の受け入れ条件 2: `sh scripts/dev/check-doc-links.sh && sh scripts/dev/check-adr-numbers.sh && sh scripts/dev/progress-index.sh --check` → exit 0。
- `CLAUDE.md` の差分: `git diff --quiet $(git merge-base HEAD main) HEAD -- CLAUDE.md` と `$CELERIS_WU_BASE` 基準の同じ検査 → どちらも exit 0。
- 範囲: `sh "$CELERIS_WU_SCOPE_PATHS"` → `docs/guides/source-health.md` とこの進捗の 2 file だけ。

## 未解決事項
- D6.2 は未実装。G2 の残りの policy の変更（U2 の `--print-review-policy`、`check-health.sh` の D6.2、D7.3 の `check-health.sh` 冒頭注記の整理）を載せる branch は、main に入るまで `check-health.sh` と `test-affected.sh` が `policy.changed` で落ちる。profiles と同じく、人の承認と operator の手の merge が要る。
- D7.3 のうち `check-health.sh` の冒頭注記と `profile.py` の docstring の整理はこの unit で行っていない。どちらも policy の file で、ADR の U6 の記述どおり U2 と同じ branch に入れる。

## 提案
- ADR に付記を足す: `generic.md` の規約は 10 項目で、D5.1 の 7 kind に当たるのは規約 1〜6 と 10。規約 7〜9 の違反を reviewer がどう返すか（kind を足すか、advisory に限るか）は未決で、U5 の前に決める。kind を足すなら D5.1 の表・`HealthFindingKind`・`generic.md` を同じ変更で変える。
- `docs/progress/2026-10-23-source-health.md` は ADR-0128 の配置（進捗は `agent-docs/progress/`）から外れている。移すなら `git mv` で `agent-docs/progress/` に移し、`docs/architecture-map.md` の 2 行（:211・:212）、`source-health.md` の link、移した file の中の guide への link を同じ変更で直す。別の task にする。
