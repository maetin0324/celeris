---
title: 健全性v2 G1 rules-archmap — architecture-map の食い違いを種類別 finding にする
tasks: [01M6TYVJSRM3SFQ3VHWQE4MZ9M]
status: done
updated: 2026-11-07
completed: 2026-11-07
---
# 健全性v2 G1 rules-archmap — architecture-map の食い違いを種類別 finding にする

完了日: 2026-11-07

WorkUnit `rules-archmap`。基準は `$CELERIS_WU_BASE`。決定は [ADR 2026-11-07 architecture-map の食い違いの種類](../../adr/2026-11-07-arch-map-diff-kinds.md)。

## 結論
- `scripts/dev/check-architecture-map.py` に `find_diffs`（6 種類の判定）と `--diffs [--reference <path>]`（JSON 行で一覧、exit 0）を足した。既定の実行（実在・6 列の形で exit 1）は変えていない。
- `scripts/dev/repo-health/rules/arch_map.py` が `arch-map.missing_row`・`missing_test`・`wrong_owner`・`missing_invariant`・`stale`・`wrong_path` を登録する。判定は `find_diffs` を呼ぶだけ。
- 地図・対照の所在は profile の `[arch_map] path・reference・stale_status`。Celeris の所在は `scripts/dev/repo-health/profiles/celeris.toml` にだけ書いた。汎用（generic・`default.toml`）では `arch_map.path` が無いので全 kind が not_applicable（`no_arch_map`）。
- core への小さな追加: `health.rule.NotApplicable`（rule が not_applicable を返す口）と engine のその扱い、`profile.py` の `arch_map` の欄の検査。
- 契約の不具合を直した: report schema の fingerprint の pattern が `_` を許さず、`arch-map.missing_row:<hash>` が検証器で不正になった。pattern を `^[a-z0-9._-]+:` にした（rule ID は ADR D5 の通り `_` を含む）。

## この repo での種類別件数（提出時の HEAD で実行）
`python3 -I scripts/dev/repo-health/repo_health.py --repo . --base $CELERIS_WU_BASE --head HEAD --policy-version 1 --profile scripts/dev/repo-health/profiles/celeris.toml --out <artifacts>/archmap-head.json` → exit 0。全 rule pass、finding は全部 existing（地図は base から変えていない）。

| 種類 | この検査 | B0（slices.json の arch_map_diffs） |
|---|---|---|
| missing_row | 1 | 13 |
| missing_test | 77 | 12 |
| wrong_owner | 32 | 10 |
| missing_invariant | 32 | 9 |
| stale | 0 | 4 |
| wrong_path | 0 | 3 |
| 計 | 142 | 51 |

B0 の基準 commit（capability-map.json の base_sha）の地図は今と同じ内容（`git diff <base_sha> HEAD -- docs/architecture-map.md` が空）で、件数も同じ 142 件。同じ行・同じ種類で B0 と一致したのは 14 件（missing_test 7・wrong_owner 4・missing_invariant 3）。この検査は capability の最初の行を位置にするので、一致の数は下限の目安。

件数が違う理由:
- missing_test・wrong_owner・missing_invariant が多い: この検査は capability-map の試験・owner・根拠 path を地図と 1 件ずつ機械的に比べる。B0 は人が読んで行ごとに主要なものを 1 件にまとめた。
- missing_row が少ない: この検査は「capability の owner path がどの行の owner・entry にも無い」だけを数える。B0 の 13 件は「行の主題が無い」の判断（例: 予算・単価、run の claim と lease）で、owner path の一部は別の行に載っているので、機械的には wrong_owner 側に数える。
- stale が 0: この repo の ADR に廃止・置き換えの状態行が無い。B0 の 4 件は「不変条件の文とコードの振る舞いが違う」の意味の判断で、決定的には判定できない。
- wrong_path が 0: 既定の検査（実在）が今の地図で通る。B0 の 3 件は「glob で書いている」「owner の path が責務と違う」の判断。

## 変更（commit の件名）
- `phase G1: architecture-map の食い違いを種類別 finding にする（find_diffs と rule arch-map.*）`
- `phase G1: rules-archmap の進捗`

## 証拠（提出時の HEAD で実行。基準は $CELERIS_WU_BASE）
- `python3 -I -m unittest discover -s scripts/tests -p 'test_architecture_map*.py'` → exit 0（10 tests。6 種類それぞれの正例・負例、行番号に依らない鍵、今の地図で既定の検査と同じ種類が 0 件、既存の test_architecture_map.py）
- `python3 -I -m unittest discover -s scripts/tests -p 'test_repo_health*.py'` → exit 0（29 tests。archmap 7 件: generic で not_applicable、base の負債は existing で pass・head の新しい食い違いは new で fail、対照の missing_row・wrong_owner・missing_test、対照なしで 2 kind が not_applicable、地図の欠落・表の破損・対照の欠落は unknown で exit 2、doc-only でも評価）
- `python3 scripts/dev/check-architecture-map.py` → exit 0（OK: 931 件のパスを確認した）
- `validate_report.py <artifacts>/archmap-head.json` → 1 valid
- `bash scripts/dev/check-health.sh` → exit 0
- `sh scripts/dev/check-doc-links.sh && sh scripts/dev/check-adr-numbers.sh && sh scripts/dev/progress-index.sh --check` → exit 0
- `git diff --name-only $CELERIS_WU_BASE | python3 agent-docs/progress/2026-11-07-health-v2-b0/slices-check.py --write-set G1 agent-docs/progress/2026-11-07-health-v2-b0/slices.json` → write-set G1: ok
- `bash scripts/dev/test-affected.sh` → exit 0（crate の変更なし。scripts の unittest 60 件 OK）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0

## 未解決
- `profiles/celeris.toml` はこの WorkUnit が新しく作った（`[arch_map]` と arch-map.* の required だけ）。並行する rules-dup・rules-structure も同じ file を作ると、工程の統合で add/add の衝突になる。両方の欄を残して 1 file にまとめれば解ける。
- 対照（capability-map.json）は B0 の時点の記録。地図を直す・コードを動かすと対照も古くなる。対照の更新の手順は G2 以降で決める。
- `docs/architecture-map.md` は直していない（検出だけ）。

## 提案
- celeris-profile WorkUnit で `default.toml` を `extends = "celeris"` にするとき、`policy_paths` に `scripts/dev/check-architecture-map.py` と対照 `agent-docs/progress/2026-11-07-health-v2-b0/capability-map.json` を足す（対照の書き換えを policy.changed で review に回す）。
