---
tasks: [01M5KWPP2W6FZAGWXFDFBQJ5S0, 01M6VCR06J339SH4JTK3Q8XHEP]
---
# ソース健全性の必須差分ゲート

この guide は入口と参照先を書く。検査器の契約、Celeris の profile、設計と review への組み込みは ADR が正本で、ここには写さない。

## 入口
- 単一の入口は `bash scripts/dev/check-health.sh [--base <rev>] [--out <report.json>]`。report JSON（`celeris.repo-health.report/1`）を書いてその path を出し、exit 0（合格）・1（必須 rule の fail）・2（error・欠測・使い方の誤り）で終わる。使い方は [check-health.sh](../../scripts/dev/check-health.sh) の冒頭注記、rule の状態と exit code の意味は [repo health の契約](../../agent-docs/adr/2026-11-07-repo-health-contract.md) の D3。
- `bash scripts/dev/test-affected.sh` は crate を選ぶ前に `check-health.sh` を必ず実行する。文書だけの変更も同じで、`--list` は対象を表示するだけ。workspace の必須検査はこれと clippy・fmt の 3 本（`.config/celeris/workspace.toml`）。
- base の既定は `--base auto`（`git merge-base HEAD main`）。決め方は [repo health の Celeris profile](../../agent-docs/adr/2026-11-07-repo-health-celeris-profile.md) の D1。

## 検査の中身と上限
- 既存検査（source-size・architecture-map・doc-links・adr-numbers・human-text・progress-index）の command と Celeris の上限（crate の層と依存・size・例外）は [profiles/celeris.toml](../../scripts/dev/repo-health/profiles/celeris.toml) にだけ書く。`profiles/default.toml` は `extends = "celeris"` で選ぶだけ。profile の項目の意味は `scripts/dev/repo-health/health/profile.py` の docstring が正本。
- source-size の閾値と例外は `scripts/dev/source-size-report.toml`、凍結した基準は `scripts/dev/source-size-baseline.json`（基準の SHA は 2026-10-23 の着手時の main）。現在値が閾値を超えたときの許容値は `max(閾値, min(固定基準値, 比較元値))` で、比較元は check-health の base。基準に無いキーの許容値は閾値。検査は基準を書き換えず、基準の値をその SHA のソースから測り直して照合する。基準の SHA か比較元を読めなければ失敗する。
- release.sh の warning-only report（[ADR-0083](../../agent-docs/adr/0083-source-size-guardrail.md)）は全負債の棚卸しで、この差分ゲートの代わりにならない。棚卸しは `python3 scripts/dev/source-size-report.py --format json` で取る。

## policy の file を変える branch
- profile の `policy_paths` に当たる file（検査器・profile・包んだ script・source-size の基準と閾値）を変える branch は、base 側の policy で評価され、`policy.changed`（必須 rule）で落ちる。
- 通し方は [健全性を設計と review に組み込む ADR](../../agent-docs/adr/2026-11-07-health-review-integration.md) の (6) に従う。source-size の基準の SHA・分類・閾値の変更もこの扱いになる。

## 変更の規約と review の方針
- 変更の仕方の汎用規約は [change_rules/generic.md](../../crates/task-worker/src/change_rules/generic.md)。この repo の必須規則は `CLAUDE.md`。この guide はどちらの本文も写さない。
- repo 固有の review 方針（高 risk の path class・owner 地図の file・保護する不変条件）は profile の `[review]` 節に書く。dispatcher と reviewer での使い方は同じ ADR の (2)〜(5)。

## 証拠と追跡
- review では、実行ログと report を検査した SHA に対応させる。作業ツリーの結果を、まだ検査していない SHA の結果として使わない。
- 残件の追跡 task は自動ゲートから起票しない。残件のキー（rule と path）を既存の未完了 task と照合し、あれば再利用する。同じキーの新規起票は最大 1 回、再試行は 2 回まで。上限に達したら既存 task で人の判断を受け、新しい task を作って回数を数え直さない。
- 2026-10-23 の導入時に解消した件と残件は [変更の記録](../progress/2026-10-23-source-health.md) にある。
