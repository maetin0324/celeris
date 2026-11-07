---
tasks: [01M6TYVJSRM3SFQ3VHWQE4MZ9M]
---
# ADR 2026-11-07: architecture-map の食い違いの種類を決定的に決める

- 状態: 採用・実装済み（健全性 v2 G1 rules-archmap）。
- 関係: [ADR 2026-11-07 汎用 repo health の契約](2026-11-07-repo-health-contract.md) D5 の `arch-map.<kind>`、[ADR 2026-11-07 健全性 v2 の基準](2026-11-07-health-v2-baseline.md)（B0 の capability-map と arch_map_diffs 51 件）。

## 結論
- 判定の正本は `scripts/dev/check-architecture-map.py` の `find_diffs`。repo health の rule `scripts/dev/repo-health/rules/arch_map.py` はこれを呼ぶだけで、判定を二重に持たない。既定の検査（`python3 scripts/dev/check-architecture-map.py`。path の実在・6 列の形で exit 1）はそのまま残し、弱めない。
- 種類は機械で決まる条件だけで決める。
  - wrong_path: 表の path・相対リンクが実在しない（既定の検査と同じ判定）。
  - missing_test: 行の「関連試験」に backtick の path が無い。対照があれば、capability の試験がその capability の行の「関連試験」に無い。
  - missing_invariant: 行の「不変条件」が空。対照があれば、capability の不変条件の根拠 path（evidence_path）がその capability の行のどこにも無い。
  - missing_row: 対照の capability の owner path が、どの行の owner・entry point の列にも無い。
  - wrong_owner: capability の行はあるが、owner path の一部がどの行の owner・entry point の列にも無い。
  - stale: 行がリンクする文書の状態行（`状態:`・`status:`）が廃止・置き換えの語（既定は検査器の `STALE_STATUS`、profile の `arch_map.stale_status` で替えられる）に当たる。
- 「行が capability を指す」は、行の path が owner path と同じ file、その上の dir、glob で一致、または owner の module の子（`foo.rs` に対する `foo/bar.rs`）のとき。
- 地図と対照の所在は profile の `[arch_map]`（`path`・`reference`・`stale_status`）から読む。`path` が無ければ全 kind が not_applicable、`reference` が無ければ missing_row・wrong_owner が not_applicable。地図が head に無い・表の形が壊れている・対照が読めないときは unknown（合格にしない）。
- 対照は policy の側（base、bootstrap なら head）で読み、base と head を同じ対照で比べる。head が対照を書き換えて finding を消せない。
- finding の鍵は種類と capability id・path で、行番号を含めない。行を足して番号がずれても base にある食い違いは existing のまま。
- B0 の 51 件は人が読んで判断した意味の食い違い（コードの振る舞いと不変条件の文の違いなど）を含む。決定的な判定はその一部しか拾えず、代わりに対照との機械的な差を全部数える。件数の違いは進捗 `agent-docs/progress/2026-11-07-health-v2-g1/rules-archmap.md` に書く。

## 理由
- reviewer と planner に渡す証跡は同じ入力から同じ結果が出る必要がある（契約 D1）。意味の判断は LLM の review に残す。
- 判定を check-architecture-map.py に置けば、既存の入口（check-health.sh・test-affected.sh）と repo health が同じ判定を使う。
