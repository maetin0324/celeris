---
title: ops-r1 の採点〜集計の経路（fix-pipeline）
tasks: [01M655T6CGPRQ2HSQPYQFXYGZX]
status: done
updated: 2026-10-30
---
# ops-r1 の採点〜集計の経路（fix-pipeline）

完了日: 2026-10-30

## 結論

preflight の blocking 不足 `g-ops-r1-pipeline-manifest` を解消した。daemon のコードは変えていない
（変更は crates/campaign/・crates/celeris/tests/・scripts/campaign/・docs/ops/・agent-docs/ の中だけ）。

- ops-r1 の 2 condition（`mlsys-main-4-ops-r1-k4p1-A-direct-claude-code`・`mlsys-main-4-ops-r1-k4p1-B-celeris-ops`）は
  既存 manifest の `contract.conditions`（arm → condition_id・受付経路・trial_id・B の cos_model）で表す。B の受付経路
  `cos_thread` を宣言した manifest は `run`・`preflight` など task API に投入する道具が拒否する（B を `POST /tasks` 投入として扱わない）。
- human-op の `record|correct|list` は `--condition <condition_id>`（`--manifest` 必須、`--arm` と排他）を受け、
  experiment_set `ops-r1`・arm A/B・`condition_id` で記録する。`--arm` の 4 値と旧記録の record_id は変えていない。
- `celeris-campaign ops-r1-assemble` が A の raw（`ops-r1-a-raw/1`）と B の intake・trace・attribution（`ops-r1-attribution/2`）から
  runner と同じ形の out と records.jsonl を作り、既存の stage-grader-inputs → grade-out → check → export → human-op report →
  final-accounting → `scripts/campaign/check-out-consistency.py` が `--manifest` 付きで通る。
- 本測定の後に流すコマンド列は `docs/ops/ops-r1-pipeline.md` §8 に書いた。

## 変更（件名。基準は `git merge-base HEAD main`）
- `pipe-design: ops-r1 の採点〜集計経路の形（conditions 欄・human-op --condition・ops-r1-assemble）`
- `pipe-design: 葉の名前を replan 後の pipe-raw・pipe-assemble に合わせる`
- `pipe-manifest: ops-r1 の contract.conditions と human-op の --condition`
- `phase 1: add ops-r1 A raw and attribution v2`
- `pipe-assemble: ops-r1-assemble 入口と ops-r1 manifest の雛形`
- `pipe-assemble: 試験の repo path を doc-link 検査に合わせ、検査結果を進捗に書く`
- `pipe-e2e: ops_r1_pipeline 結合試験と check-out-consistency の arm_label 引き`
- `pipe-e2e: credential_wait_notify の試験が host の release 置き場を読まないようにする`
- `pipe-close: fix-pipeline の進捗と ops-r1-pipeline の実行例` — この進捗と手順書の §8

各葉の詳細は同じ dir の `pipeline-design.md`・`pipeline-manifest.md`・`pipeline-raw.md`・`pipeline-assemble.md`・`pipeline-e2e.md`。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`、範囲は `git merge-base HEAD main`）
- `cargo test -p celeris --test ops_r1_pipeline` → exit 0、2 passed（A・B の stub 記録で全段 exit 0、試行数・分母・終了理由・合否の一致、2 回の通しの byte 一致）。
- 文書検査: `bash scripts/dev/check-doc-links.sh` → ok、`bash scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` → ok、
  `sh scripts/dev/progress-index.sh --check` → ok（いずれも exit 0）。`bash scripts/dev/check-adr-numbers.sh` → ok。
- `bash scripts/dev/test-affected.sh` → exit 0（この WU の差分は文書だけなので、走ったのは check-doc-links とその self-test）。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- 範囲: `git diff --name-only $(git merge-base HEAD main)..HEAD` の path は全部 crates/campaign/・crates/celeris/tests/・
  scripts/campaign/・docs/ops/・agent-docs/ の中（範囲外 0 件）。

## 未解決
- B の CoS の消費（chat_runs）を試行の tokens に入れていない。pipe-assemble は records.jsonl に `role=cos`・`shared_overhead=true`
  の usage 行として書き、final-accounting の `shared_unattributable.cos` に出す。設計 §4.2 の「attribution の行の和」とは違う
  （final-accounting の by_role の和の照合が CoS を含めると合わないため）。
- B の `workspace_sha256_initial` と `--b-artifact` の取り方は runbook の担当（設計 §6）で、まだ決まっていない。
- 本物の測定記録での通しは未実施（本測定の後に `docs/ops/ops-r1-pipeline.md` §8 で行う）。
- `crates/celeris/tests/` の他の試験に `[selfdeploy] releases_dir` を書かない config があるかは未調査（pipe-e2e の未解決のまま）。

## 提案
- final-accounting の by_role に ops-r1 の CoS の役割（cos_project・cos_general）を足し、ops-r1 の試行の値を CoS 込みにするかを人が決める。
  足すなら旧系列の出力 bytes を変えない分岐が要る。
