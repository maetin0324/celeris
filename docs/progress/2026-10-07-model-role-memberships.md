---
title: モデルごとの複数役割と優先度
tasks: [01M49Z9NNAAKRJB4GJHFHGXX21]
status: in_progress
updated: 2026-10-07
---

# モデルごとの複数役割と優先度

設計: [モデル割り当て ADR の付記](../../agent-docs/adr/2026-10-06-model-role-assignments.md)。本番設定・daemon・DB は操作していない。

## 実装済みの範囲

- migration 0054: `(source, tier, model_id)` と priority、明示的な空集合を保持する scope。旧データとメモを保持。
- 役割集合の GET / PUT / preview、config の互換読み取り、同じ source 内の複数候補、変更イベント。
- dispatcher / proxy / routing catalog の候補列を複数モデルへ拡張。legacy の順位、消失・disabled の除外、account の枯渇時の fallback。
- web のモデル別トグル・priority、検索・source / 状態フィルタ、役割別並べ替え、保存前の影響確認。既存 provider 追加操作を保持。

## 検証記録（phase 1）

| 検査 | 結果 |
| --- | --- |
| `cargo test -p task-api --test model_assignments` | 7 passed（集合 API・複数役割・空にした役割の config 復活防止を含む） |
| `cargo test -p task-core model_catalog --lib` | 初期 14 passed。追加した移行・再起動試験は workspace 検査で実行 |
| `cargo test -p task-dispatch role_assignments --lib` | 既存 7 passed。追加した複数候補試験は workspace 検査で実行 |
| `pnpm -C web typecheck` | exit 0 |
| `pnpm -C web test` | exit 0（Vitest 479 tests / 71 files + gateway 59 tests） |
| `pnpm -C web e2e -- admin/models.spec.ts` | 実際には functional 全体が実行され、270 passed / 2 failed / 8 skipped。2 件は新試験の Table 領域 locator 誤り |
| `WEB_E2E_SCOPE=functional pnpm -C web exec playwright test e2e/admin/models.spec.ts` | locator 修正後 8 passed。複数役割・並べ替え・空集合保持・provider 追加・360px を確認 |
| `bash scripts/dev/test-parallel.sh` | 再実行: nextest 4223 passed / 1 failed（migration 版数一覧に 54 が不足）。doc-test exit 0。修正後、該当試験を単独再実行して pass。最終実装後に全体を再実行する |
| `cargo clippy --workspace -- -D warnings` | 初回 type_complexity を修正後 exit 0 |

追加検証: `pnpm -C web lint` exit 0（既存 warnings 4 件）、`pnpm -C web e2e` 272 passed / 8 skipped、`pnpm -C web e2e:nfr` 106 passed。

schema は docs / web / gui を再生成。GUI の `pnpm gen:types` は pnpm store の読み取り専用制約で失敗したため、既存の同版 json2ts の CLI を読み取り、出力先だけ本 worktree に指定して再生成した。

## 残作業

- shadow / enforce の全候補から推定器の選択を実行モデルへ反映する経路を仕上げる。既存 dispatcher は provider 単位の heuristic、proxy estimator は起動時 catalog を保持しているため、集合の列挙だけで完了とはしない。
- config 候補と membership の同順位規則を全経路で再点検する。
- 追加した proxy の account 単位 fallback と推定器への全候補入力の決定的試験。
- 指定された全体検査を最終 tree で再実行し、この表を更新する。
