# task-features reverify2: 全体検証のやり直しと記録（task subject の deps_n）

tasks: [01M4NM332S384YYC4VC5DB7KCY]

- task: 01M4M8D4AQHBC2KG3H1BPJ7Z0J / unit: task-features / WU key: reverify2
- 基点: `CELERIS_WU_BASE` = 4fa839f4（deps-subject の commit 1fd7bd85 を含む）
- 目的: 前回の final review（criterion 3）の差し戻し「task subject の `deps_n` が常に欠測」の修正（deps-subject）が、進捗と ADR 付記の記述も含めて実装どおりか確かめる。
- コードは変えていない（`git diff --quiet "$CELERIS_WU_BASE" -- crates/` が clean）。変えたのは `agent-docs/progress/2026-10-11-llm-source-model-routing/task-features.md`（試験の証拠に reverify2 の節）、本ファイル、`agent-docs/adr/2026-10-11-task-routing-features.md` の付記（`deps_n` の抽出元を subject 種別ごとに 1 文で足した）。

## 差し戻しの内容と直し方

- 差し戻し: `route_extract.rs` が常に `work_units.depends_on_json`（LEFT JOIN した WU 列）しか数えず、task subject では `deps_n` が NULL＝欠測になっていた。
- 直し（`crates/task-core/src/route_extract.rs`、1fd7bd85）: subject が `work_unit` なら `depends_on_json` の配列長、`task` なら `tasks.json.depends_on` の配列長。JSON が欠落・不正なら null と `missing`、空配列は 0。
- 試験: `route_extract_task_uses_task_json_dependencies_and_populates_task_fields`、`route_extract_task_empty_dependencies_are_zero`、`route_extract_work_unit_dependencies_still_use_work_unit_column`。

## deps_n の突き合わせ（ADR D3(a) の表 × 実装）

| subject | 抽出元（ADR） | 実装 | 試験 |
|---|---|---|---|
| work_unit | `work_units.depends_on_json` | 配列長（不正・欠落は null＋missing） | `route_extract_work_unit_dependencies_still_use_work_unit_column`（3 → 3） |
| task | `tasks.json.depends_on` | 配列長（不正・欠落は null＋missing、空は 0） | `route_extract_task_uses_task_json_dependencies_and_populates_task_fields`（2 → 2）、`route_extract_task_empty_dependencies_are_zero`（0） |

ADR の表・進捗の特徴量表（`task-features.md` の aux 表と D3(a) 照合）・ADR 付記の 3 か所が、この抽出元の書き方で一致した。

## 検証（この run、`CELERIS_WU_BASE` 4fa839f4 の作業ツリー）

| 検査 | 結果 |
|---|---|
| `TMPDIR=/tmp bash scripts/dev/test-parallel.sh` | exit 0。nextest 5145 passed / 0 failed / 14 ignored、doctest exit 0 |
| `cargo fmt --all -- --check` | exit 0 |
| `cargo clippy --workspace -- -D warnings` | exit 0（警告 0） |
| `check-doc-links.sh` | ok |
| `check-adr-numbers.sh` | ok（183 files） |
| `check-architecture-map.py` | exit 1（3 件。reverify と同じ既存の不一致、本 WU の差分外） |
| `git diff --quiet "$CELERIS_WU_BASE" -- crates/` | clean |

- 参考: 作業 tmp（長い path）のまま `test-parallel` を回すと 108 件が `path must be shorter than SUN_LEN` 等で落ちた。`TMPDIR=/tmp` で全件通ったため、これは試験環境の path 長。コードの不備ではない。

## 未解決

- `check-architecture-map.py` の 3 件（`docs/architecture-map.md`）は refs-repo の担当の表の不一致。本 WU では直さない（前回 reverify の提案どおり、表の path を完全な path に直すか、`<workspace>/…` の行をチェッカー対象外にする）。
- 試験後の `tmp_leftovers: 4` は警告のみ（exit に影響しない）。
