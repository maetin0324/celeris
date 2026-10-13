---
title: resolve — default branch の解決を resolve_default_branch 1 関数にまとめる
updated: 2026-10-11
status: done
tasks: [01M4NM5720BYCYHXT02297GFKK]
---

# resolve

完了日: 2026-10-11

ADR `agent-docs/adr/2026-10-11-stranded-done-branches.md` 付記 F1 の実装。

## 変えたこと

- `task_ops::changes::default_branch`（何も無ければ `"main"`）を消し、`resolve_default_branch(repo, configured) -> Result<String, DefaultBranchError>` に置き換えた。順序は 設定 → `origin/HEAD` → `main` → `master`。どれも無ければ `Err("default branch cannot be resolved: no configured branch, origin/HEAD, main or master")`。
- sweep: `BranchRecord.default_branch` は `Result<String, DefaultBranchError>`（daemon の `stranded_sweep.rs` が解決した結果）。`Err` のときだけ理由つき `unintegrated`。`repository default branch is missing` の個別判定は消した。path が無い repo は従来どおり `repository cannot be resolved`（Remote の扱いは remote 葉）。
- 呼び出し元（解決できないとき）:
  - delivery: `Skip::Report { RefsUnresolvable }`、detail に理由
  - doc_gardener: `Err(String)`
  - task-api changes: 一覧は `missing: true`（`default_branch` は空文字）、diff・integrate（merge / PR）は 409 `default_branch_unresolvable`。discard は既定 branch を使わないので影響なし
  - task-api integrations（取り残しの merge）: 409 `default_branch_unresolvable`
  - task-api docs: `docs_unavailable` で理由を返す（ADR は「一覧を空にし理由を載せる」。docs 一覧の応答に repo ごとの理由欄が無いため、既存の docs_unavailable 問題に理由を載せた）
  - task-api handlers/tasks.rs: `IntegrationContext.repositories` に入れない（`"main"` で埋めない）、warn log
  - task-dispatch behind_target / review_spawn: `review_target_ref` が `Ok(None)` を返し、その repo を review sync・behind の対象外にする（ADR の「DispatchError を返す」ではなく、他の repo の sync を止めないよう skip にした。behind は null のまま）
  - housekeeping: seed の `commit: None`。run_context: docs 一覧を空。checkpoint: `(None, Vec::new())`
  - celerisctl `docs-maintenance apply`: `--default-branch` の既定 `main` を消し、省略時は解決、失敗で非 0
- 新しいエラーコード `default_branch_unresolvable`（409）を API が返す。`docs/api/v1/api.md` のエラーコード表への追記はこの葉の範囲 check（crates/・agent-docs/ のみ）の外なので入れていない（提案参照）。

## 証拠

- `cargo nextest run -p celeris -p task-ops stranded_default_branch` → 6 passed（changes 単体 3: 設定→検出順、origin/HEAD 優先、解決不能で Err / sweep 3: NULL+main の repos 付き task が integrated、NULL+origin/HEAD の repoless task が integrated、どれも無い repo が理由つき unintegrated）
- `grep -rn 'pub fn default_branch(' crates | wc -l` → 0、`grep -rn 'fn resolve_default_branch' crates` → 1 件（task-ops/src/changes.rs）
- `cargo build --workspace --all-targets` → exit 0
- `cargo clippy --workspace --all-targets -- -D warnings` → exit 0
- `TMPDIR=/tmp bash scripts/dev/test-parallel.sh` → 5166 passed, 13 skipped, exit 0（run の既定 TMPDIR は unix socket path 長を超えるため /tmp で試験 fixture だけ置いた）

## 未解決事項

- 範囲 check 不合格（attempt 1: docs/api/v1/api.md が範囲外）を受け、api.md の変更を戻した。
- docs・behind_target の失敗時の振る舞いは ADR の表と少し違う（上記）。verify 葉で ADR 付記の表を実装に合わせて直す。

## 提案

- `docs/api/v1/api.md` のエラーコード表に次の行を足す（verify 葉か後続で。範囲に docs/ を許す葉が要る）:
  `| default_branch_unresolvable | 409 | ADR 2026-10-11-stranded-done-branches F1: repository の既定の branch を決められない（project_repos.default_branch が空で、origin/HEAD・main・master のどれも無い）。変更の diff・取り込み・取り残しの merge が返す。detail は "<repo>: <理由>"。何も触っていない |`
