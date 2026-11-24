---
tasks: [01M7XQ9R1EHA8GQ0TZVRM4FBVX]
wu: reclose-audit
status: done
completed: 2026-11-24
---
# 戦略層 v1 子 7 reclose-audit: 修正後の ADR・LoC・task の進捗を直す

## 結論
- task の進捗 [2026-11-23-strategy-v1-7](../2026-11-23-strategy-v1-7.md) を、修正の段（drilldown-api・dedup-api・api-doc・drilldown-e2e）の後の HEAD に合わせた。
  front matter の `status: done` は変えていない。`updated`・`completed` と全試験の再実行は reclose-verify が書く。
- 「親の目的への寄与」の observability の行: drill-down の確認済みの根拠を、backend の 4 試験（`project_overview_drilldown_` 4 件）と、その golden を偽 daemon が
  そのまま返す e2e に改めた。v3 の「偽 daemon の手書きの link を押す e2e」は根拠に数えないと書いた。
- 「試験」: 修正の段で足した試験（Rust 5 件・vitest 2 件・書き換えた e2e 1 件）と、変更前に落ちる負の試験の再現（drilldown-api の 4 failed / 4、drilldown-e2e の vitest の exit 1）を書いた。
- 「LoC」: base 固定の分類規則で測り直した（下）。
- 「未確認」: 直したもの（実験の task の `log`・`notebook`・`git` の `task_id`、api.md）を外し、残るもの（O7 の評価の run の `log`、`.ipynb` の描画、run が manifest を
  作る経路、gateway が 403 を 502 にする件、本物の daemon での画面など）を残した。
- ADR [2026-11-23-project-observability-publish](../../adr/2026-11-23-project-observability-publish.md) に「付記（reclose-audit、2026-11-24）」を足し、保証の表の
  e2e の行と未確認の drilldown-e2e の項を直した。保証の表の試験名 78 件はすべて crates に実在した。

## 実装前 brief
1. 根本原因: v3 の close の後に修正の段が backend・e2e・API 文書を変えたが、task の進捗と ADR は v3 の close の時点の記述（drill-down の根拠に手書きの link の e2e、
   `log` は未実装、LoC は v3 の値）のままだった。
2. 責務 owner: task の進捗 `agent-docs/progress/2026-11-23-strategy-v1-7.md` と ADR `agent-docs/adr/2026-11-23-project-observability-publish.md`。
3. 既存の再利用先: 各 WU の進捗の記録（変更前の再現・試験名）、close-audit の測定の command、task の受け入れ条件 1 の式。
4. 選択肢: v3 の close-verify の結果の節を消す案と、修正の前の結果と明記して残す案。後者にした（再実行は reclose-verify の仕事で、それまで消すと試験の記録が無くなる）。
5. 新しい状態・分岐・fallback・port: 無し（文書だけ）。
6. 撤去対象: 進捗の「未確認」の `log` の link の未実装、observability の行の「backend 19 件・e2e 2 件」の drill-down の根拠、v3 の LoC の値。
7. 不変条件: 進捗の数値は command の出力と一致する（下の証拠）。保証の表の試験名はすべて実在する。

## 証拠（修正後の HEAD で実行。この WU の commit は文書だけを変える）

### 受け入れ条件 0: observability の行が backend の試験と golden の e2e を根拠にしている
- `cargo nextest run -p task-core -p task-ops -p task-api -p celeris-mcp -E 'test(/publish_manifest_|publish_signoff_|proposal_review_|project_overview_|cos_ops_registry_/)' --no-tests=fail`
  → exit 0、`83 tests run: 83 passed, 3250 skipped`。`cargo nextest list` の同じ filter で `publish_manifest_` 15・`publish_signoff_` 17・`proposal_review_` 23・
  `project_overview_` 23・`cos_ops_registry_` 5。`cargo nextest list -p task-api --test project_overview` に `project_overview_drilldown_` の 4 件がある。
- `corepack pnpm@12.6.0 -C web install --frozen-lockfile --offline` → exit 0。`corepack pnpm@12.6.0 -C web exec playwright test e2e/work/project-overview.spec.ts`
  → exit 0、`2 passed`（`drill-down（本物の backend の概要の golden） › log・Notebook・作業ツリー・task 付きの Git・文献・chat の thread・task へ移る` を含む）。
- `corepack pnpm@12.6.0 -C web exec vitest run e2e/support/fake-daemon.test.ts -t project_overview_` → exit 0、`2 passed | 19 skipped`。

### 受け入れ条件 1: LoC の 4 つの欄を測り直した
```sh
b=$(git merge-base HEAD main)
git show "$b:scripts/dev/source-size-report.py"   > "$TMPDIR/loc/source-size-report.py"
git show "$b:scripts/dev/source-size-report.toml" > "$TMPDIR/loc/source-size-report.toml"
python3 "$TMPDIR/loc/source-size-report.py" --root "$PWD" --config "$TMPDIR/loc/source-size-report.toml" --exclusive-classes --rev "$b"  --format json > base.json
python3 "$TMPDIR/loc/source-size-report.py" --root "$PWD" --config "$TMPDIR/loc/source-size-report.toml" --exclusive-classes --rev HEAD --format json > head.json
```
2 回とも exit 0。base は `integrate wu/stale-names (phase rename)`（v3 の close と同じ）。

| 欄 | base | HEAD | 差分 |
|---|---:|---:|---:|
| `views.product_only.lines` | 388,192 | 393,338 | +5,146 |
| `views.product_only.rust_lines` | 342,535 | 346,168 | +3,633 |
| `matrix.production.typescript.lines` | 34,523 | 36,035 | +1,512 |
| `views.own_total_with_tests.lines` | 1,071,391 | 1,082,712 | +11,321 |

同じ script と toml を v3 の close の統合の commit（`integrate wu/close (phase close)`）に当てると 393,266・346,096・36,035・1,081,892 で、修正の段の分は
production Rust +72（drilldown-api +104、dedup-api −32）・production TS 0・自前の総量 +820。純増の理由は task の進捗の「## LoC」。

### 受け入れ条件 2: ADR の保証の表の試験名がすべて crates に実在する
```sh
f=agent-docs/adr/2026-11-23-project-observability-publish.md
sec=$(awk '/^## 保証の表/{p=1;next} /^## /{p=0} p' "$f")
names=$(printf '%s\n' "$sec" | grep -oE '`(publish_manifest_|publish_signoff_|proposal_review_|project_overview_)[a-z0-9_]+`' | tr -d '`' | sort -u)
for n in $names; do git grep -qE "fn $n\(" -- crates || echo "missing $n"; done
```
→ 78 件、`missing` 0 件（ADR を変えた後にも同じ結果）。略記 0 件。3 つの spec は表にあり file も在る。

### 回帰の再確認
- `git diff --quiet $(git merge-base HEAD main) HEAD -- crates/task-core/migrations/` → exit 0。`events.rs`・`store/`・`crates/task-dispatch`・`crates/task-worker` の差分 0 行。
- 足した `trait` の行 0、`…Manager` の型 0、高 risk の path class に当たる path 0 件。足した `unwrap()` は試験の 2 行だけ（`crates/celeris-mcp/tests/mcp_integration.rs`）。
- `docs/api/v1/api.md` に 3 つの口・`| 246 |`〜`| 248 |`・4 つの拒否の code がある（`grep -qF` で 10 項目）。

## 検査の結果（提出時の HEAD で実行）
最初の commit の後に実行した。2 つ目の commit はこの file の検査の結果の節だけを変える。文書の検査は 2 つ目の commit の後にも実行した。
- `sh scripts/dev/check-doc-links.sh` → exit 0（`check-doc-links: ok`）。
- `sh scripts/dev/check-adr-numbers.sh` → exit 0（`check-adr-numbers: ok (481 files)`）。`sh scripts/dev/progress-index.sh --check` → exit 0（`progress-index --check: ok`）。
- `cargo fmt --all -- --check` → exit 0（出力 0 行）。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `bash scripts/dev/test-affected.sh`（基点 `$CELERIS_WU_BASE`）→ exit 0。`test-affected: 試験対象の変更なし（docs のみ等）`。repo-health は `mode=full`、
  基点 `git merge-base HEAD main` で `verdict=pass exit=0 fail_grade=0`（findings 725: new 3・worsened 2・existing 720。required の項目はすべて pass）、
  `repo_health_gate: all checks passed`、自己試験 126 件 OK。
- 範囲: `sh "$CELERIS_WU_SCOPE_PATHS"` の path は ADR・task の進捗・この file の 3 つだけ。

## commit（件名）
- `strategy-v1 7 reclose-audit: 修正後の LoC・試験・未確認・寄与を task の進捗に書き、ADR に付記する`
- `strategy-v1 7 reclose-audit: 提出時の HEAD の検査の結果を WU の進捗に書く`

## 未確認・引き継ぎ
- 修正後の HEAD での 3 つの e2e（proposal-review・publish-manifest を含む）・fmt・clippy・task 全体の test-affected の再実行と、front matter の `updated`・`completed` は reclose-verify。
- task の進捗の「提出時の HEAD の全試験」と「close の受け入れ条件 0〜4」は v3 の close-verify の結果（修正の前の HEAD）と明記して残した。reclose-verify が置き換える。

## 親の目的への寄与
- observability（確認済みの根拠の訂正）: 本物の route が drill-down の link を出すことは backend の 4 試験で、画面がその link で生の資料へ移ることは golden を返す e2e で
  示されている。v3 の進捗は手書きの link の e2e を確認済みに数えていたので、それを外した。
- 改善 A〜D への直接の寄与は無い（文書だけ）。
