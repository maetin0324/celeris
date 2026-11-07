---
title: 健全性v2 G2 profiles — profile の [review] 節
tasks: [01M6VCXJCXEPBQCR5PNR195JCH]
status: done
updated: 2026-11-07
---
# 健全性v2 G2 profiles — profile の [review] 節

完了日: 2026-11-07。実装と人の承認（受け入れ条件 3）は 2 回目の run までに済んだ。必須の repo 検査 `test-affected.sh` は `policy.changed` で落ちていたが、人の決定 `policy-branch-required-check` に従って operator がこの branch を main に merge し（2026-11-07 19:08Z、件名「integrate 01M6VCXJCX: profile の [review] 節（policy の branch、人の承認済み。ADR health-review-integration D6.6）」）、3 回目の run で通った（下の「3 回目の run」）。

unit `profiles`（G2 の工程 write「汎用規約本文と Celeris profile の [review] 節を書く」の子）。[ADR 2026-11-07 健全性を設計と review に組み込む](../../adr/2026-11-07-health-review-integration.md) の D2.1・D2.2・D6.6 に従う。基準は `git merge-base HEAD main`。policy の file だけの branch（D6.1）で、受け入れには人の承認（`{"type":"human"}`）が要る。`--print-review-policy` は作っていない（U2 の別の範囲）。

## 変更
- 件名「G2 profiles: profile の [review] 節と検査」の commit
  - `scripts/dev/repo-health/health/profile.py`: `_TOP_KEYS` に `review`、`_validate_review` を足した。下位項目は `owner_map`（空でない文字列）・`high_risk`（`{class, paths}` の配列。class は空でなく重複不可、paths は 1 件以上の正規表現）・`invariants`（名前 → 根拠の file の表）だけで、ほかは `profile_invalid` の読み込みエラー。docstring（項目の正本）にも書いた。
  - `scripts/dev/repo-health/profiles/celeris.toml`: `[review]` 節。`owner_map = "docs/architecture-map.md"`、high_risk の class は credential・sandbox・store・events・lease・budget・selfdeploy の 7 件（path は地図の行と B0 の capability-map.json の owners から決めた）、invariants は 13 件（`CLAUDE.md` の 6 件と ADR・運用文書の 7 件）。
  - `scripts/dev/repo-health/profiles/default.toml`: 下位項目の無い `[review]` だけを足した。`extends = "celeris"` のまま、merge の結果は celeris の `[review]` と同じになる。
  - `scripts/tests/test_repo_health_review_policy.py`: 8 件（形の検査、未知の下位項目の拒否、celeris の path が追跡 file に当たること、default の merge が celeris の値になること）。
- 件名「G2 profiles: 試験の仮の地図 path を doc-links に当たらない名前にする」の commit — 試験の仮の値 `docs/map.md` を `doc.links` が無い link と読んだので `OWNERS.md` に変えた。

## 3 回目の run（main への merge の後。提出時の HEAD で実行）
基準は `git merge-base HEAD main`。operator の merge でこの branch の 2 回目の先端が main の祖先になり、基準はその先端になった。基準から HEAD の差分はこの進捗の 1 file だけで、policy の file 3 件と試験はこの branch と main で同じ内容（`git diff HEAD main -- scripts/dev/repo-health scripts/tests` が空）。main は取り込んでいない（親 task がまだ main を取り込んでいないため。KB `child-merge-main-before-parent-sync-final-review`）。
- 件名「G2 profiles: main への merge の後の進捗（status done）」の commit — この進捗の status を done にし、3 回目の run の結果を書いた。
- 受け入れ条件 0 の検査 → `ok`、exit 0
- `python3 -m unittest discover -s scripts/tests -p 'test_repo_health*.py'` → 62 件 OK、exit 0
- `bash scripts/dev/check-health.sh` → exit 0。verdict pass、`fail_grade=0 new=0 worsened=0 existing=142`。`policy.changed` は pass。変更が doc_only なので source-size などは not_applicable。
- `bash scripts/dev/test-affected.sh` → exit 0。check-health は pass、「試験対象の変更なし（docs のみ等）」（`--list` の対象は 0 件）。
- `sh scripts/dev/progress-index.sh --check` → exit 0
- `cargo clippy --workspace -- -D warnings` → exit 0、`cargo fmt --all -- --check` → exit 0

## 証拠（1・2 回目の run。main への merge の前）
- 受け入れ条件 0 の検査（両 profile の `profile.validate`・`[review]` の有無・必須 7 class）→ `ok`、exit 0
- `python3 -m unittest discover -s scripts/tests -p 'test_repo_health*.py'` → 62 件 OK、exit 0
- `bash scripts/dev/check-health.sh` → exit 1。report の要点: verdict fail、`fail_grade=1 new=1 worsened=0 existing=716`。fail の rule は `policy.changed` だけで、変わった policy の file は `health/profile.py`・`profiles/celeris.toml`・`profiles/default.toml` の 3 件。ほかの必須 rule（arch-map.*・doc.links・adr.numbers・progress.index・human-text・source-size）は pass。
- `bash scripts/dev/test-affected.sh` → exit 1。最初に `check-health.sh` を呼び、同じ `policy.changed` で止まる（`--list` の対象 crate は 0 件）。
- `cargo clippy --workspace -- -D warnings` → exit 0、`cargo fmt --all -- --check` → exit 0（Python と TOML だけの変更で、Rust は変えていない）。
- 試算（2 回目の run）: main の tree にこの branch の `health/profile.py`・`profiles/celeris.toml`・`profiles/default.toml`・`scripts/tests/test_repo_health_review_policy.py` を重ねた仮の commit（`git commit-tree` で作り、ref は作っていない）を base にして `python3 scripts/dev/repo-health/repo_health.py --repo scripts/dev --base <仮の commit> --head worktree --policy-version 1` → exit 0、verdict pass、`policy.changed` は pass（fail_grade=0）。policy の commit が main に入れば、この branch の検査は `policy.changed` で落ちなくなる。
- 試算の続き: 同じ仮の commit を detach した一時の worktree（検査後に削除）で、`python3 -m unittest discover -s scripts/tests -p 'test_repo_health*.py'` → 62 件 OK、`bash scripts/dev/check-health.sh --base <仮の commit>` → exit 0（fail_grade=0）。main にこの 4 file だけを入れても main の検査は通る。
- `bash scripts/dev/check-health.sh --base HEAD`（この branch の HEAD の tree を base にする。branch を main に merge した後の main に当たる）→ exit 0（fail_grade=0）。

## 未解決
- 必須検査が `policy.changed` で落ちる件（1・2 回目の run）は、人の決定 `policy-branch-required-check` の答え（operator がこの branch を main に手で merge）で解けた。原因と経緯は成果物 profile-review.md と上の「証拠（1・2 回目の run）」にある。
  - 人の承認でこの必須検査を外す仕組みは無い（`crates/task-core/src/required_checks.rs`）。ADR D6.6 の「U2 の最初の branch は人の承認で通す」は、今回は人の手の merge で実現した。
- G2 の残りの policy の変更（`--print-review-policy`、`check-health.sh` の D6.2・D7.3）も同じ必須検査で落ちるので、同じ扱いが要る。
- D6.2 の「変更 path が全部 policy の file」の条件では、`scripts/tests/test_repo_health_review_policy.py` とこの進捗は policy の file に入らない。U2 で `check-health.sh` に D6.2 を入れるとき、試験と進捗を policy の branch に含めてよいかを決める必要がある。
- 親 task の branch はまだ main を取り込んでいない。main にはこの branch の 2 回目の先端（親 task の ADR と survey を含む）が入っているので、親が main を取り込んでも、この branch の commit は衝突しない見込み（未確認）。

## 提案
- ADR D2.2 の例の `^crates/task-store/` は実在しない。ADR の付記で、class の値はこの profile が正本だと書く。
