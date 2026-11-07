---
title: 健全性v2 G2 profiles — profile の [review] 節
tasks: [01M6VCXJCXEPBQCR5PNR195JCH]
status: done
updated: 2026-11-07
completed: 2026-11-07
---
# 健全性v2 G2 profiles — profile の [review] 節

完了日: 2026-11-07

unit `profiles`（G2 の工程 write「汎用規約本文と Celeris profile の [review] 節を書く」の子）。[ADR 2026-11-07 健全性を設計と review に組み込む](../../adr/2026-11-07-health-review-integration.md) の D2.1・D2.2・D6.6 に従う。基準は `git merge-base HEAD main`。policy の file だけの branch（D6.1）で、受け入れには人の承認（`{"type":"human"}`）が要る。`--print-review-policy` は作っていない（U2 の別の範囲）。

## 変更
- 件名「G2 profiles: profile の [review] 節と検査」の commit
  - `scripts/dev/repo-health/health/profile.py`: `_TOP_KEYS` に `review`、`_validate_review` を足した。下位項目は `owner_map`（空でない文字列）・`high_risk`（`{class, paths}` の配列。class は空でなく重複不可、paths は 1 件以上の正規表現）・`invariants`（名前 → 根拠の file の表）だけで、ほかは `profile_invalid` の読み込みエラー。docstring（項目の正本）にも書いた。
  - `scripts/dev/repo-health/profiles/celeris.toml`: `[review]` 節。`owner_map = "docs/architecture-map.md"`、high_risk の class は credential・sandbox・store・events・lease・budget・selfdeploy の 7 件（path は地図の行と B0 の capability-map.json の owners から決めた）、invariants は 13 件（`CLAUDE.md` の 6 件と ADR・運用文書の 7 件）。
  - `scripts/dev/repo-health/profiles/default.toml`: 下位項目の無い `[review]` だけを足した。`extends = "celeris"` のまま、merge の結果は celeris の `[review]` と同じになる。
  - `scripts/tests/test_repo_health_review_policy.py`: 8 件（形の検査、未知の下位項目の拒否、celeris の path が追跡 file に当たること、default の merge が celeris の値になること）。
- 件名「G2 profiles: 試験の仮の地図 path を doc-links に当たらない名前にする」の commit — 試験の仮の値 `docs/map.md` を `doc.links` が無い link と読んだので `OWNERS.md` に変えた。

## 証拠（提出時の HEAD で実行）
- 受け入れ条件 0 の検査（両 profile の `profile.validate`・`[review]` の有無・必須 7 class）→ `ok`、exit 0
- `python3 -m unittest discover -s scripts/tests -p 'test_repo_health*.py'` → 62 件 OK、exit 0
- `bash scripts/dev/check-health.sh` → exit 1。report の要点: verdict fail、`fail_grade=1 new=1 worsened=0 existing=716`。fail の rule は `policy.changed` だけで、変わった policy の file は `health/profile.py`・`profiles/celeris.toml`・`profiles/default.toml` の 3 件。ほかの必須 rule（arch-map.*・doc.links・adr.numbers・progress.index・human-text・source-size）は pass。
- Python と TOML だけの変更なので cargo の試験・clippy・fmt は走らせていない（`test-affected.sh` は repo_health の Python 試験を上と同じく走らせる）。

## 未解決
- `git merge-base HEAD main` から HEAD の差分には、親 task の ADR と survey（`agent-docs/` の 3 file）も入る。親がまだ main に入っていないため。
- D6.2 の「変更 path が全部 policy の file」の条件では、`scripts/tests/test_repo_health_review_policy.py` とこの進捗は policy の file に入らない。U2 で `check-health.sh` に D6.2 を入れるとき、試験と進捗を policy の branch に含めてよいかを決める必要がある。

## 提案
- ADR D2.2 の例の `^crates/task-store/` は実在しない。ADR の付記で、class の値はこの profile が正本だと書く。
