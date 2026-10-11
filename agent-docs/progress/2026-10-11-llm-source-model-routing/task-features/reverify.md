# task-features / reverify: 全体検証のやり直しと記録の更新

- task: 01M4M8D4AQHBC2KG3H1BPJ7Z0J / unit: task-features / WU key: reverify
- 基点: `CELERIS_WU_BASE` = 6534045b（作業ツリーの HEAD は 00ba6d56 の後の commit 群）
- 目的: final review（01M4NZVDGH3EQ2RV476X3PJENR）の差し戻し 6 点を直した後の全体検証のやり直し。進捗と ADR 付記の `task-brief/2`・aux 抽出元の記述を実装と突き合わせて直す。
- コードは変えていない（`git diff --quiet "$CELERIS_WU_BASE" -- crates/` が clean）。変えたのは `agent-docs/progress/2026-10-11-llm-source-model-routing/task-features.md`・本ファイル・`agent-docs/adr/2026-10-11-task-routing-features.md` の付記。

## 変更の内容

- `task-features.md`: 版（`task-brief/2`・hash `sha256:3574d17f…`）、`task-brief/1` から上げた理由（差し戻し 6 点と直し方）、brief の過去の試行・性質の節の形、aux の抽出元（実装どおり）、`features_for_subject` の 4 引数、試験の証拠（この run の結果）。
- ADR 付記: `features_for_subject` の 4 引数、`task-brief/2` への改版、D3(a) の表と実装の抽出元の読み替え（dispatch の context を優先、`checks` は command のみ、`retries_n`・`repairs_n` の出所）。

## 検証の結果（この run）

| 検査 | 結果 |
|---|---|
| test-parallel | exit 0。nextest 5142 passed / 0 failed / 14 ignored。doctest exit 0 |
| `route_record_schema_pinned` | 1 passed |
| `cargo fmt --all -- --check` | exit 0 |
| `cargo clippy --workspace -- -D warnings` | exit 0 |
| `check-doc-links.sh` | ok |
| `check-adr-numbers.sh` | ok（183 files） |
| `check-architecture-map.py` | exit 1（base 由来。詳細は task-features.md の試験の証拠） |

## 未解決

- `check-architecture-map.py` の失敗: `docs/architecture-map.md` の 66・67 行（試験の一時 dir の行）が、チェッカーに「存在しないパス」と判定される。base に既にあり、本 WU の変更ではない。表は refs-repo の WU の担当なので本 WU では直さない。提案は task-features.md の試験の証拠の表を参照。
