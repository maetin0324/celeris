---
title: stub pilot の repo 登録 422 を直し test-long-horizon-pilot.sh を通す（fix-repo-422）
tasks: [01M6MH1H0RTRBMZF1C113WBQRZ]
status: done
completed: 2026-11-05
updated: 2026-11-05
---
# stub pilot の repo 登録 422 を直す

完了日: 2026-11-05。`bash scripts/dev/test-long-horizon-pilot.sh` は exit 0（C08 の new・old.passive、C06 の effect stub とも通る）。

## 結論
422 の原因は 1 つだったが、直す途中で同じ経路に 2 つ目と 3 つ目の欠陥があり、3 つとも直した。

1. 422（`scripts/dev/long-horizon-pilot/driver.py` の `Trial.run()`）。`POST /api/v1/projects` に workspace を渡すと主 repo が自動登録され、名前は作業場所の basename（`trial-repo`）になる。続けて同名の `POST /projects/{id}/repos` を呼ぶと `plan_create` / `repo_create`（`crates/task-api/src/repos.rs`）が「repo name "trial-repo" is already used in this project」で 422 にする。応答 body は driver の `api()` が捨てていたので、HTTP エラー時に body を stderr へ出すようにした。直し方: 自動登録された repo を `GET` で 1 件確認し、`PATCH /api/v1/repos/{id}` で `default_branch` と `kind` を合わせる。
2. 部署が決まらず delivery が見送られる（`crates/task-ops/src/delivery.rs` の `resolve_department`）。空の DB には組織が無いので `delivery_skipped(department_unresolved)` になり、成果が main に載らず `outcome_reached` が常に fail だった。直し方: driver が部署 `pilot-delivery` を API で作り、config の `[selfdeploy] delivery_default_departments` に案件 ID を書く。
3. 取り込みの事実が 300 秒遅れる（`crates/celeris/src/delivery.rs` の `MergeQueued` 分岐）。delivery が main へ merge した後、`task_integrations`（`IntegrationMerged` event）を書くのは 300 秒周期の stranded sweep だけで、project controller は次の root を最大 300 秒作らなかった（C08 の o2 が 302 秒）。直し方: merge 成功の箇所で `task_integration_upsert(Integrated, actor=daemon)` も書く。ADR 付記 D5・D6（`agent-docs/adr/2026-11-05-a9-llm-pilot-findings.md`）を先に書いた。

## 証拠（提出時の HEAD で実行）
- `bash scripts/dev/test-long-horizon-pilot.sh`: exit 0。「stub daemon raw ok」「C06 effect stub raw ok」。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。
- `bash scripts/dev/test-affected.sh`: exit 0（977 件 pass）。
- 実 LLM（`--harness config`）は起動していない。本番の DB・daemon・config には触れていない。

## 未解決・提案
- 3 の修正に対する Rust の単体試験は足していない。pilot の stub 試験（C08 が 4 root とも tree に載る）で固定している。
- 実 LLM の再 pilot は operator が回す（前の進捗の手順）。
