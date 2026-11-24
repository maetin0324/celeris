---
tasks: [01M7XQA74FP70G3T3KSTP02S0W]
wu: artifact-name
status: done
completed: 2026-11-24
---
# 戦略層 v1 子 8 artifact-name: artifacts の中の未申告の成果物の名を artifacts からの相対 path にする

## 結論
- `crates/task-dispatch/src/undeclared_artifacts.rs` の `scan_undeclared_artifacts_in_dir` が返す `ArtifactRef.name` を、file の basename から `artifacts_dir` からの相対 path（区切り `/`）に変えた。`artifacts/strategy/s1/v1.json` の名は `strategy/s1/v1.json`（変更前は `v1.json`）。深さ 1 の file の名は前と同じ file 名。
- `path`・`kind`・`sha256`・対象・順序・上限・重複の規則（`(path, sha256)`）は変えていない。`scan_undeclared_markdown_artifacts` の名（file 名）と、申告済みの成果物の名も変えていない。
- 補完（`undeclared_artifacts/backfill.rs`）は同じ関数を使うので同じ名になる。試験で確かめた。
- 最終 review の `ArtifactExists` は、WorkUnit の run が申告せずに書いた入れ子の版を版の名で見つける。試験で確かめた。
- ADR-0067 に付記（2026-11-24、D3-e）を足した。名の規則・理由・契約の変更・既存の受け入れ条件への影響の見込みを書いた。
- 新しい状態・分岐・fallback・config の欄は足していない。production の差分は 1 file（+5 −4 行）。候補の型 `Candidate` の欄 `name` を消し、名は `rel_path` から作る。
- 評価器の prompt（`crates/task-worker/src/change_rules/`）・`control-ts`・`web` の差分は 0。本番の DB・daemon・config には触れていない。LLM の run は起こしていない。

## 実装前 brief
1. 根本原因: `scan_undeclared_artifacts_in_dir` は候補の名に `entry.file_name()`（basename）を使っていた。`artifacts_dir` の中の入れ子の file は、名の規則（`task_core::strategy_ref::artifact_name` の `strategy/<id>/v<n>.json`）と、`ArtifactExists` の既定 path `{artifacts_rel}/{name}` の両方に合わない名で記録された。症状は方式 c の版の段で出た。claude-code の worker が書いた版が `v1.json` の名で記録され、driver が `strategy/pilot-replay/v1.json` を見つけられずに止まった。stub は `artifact` 行で名を申告していたので通っていた。
2. 責務 owner: 未申告の成果物の名を決める `undeclared_artifacts::scan_undeclared_artifacts_in_dir`。run 後の走査（`dispatcher/worker_task.rs`）と補完（`backfill.rs`）はこの関数の結果をそのまま記録するだけなので、変えない。
3. 既存の再利用先: 走査の中の `rel_path`（`artifacts_dir` からの相対。すでに深さと並べ替えに使っている）。版の名の関数 `strategy_ref::artifact_name`・`strategy_artifact`。最終 review の候補の組み立て `review::artifact_candidates` と試験の helper（`root_review_artifacts.rs` の `adopt_units`・`unit_run_produced`・`candidates`・`artifact_verdict`）。補完の試験の helper（`backfill_tests` の `task`・`remote`・`produced`）。
4. 選択肢:
   - (a) claude-code・codex・acp の adapter に成果物の申告の口（`result.json` の欄など）を足す。新しい port になり、3 つの adapter と prompt の契約を変える。既に書かれた file の名の不一致は残る。不採用。
   - (b) driver か最終 review で basename や path の末尾でも照合する。症状の出た場所に特例を足すことになり、名の規則が 2 つになる。不採用。
   - (c) 走査の名を `artifacts_dir` からの相対 path にする。原因の owner を直し、名と既定 path の規則が 1 つになる。深さ 1 の名は変わらない。採用。
5. 新しい状態・分岐・fallback・port: 無し。
6. 撤去対象: 候補の型 `Candidate` の欄 `name`（basename）。名は `rel_path` から作る。
7. 不変条件と試験: 下の節。

## 守る不変条件と、既存の試験で検出できない理由
- `scan_undeclared_artifacts_in_dir` の `name` は `artifacts_dir` からの相対 path（区切り `/`）。深さ 1 は file 名。
- その名は版の名として `strategy_ref::strategy_artifact` で読める。
- 補完も run 後の走査と同じ名を記録する。
- 最終 review の `ArtifactExists` は、WorkUnit の run の入れ子の未申告の版を名で見つける。
- `scan_undeclared_markdown_artifacts` の名は file 名のまま。
- 既存の試験で検出できない理由: D3-c の試験（`in_dir_scan_*`・`git_worktree_task_*`）と補完の試験は入れ子の file の `path` と `kind` だけを見ていて、`name` を見ていなかった。`root_review_artifacts.rs` の試験は候補の `ArtifactRef` を手で作り、名を正しく与えていたので、走査が付ける名を通らなかった。

## 試験（`crates/task-dispatch`。新しい試験の名は `undeclared_nested_name_` で始まる）
- `undeclared_artifacts::tests::undeclared_nested_name_is_relative_to_the_artifacts_dir`: (1) `artifacts/strategy/s1/v1.json` の名が `strategy/s1/v1.json`、`artifacts/cmp4/final/report.md` の名が `cmp4/final/report.md`。
- `undeclared_artifacts::tests::undeclared_nested_name_reads_as_a_strategy_version`: (2) その名が `artifact_name("s1", 1)` と一致し、`strategy_artifact` で `("s1", 1)` と読める。
- `undeclared_artifacts::tests::undeclared_nested_name_keeps_depth_one_and_markdown_scan_names`: (3) 深さ 1 の名（`report.md`・`data.csv`・`manifest.json`）は file 名のまま。作業場所全体の `*.md` 走査の名（`docs/paper/notes.md` → `notes.md`）も変わらない。
- `undeclared_artifacts::tests::backfill_tests::undeclared_nested_name_backfill_registers_the_same_relative_name`: 補完が `strategy/s1/v1.json` と `report.md` の名で登録し、結果が同じ写しへの `scan_undeclared_artifacts_in_dir` と一致する。
- `dispatcher::tests::root_review_artifacts::undeclared_nested_name_root_review_finds_the_work_unit_strategy_version`: WorkUnit の run の `wu/version/artifacts/strategy/s1/v1.json` を走査で登録し、root の最終 review の `ArtifactExists{name: "strategy/s1/v1.json"}` が `path=wu/version/artifacts/strategy/s1/v1.json` で合格する。

入れ子の名を basename と仮定して確かめる既存の試験は無かった。そのため既存の試験は直していない（弱めた試験は 0）。

### 変更前に落ちた出力（試験だけを足し、`undeclared_artifacts.rs` を変える前に実行）
`cargo nextest run -p task-dispatch --lib -E 'test(/undeclared_nested_name_/)'` → 失敗（`error: test run failed`）、5 件のうち 4 件が落ち、1 件（深さ 1、変更前も通るべきもの）が通った:

```
FAIL undeclared_artifacts::tests::undeclared_nested_name_reads_as_a_strategy_version
  left: "v1.json"
 right: "strategy/s1/v1.json"
FAIL undeclared_artifacts::tests::undeclared_nested_name_is_relative_to_the_artifacts_dir
  left: [("report.md", "artifacts/cmp4/final/report.md"), ("v1.json", "artifacts/strategy/s1/v1.json")]
 right: [("cmp4/final/report.md", "artifacts/cmp4/final/report.md"), ("strategy/s1/v1.json", "artifacts/strategy/s1/v1.json")]
FAIL undeclared_artifacts::tests::backfill_tests::undeclared_nested_name_backfill_registers_the_same_relative_name
  left: [("report.md", "artifacts/report.md"), ("v1.json", "artifacts/strategy/s1/v1.json")]
 right: [("report.md", "artifacts/report.md"), ("strategy/s1/v1.json", "artifacts/strategy/s1/v1.json")]
FAIL dispatcher::tests::root_review_artifacts::undeclared_nested_name_root_review_finds_the_work_unit_strategy_version
  artifact "strategy/s1/v1.json" not found at artifacts/strategy/s1/v1.json
Summary 5 tests run: 1 passed, 4 failed
```

変更後: 同じコマンド → exit 0、5 tests run: 5 passed。`-E 'test(/undeclared/) | test(/root_review_artifact/) | test(/artifact_exists/)'` → 38 tests run: 38 passed。

## 証拠（基準は `$CELERIS_WU_BASE`。Run #2 で、実装の commit の後の HEAD で再実行した。その後の commit はこの節の書き換えだけ）
- `bash scripts/dev/test-affected.sh` → exit 0。対象 crate: celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch。nextest 4011 tests run: 4011 passed、11 skipped。repo-health verdict=pass（fail_grade=0）。script bundle SKIP 0 件。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0（Run #1 の 1 回目は新しい試験の 1 行の折り返しで exit 1。`cargo fmt --all` で直した）。
- `cargo nextest run -p task-dispatch --lib -E 'test(/undeclared_nested_name_/)'` → exit 0、5 passed。
- 変更前の再現: `undeclared_artifacts.rs` だけを `$CELERIS_WU_BASE` の版に戻して同じ試験 → exit 100、5 件のうち 4 件が落ちた（上の出力と同じ）。戻した file は `git checkout` で元に戻した。
- `sh scripts/dev/progress-index.sh --check` → ok。`sh "$CELERIS_WU_SCOPE_PATHS"` → この WU の 5 file だけ。
- `git diff "$CELERIS_WU_BASE" -- crates/task-worker/src/change_rules control-ts web | wc -l` → 0。

## LoC（`git diff --numstat "$CELERIS_WU_BASE"`）
- production: `crates/task-dispatch/src/undeclared_artifacts.rs` +5 −4（doc comment 4 行を足し、欄 `name` とその代入を消した）。
- test: `crates/task-dispatch/src/undeclared_artifacts/tests.rs` +127、`crates/task-dispatch/src/dispatcher/tests/root_review_artifacts.rs` +37。
- 文書: ADR-0067 の付記 +52、この進捗。
- 純増の理由: production は 1 行の純増（doc comment）。試験は契約の変更を固定する 5 件。

## commit
- `strategy-v1 8 artifact-name: artifacts の中の未申告の成果物の名を artifacts からの相対 path にする`

## 未確認
- 実 LLM（claude-code）の方式 c の版の段がこの変更で通るか。operator が段 recheck の後の人の確認の後に流す（この WU では LLM の run を起こさない）。
- 本番の受け入れ条件のうち、`ArtifactExists` の名が file 名で、file が `artifacts/` の入れ子にあり申告されていないものの件数。本番の DB は見ていない。repo の中の JSON の `"artifact_exists", "name"` の例 82 件は全部 `/` の無い file 名。
- 変更前に basename の名で記録された成果物は、中身が変わるまで新しい名で登録し直されない（重複の規則が `(path, sha256)` のため。events は書き換えない）。

## 提案
- 親の途中評価の記録を名の接頭辞だけで見分けている（`task_core::parent_checkpoint::evaluation_decision_id` の `parent-evaluation/`。`review/parent_evaluation.rs` の `finish` の「記録済み」の判定と `request` の `previous`）。この変更で、claude-code・codex・acp の worker が `artifacts/parent-evaluation/<id>.json` に書いた file も同じ名で記録されるようになる。申告の口を持つ harness では前から同じ名を申告できた。記録の `kind`（`parent_evaluation`・`parent_evaluation_attempt`）でも見分ける修正を別の変更として提案する（この WU の範囲の外。ADR-0067 付記 2026-11-24 の最後の項）。
