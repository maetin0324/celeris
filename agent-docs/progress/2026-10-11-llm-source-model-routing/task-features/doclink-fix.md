# task-features doclink-fix: route_extract.rs の試験 fixture の文書参照を直す

tasks: [01M4NM332S384YYC4VC5DB7KCY]

- task: 01M4M8D4AQHBC2KG3H1BPJ7Z0J / unit: task-features / WU key: doclink-fix
- 基点: 作業ブランチ `celeris-wu/01M4P7CTS42KTCR8HS4JF0252R/doclink-fix`（HEAD 0482e065）

## 原因

reverify3 で `sh scripts/dev/check-doc-links.sh` が exit 1（broken reference 5 件）。
`crates/*.rs` 内の `docs/…` と `agent-docs/…` は生きた文書参照として検査されるため、
`crates/task-core/src/route_extract.rs` の write set 試験 fixture に置いた path `docs/wu.md` が
実在しない文書参照として数えられた。試験の意図は「WU 単位の書き込み集合が brief・aux に入る」ことの確認で、
`docs/wu.md` という名前そのものには意味がない（実在する文書である必要はない）。

## 変更

`crates/task-core/src/route_extract.rs` のみ。`docs/wu.md` → `notes/wu.md` を全 5 箇所で置換（`replace_all`）。

| 行（変更前） | 箇所 |
|---|---|
| 525 | `work_unit_write_sets` の INSERT の `paths_json` |
| 552 | `work_unit` 試験の期待値 `repo:docs/wu.md` |
| 566 | brief 期待文字列の `repo:docs/wu.md` |
| 592 | `task` 試験の `!contains("docs/wu.md")`（否定） |
| 665 | `EventSeq(3)` 試験の `expected.push` |

- 並び順: `repo:notes/wu.md` は `repo:crates/…` より後・旧 `docs` と同じ位置で、ソート順は変わらない（期待される並びの変更なし）。
- 試験の意味は変えない。`check-doc-links.sh` の除外表は変更していない。

## 結果

| 検査 | コマンド | 結果 |
|---|---|---|
| 文書参照の検査 | `sh scripts/dev/check-doc-links.sh` | `check-doc-links: ok`、exit 0 |
| 残り | `grep -c "docs/wu.md" crates/task-core/src/route_extract.rs` | 0 |
| task-core route_ 試験 | `cargo test -p task-core route_` | route_extract 試験を含め `36 passed; 0 failed`、exit 0 |
| clippy | `cargo clippy -p task-core --all-targets -- -D warnings` | `Finished`、警告 0、exit 0 |

## 未解決事項

- なし（この WU の範囲）。workspace 全体の nextest・clippy は reverify4 で実行する。
