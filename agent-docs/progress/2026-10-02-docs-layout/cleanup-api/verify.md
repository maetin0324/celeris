---
title: docs 再構成 — cleanup-api 統合後の検証と処理表のまとめ（cleanup-api / verify）
tasks: [01M3YNFVFZG5AFFM0RS05X22ZT]
status: done
updated: 2026-10-02
---
# docs 再構成 — cleanup-api 統合後の検証と処理表のまとめ（cleanup-api / verify）

integrate-body が 2 度 failed になった後の統合やり直し用 leaf。merge 自体（14264582 時点）は済んでおり、統合後の
`docs/api/v1/gui-api.md`・`docs/protocol/worker-protocol.md` を読み、兄弟 leaf（api-list・protocol・api-rest・
api-s3a・api-s3b）が残した欠陥 2 件を直し、処理表をここに集約した。

## 処理した文書（兄弟 leaf 分 + 本 leaf 分）

| 処理(削除/統合/移動/修正) | 旧パス | 新パス or - | 理由 | 最後の commit |
|---|---|---|---|---|
| 統合 | `docs/api/v1/overview.md` | `docs/api/v1/gui-api.md` §3.125 | 同じ API（実行・計画・木・決定の要求・撤去した入口）を 2 つ目の文書で説明していた | `a42f9a54` |
| 修正 | `docs/api/v1/gui-api.md` §1・§2 | - | §2 を router の全 route（174 = 表 168 + browser 制御 6、146 パス）に合わせ、§1 を middleware・problem と照らした | `01fd1f91` |
| 修正 | `docs/README.md` | - | `api/` の行から `overview.md` を外し、説明は gui-api.md の 1 本と明記 | `01fd1f91` |
| 処理なし | `docs/gui/` | - | move-docs（別 unit）で `agent-docs/gui/` へ移動済み。本 task では既に無い | - |
| 修正 | `docs/protocol/worker-protocol.md` | - | `protocol.rs`・`subprocess.rs`・`adapter.rs` と食い違う欄・規則を直し、`PROTOCOL_VERSION = 4`・WorkerMessage 全 10 variant（`wait` 追加）を反映 | `eb7392bd` |
| 修正 | `docs/api/v1/gui-api.md` §4〜§10 | - | sse.rs・types.rs・schema.rs・task-ops と照合。旧 §6.2 の手書き Rust 型写し（約 600 行）を削除、§9『未決』を本文へ畳み込み | `d9524c0f` |
| 修正 | `docs/api/v1/gui-api.md` §3.1〜§3.62 | - | handler・types.rs・task-ops と照合し、撤去済み（410 `removed_by_adr_0079`）の節・経緯の記述を直した | `d9524c0f` |
| 修正 | `docs/api/v1/gui-api.md` §3.63〜末尾 | - | §3.63 以降の食い違いを直し、§2 にあって §3 に説明が無かった route を §3.126 に追加 | `d9524c0f` |
| 修正 | `docs/api/v1/gui-api.md` §3.63 見出し | - | `#### 3.63 POST /milestones/{id}/decide`（410 撤去済み）と `### 3.63 POST /tasks/{id}/retry` の番号重複を解消（旧節は §3.125.8 の撤去一覧へ） | `a056f89e` |
| 修正 | `docs/api/v1/gui-api.md` §10 見出し + §3.23 の参照 | - | api-rest leaf が意図的に残した欠番（§9 を本文へ畳んだ後、旧 §10 の番号をそのまま残していた）を解消: `## 10.` → `## 9.`、§3.23 近くの `（3.39〜3.41、§10）` 参照を `§9` に直した（本 leaf、未コミット分を含む） | 本 leaf |

## 直した点（本 leaf 固有）

- 衝突マーカー（`<<<<<<<` / `=======` / `>>>>>>>`）: 両ファイルとも 0 件。
- 見出し番号: `## ` 見出しは 1〜9 が連番・欠番なし（旧 `## 10.` の欠番 `## 9.` を解消）。`### 3.x` 見出しは重複なし（§3.63 の重複は既に a056f89e で解消済みと確認）。
- 表の列数: gui-api.md・worker-protocol.md の全 Markdown 表を走査し、ヘッダと行の列数不一致は 0 件。
- §2 の表 ⇔ router: `crates/task-api/src` 配下 `.route(…)` の文字列リテラル経路 141 本（§2 の表の 168 行のユニークパス）と diff → 差分ゼロ。定数 `BASE` + `format!` で組む browser 制御 5 route（6 メソッド）は §2 表の外（§3.126.18 の別表）に説明があり、見出しの「174 = 表 168 + browser 制御 6」「146 本」の数とも一致。
- §3 見出し ⇔ router: `### 3.x` の `METHOD /path` 27 件（グループ見出しを除く単独節）はすべて実在 route。唯一の非直接一致は §3.8 の `{stdout|stderr|result|request|prompt}` 展開表記で、5 本とも実在（意図した略記）。
- worker-protocol.md: `PROTOCOL_VERSION = 4` を明記。`WorkerMessage` の全 10 variant（Progress/Comment/Delegate/Artifact/Question/Done/Error/Yielded/BudgetExhausted/Wait）がいずれも本文に 1 件以上登場。

## 証拠コマンドと結果

| コマンド | 結果 |
|---|---|
| `grep -n '^<<<<<<<\|^=======\|^>>>>>>>' docs/api/v1/gui-api.md docs/protocol/worker-protocol.md` | 該当なし（exit 1） |
| `find crates/task-api/src -name '*.rs' ! -name '*_tests.rs' ! -name 'tests.rs' -exec cat {} + \| tr -d '\n' \| grep -oE '\.route\( *"[^"]+"'` の経路（sort -u）と §2 表のパス（sort -u） | 141 件ずつで `diff` 差分ゼロ |
| python3 一行スクリプト（`.route(` を括弧対応で読む） | 146 本の `.route()` 呼び出し、うちリテラル経路 141・`BASE`/`format!` 経路 5（メソッド合計 168 + 6 = 174） |
| `grep -n '^### ' docs/api/v1/gui-api.md \| sed … \| uniq -c` の重複検査 | 重複 0 件 |
| `grep -n '^## ' docs/api/v1/gui-api.md` | `1〜9` が連番・欠番なし |
| Python 表の列数検査（gui-api.md・worker-protocol.md 全表） | 不一致 0 件 |
| `grep -n 'PROTOCOL_VERSION' crates/task-worker/src/protocol.rs` | `pub const PROTOCOL_VERSION: u32 = 4;` |
| `WorkerMessage` の全 10 variant を snake_case で `grep -c` | いずれも 1 件以上（progress 12 / comment 6 / delegate 10 / artifact 10 / question 17 / done 26 / error 14 / yielded 9 / budget_exhausted 6 / wait 9） |
| `sh scripts/dev/check-doc-links.sh` | exit 1、43 件。全件 docs/api/v1/gui-api.md・docs/protocol/worker-protocol.md の外（README・agent-docs・docs/guides・docs/ops・gui/docs）。本 leaf 由来・兄弟 leaf 由来の壊れたリンクは 0 件 |
| `git diff --stat HEAD -- crates/ web/ gui/ scripts/ CLAUDE.md .claude/ docs-layout.tsv '*.schema.json'` | 出力なし（範囲外差分ゼロ） |
| `git diff --stat HEAD` | `docs/api/v1/gui-api.md`・`docs/protocol/worker-protocol.md`（本 leaf 分、行数小）+ この記録 |

Rust・生成 schema は変更していないため `cargo test --workspace` / `cargo clippy --workspace -- -D warnings` は実行していない（docs のみの変更。crates 差分ゼロで代える）。

## 未解決

- `gui/docs/celeris-api-v1.md`（`scripts/sync-gui-docs.sh` の写し）は削除済みの `docs/api/v1/overview.md` を参照したまま。gui/ はこの task では変更できない（api-list・api-rest・api-s3b の記録と同じ指摘）。
- リポジトリ全体の `check-doc-links.sh` は 43 件の既存の壊れたリンクで失敗する。すべて docs/guides・docs/ops・agent-docs・gui/docs にあり、本 task（docs/api・docs/protocol）の範囲外。
- browser の Live View（§3.126.14〜17）・制御（§3.126.18）は `docs/guides/browser-capability.md` 側に説明が無い（gui-api.md 側には §3.126 で記載済み）。
- §3.126 の route は定数 `BASE` + `format!` で登録されるため、文字列リテラルだけを拾う機械的な route 一致検査（今回使ったワンライナーを含む）では見えない。決定的な check にするなら括弧対応で読む方式（api-list leaf が使った `routes.py` 相当）が必要。

## 提案

- `gui/docs/celeris-api-v1.md` を消すか、冒頭を `docs/api/v1/gui-api.md` への案内だけにする（gui/ を触れる task で）。
- §2 の表 ⇔ router の一致検査を決定的な check として `scripts/dev/` に置き、`BASE`/`format!` 経路も拾えるようにする（api-list leaf の提案と同じ）。
- `docs/guides/browser-capability.md` に Live View・制御 API の節を足す（api-list leaf の提案と同じ）。
