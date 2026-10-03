---
title: union の範囲を追記専用の記録に限り、誤って追跡した artifacts/review.json を外す
tasks: [01M3ZCXNXTRTA9GNJ52Q36ZFCP]
status: done
updated: 2026-10-03
---

# union の範囲を追記専用の記録に限り、誤って追跡した artifacts/review.json を外す

final review の差し戻し 2 点を直した。

## (1) union の範囲

`.gitattributes` の `merge=union` を `agent-docs/PROGRESS.md` と `docs/PROGRESS.md` の 2 行だけに限った（`agent-docs/progress/*.md`・`agent-docs/progress/**/*.md`・`docs/progress/**/*.md` からは外した）。task ごとの進捗ファイルは front matter（`status:`・`updated:`）を書き換えるため、union（行単位で無条件に結合）を使うと、両側が同じ front matter 行を別の値に変えても `git merge` が exit 0 で終わり、同じ key を 2 つ持つ front matter を黙って作ってしまう（records resolver にもそもそも衝突が見えない）。union を外した結果、これらのファイルは git の通常の 3-way merge に委ねられ、front matter の書き換えは実衝突（`U` 状態）になり、`crates/task-dispatch/src/auto_resolve/records.rs` の resolver が「base の全行を保持した末尾追記だけ」を自動結合し、既存行の変更は `NeedsHuman` に回す（既存のまま、変更なし）。

`crates/task-dispatch/src/classify.rs` 側の Record 分類（`agent-docs/progress/` 配下・凍結済み `agent-docs/PROGRESS.md`・旧 `docs/PROGRESS.md`・旧 `docs/progress/**/*.md`）は変えていない。records resolver は変更していない（既にある `changed_existing_line_requests_human` が証明する通り、既存行の変更は元から `NotHandled` に判定していた。問題は git 自身の union 属性が resolver を呼ぶ前に衝突を消していたこと）。

ADR `agent-docs/adr/2026-10-02-parallel-integration-auto-resolve.md` の D1a を実装に合わせて書き直し、末尾に『付記（union の範囲）』を足した。

## (2) artifacts/review.json

commit `e49b57cb`（子 task `01M40VEMMFEX9F89PBJXX54NRB`、WU `inbox-request` が delegate）が repo 根に `artifacts/review.json`（Celeris の判定材料）を追跡ファイルとして入れていた。このブランチは `inbox-request`・`sync-gate` 済みの状態（`celeris/01M3ZCXNXTRTA9GNJ52Q36ZFCP` の head、`0da30166`）へ `git merge --ff-only` で追従したうえで `git rm --cached artifacts/review.json` した。`.gitignore` に `/artifacts/` を足して再発を防いだ。

## 証拠

- `sh scripts/dev/tests/progress_union_merge.sh` → exit 0。出力に `OK: .gitattributes limits merge=union to agent-docs/PROGRESS.md and docs/PROGRESS.md`、`OK: git merge kept both sections for agent-docs/PROGRESS.md`、`OK: git merge kept both sections for docs/PROGRESS.md`、`OK: front matter conflict is not silently merged`、`progress_union_merge: all checks passed`。
  - (a) 凍結 `agent-docs/PROGRESS.md`・`docs/PROGRESS.md` への両側追記は衝突なく両方の節を残す。
  - (b) `agent-docs/progress/2026-10-03-example/leaf.md` の front matter `status:` 行を両側が別の値（done / blocked）に変えると `git merge` が非 0 で終わり、衝突マーカーが残る（黙って結合しない）。
- `cargo test -p task-dispatch --lib` → `test result: ok. 546 passed; 0 failed; 0 ignored`。新規試験 `auto_resolve::tests::front_matter_status_conflict_requests_human_and_is_not_silently_merged`（実リポジトリの `.gitattributes` を一時 repo に写し、front matter 衝突が `Resolution::NeedsHuman`（`reason` に「記録の既存行が両側で変わった」）に回ることを確かめる）を含む。
- `cargo test -p task-ops` → `test result: ok. 464 passed; 0 failed; 1 ignored`（無関係の手動測定試験 1 件のみ ignored）。
- `cargo fmt --all -- --check` → exit 0。
- `cargo clippy --workspace --all-targets -- -D warnings` → exit 0（警告なし）。
- `git ls-files | grep -c 'artifacts/review.json'` → 0（追跡から外れた）。
- `grep -c 'agent-docs/progress' .gitattributes` → 0（union の対象から外れた）。

## 未解決事項

無し。final-gate（union 修正後に最新 main を取り込みゲートを通す）と integrate-close（この段の統合）は別 WU。
