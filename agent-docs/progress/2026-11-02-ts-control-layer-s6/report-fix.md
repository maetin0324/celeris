---
title: S6 report-fix — 集計 script の gate 件数の不具合と、S6 報告・境界 ADR の食い違いの訂正
tasks: [01M6FPPJN7A8H427MXFVYFY7H4]
status: done
completed: 2026-11-03
updated: 2026-11-03
---
# S6 report-fix — final review の差し戻しの訂正

unit report-fix。集計 script の重複除去を直し、[S6 の報告](../2026-11-02-ts-control-layer-s6.md) と[境界 ADR](../../adr/2026-11-02-ts-control-layer-boundary.md) を生データと実装に合わせた。測定はやり直していない。生データの jsonl と `crates/` は変えていない。

## 訂正した箇所
1. `scripts/dev/edit-activate-summary.py`: 重複除去の key に `release.at` を足し、docstring を直した。1 回の測定では同じ step・rep 0・同じ at の estimate が release の数だけ並ぶので、旧い key では 1 release に潰れていた。
2. `scripts/dev/tests/edit-activate-summary.sh`（新規）: 回帰試験。引数なしで動き、一時 dir は `mktemp -d`。第 1 引数で試す script を差し替えられる。
3. 報告の「full regression の総コスト」: ok=true の gate 4 件の表、501.430〜804.889 秒の幅、最新の 501.430 秒、代表値の選び方と理由を書いた。代表値は最近傍順位の p50 で、値は 648.145 秒のまま。`648.145` を引く記述は、報告のこの節の他に `agent-docs/progress/2026-11-02-ts-control-layer-s6/` の `.md` にも境界 ADR にも無かった（grep で確認）。
4. 報告の結論と表の注: `policy` は `-p task-core`、`cos-digest` は `-p task-core -p task-dispatch` と直し、p50 の差 110.0 秒と約 46 倍の範囲を書いた。
5. 報告の「提案」: `*-warm.jsonl` の項を消した。残る提案は無い。
6. 報告の test-build の「未計測」: 理由は未確認と書いた。`overhead.md` は未測定とだけ書き、理由を書いていない。
7. 報告の既知の制約に 11、未解決に 5 を足した（apply と記録の間で process が落ちる場合）。
8. 境界 ADR の付記 S6-1: 表を raw-names の後の名前で直接示し、古い `-warm` の連結 file 名を消した。S5-10 の「対象 crate `task-core`」と実測の違いを書いた。
9. 境界 ADR の付記 S5-13 の (12): `digest.rs` が `apply_digest_pass` の後に `append_pass_record` を別に呼ぶ実装に文を合わせ、その間で落ちると記録も計数も残らないことを書いた。

## 差し戻しの文面と違えた点
差し戻しは `cos-digest` の「6 段すべてが 2 crate」と書くが、`cos.jsonl` の `cmd` では 2 crate なのは `check`・`clippy`・`test-build`・`test-run` の 4 段で、`build`・`release-build` は `policy` と同じ `-p celeris` である。報告と S6-1 は `cmd` のとおりに書いた。

## 受け入れ条件と検査（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- 条件 0（gate 4 件、生データ全部で exit 0）: `python3 -I scripts/dev/edit-activate-summary.py agent-docs/progress/2026-11-02-ts-control-layer-s6/*.jsonl` → exit 0、problems 0。`release_gate` は 4 件で、合計 648.145・804.889・740.921・501.43 秒、cargo-test 575.619・693.076・576.226・460.793 秒。5 系列とも 20 試行。
- 条件 1（回帰試験）: `bash scripts/dev/tests/edit-activate-summary.sh` → exit 0（5 件 ok。a・b と連結 file の同時指定、生データの gate 4 件を含む）。key から `rel.get("at")` を外した写しを第 1 引数で渡す → exit 1（`release_gate R1=3.0（期待 R1=3.0,R2=30.0）`）。
- 条件 2（gate の表と代表値）: 報告の「full regression の総コスト」の節。
- 条件 3（2 crate の明記）: 報告の表の注と付記 S6-1。`grep -n "policy-warm.jsonl\|cos-digest-warm.jsonl\|policy-ts-warm\|cos-ts-warm"` を ADR と報告にかける → 0 件（exit 1）。
- 条件 4（(12) と実装の一致）: `crates/task-dispatch/src/dispatcher/cos_chat/digest.rs` を読んで確かめた。mode `ts` の経路は `apply_digest_pass` の後に `append_pass_record` を呼び、追記の失敗は warn だけである。
- 条件 5（生データと crates/ に差分なし）: `git diff --quiet $CELERIS_WU_BASE -- crates/` は親の変更を拾うので使わず、この unit の開始点との比較 `git diff --cached --quiet HEAD -- '*.jsonl' crates` → exit 0。`sh "$CELERIS_WU_SCOPE_PATHS"` は ADR・報告・この file・script・試験の 5 path だけを出す。
- 文書の検査: `sh scripts/dev/check-doc-links.sh` → ok、`sh scripts/dev/check-adr-numbers.sh` → ok（357 files）、`sh scripts/dev/progress-index.sh --check` → ok、`python3 scripts/dev/check-architecture-map.py` → OK（880 件）。
- `bash scripts/dev/test-affected.sh` → exit 0、`cargo clippy --workspace -- -D warnings` → exit 0、`cargo fmt --all -- --check` → exit 0。

## 未解決事項
1. `apply_digest_pass` と `append_pass_record` の間で process が落ちた場合の扱いは決めていない。`crates/` を変えない unit なので、事実だけを書いた。
2. test-build の増分を計らなかった理由は未確認。
3. 4 件の gate の秒の差の原因は未確認。

## 提案
- `agent-docs/progress/2026-11-02-ts-control-layer-s5/guard.md` の未解決 2 は「apply を記録と同じ transaction に置く設計に頼っている」と書き、(12) の訂正前と同じ記述である。この unit の objective の範囲外なので変えていない。S5 の記録を直すなら別の unit で行う。
- `raw-names.md` の「ADR の日付付きの旧い表を除く」は raw-names の時点の記録で、その表は今は無い。記録なので変えていない。
