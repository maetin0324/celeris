---
title: 最終 main 取り込みと全検査
tasks: [01M41M4FDCV8Z4JZQ6ABE2M2XG]
status: done
updated: 2026-10-03
---
# 最終 main 取り込みと全検査

main `8fb9b4ae1e825b5f577871d4b2fda825b16217e1` を rebase せず merge した。review 済み candidate SHA を先に照合し、合格した item にだけ自動解消を適用する。per-repo candidate 記録と自動解消 action をともに残す。内容衝突は統合依頼を受信箱に投影し、旧 repair 経路へ流さない。

migration は main の番号を維持した。`0042`〜`0046` はこのブランチの migration、`0047` は取り込んだ main の integration request index で、重複はない。`RESERVED_VERSIONS` は未収録の 38〜40 のみとし、旧 schema からの移行試験に 0047 の index を含めた。API/Event schema と gui/web の生成型は再生成した。

| 検査 | 結果 |
|---|---|
| `cargo test --workspace` | 成功（3,819 passed、0 failed、13 ignored） |
| `cargo fmt --all -- --check` | 成功 |
| `cargo clippy --workspace -- -D warnings` | 成功（警告なし） |
| gui `install --frozen-lockfile` / `typecheck` / `test` | 成功（1,287 試験） |
| web `install --frozen-lockfile` / `typecheck` / `test` | 成功（42 server 試験） |
| `check-doc-links.sh` / `check-adr-numbers.sh` / `progress-index.sh --check` | 成功 |
