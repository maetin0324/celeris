---
tasks: [01M7XQ9R1EHA8GQ0TZVRM4FBVX]
wu: drilldown-api
status: done
completed: 2026-11-24
---
# 戦略層 v1 子 7 drilldown-api: 概要が実験の task の log・Notebook・task 付きの Git の link を出す

## 結論
- final review（v3）の不合格の条件 0（本物の backend が実験の `log`・`notebook`・task 付きの `git` を出さない）を直した。
  `task_ops::project_overview` に `experiment_drills` を足し、X(v) の実験の task ごとに次を読む。
  - 最後の run（`SqliteStore::runs_for_task` の `started_at` の順の最後）→ `Link::Log{task_id, run_id}`。run が無い task には出さない。
  - `ArtifactProduced` のうち name が `.ipynb` で終わる成果物 → 既存の `artifact_link` が作る `Link::Notebook`（同じ name は 1 件）。
  - 最後の `IntegrationMerged`（delivery と sweep が取り込みを書く event）→ `Link::Git{repo, sha, task_id: Some(task)}`。repo は task の `repos` の名前。
- link を付ける項目: O5（task・notebook・log）、O9（task・log・git）、O10（task・log）。戦略の文書の `commit` の参照（O6・O7）は、repo と sha が
  実験の task の取り込みと一致すれば `task_id` を持つ。
- 本物の route の応答を fixture `crates/task-api/tests/fixtures/project_overview_drilldown.json` に固定した。`log`・`notebook`・`task_id` 付きの `git` を含む。
- 新しい表・migration・event の種類・Manager・trait・状態機械は足していない。Project.request と Roadmap は書き換えていない。本番への昇格・publish はしていない。

## 実装前 brief
1. 根本原因: `project_overview.rs` は実験の task の run・成果物・取り込みの event を読んでいなかった。O5・O9・O10 は task の link だけを作り、
   `Scope::link` は `commit` の参照をいつも `task_id: None` の `git` にしていた。症状は Web の概要の panel（log へ移れない、git が文字だけ）に出ていた。
2. 責務 owner: 概要の導出 `crates/task-ops/src/project_overview.rs`（architecture-map の概要の行）。Web・route は変えない（wire の `ProjectOverviewLink` は既に 7 種を持つ）。
3. 既存の再利用先: `SqliteStore::runs_for_task`（run の索引）、`Events`（task ごとの event の cache）、`artifact_link`（`.ipynb` を `notebook` にする）、
   `IntegrationMerged`（A9 D1 が取り込み済みと数える event）、`Task.repos`（repo の名前）。
4. 選択肢: (a) 案件の全 task の event から取り込みを集める → D1.5 が禁じる全走査になるので不採用。(b) `task_integrations` の表から引く → 目的が
   event を指定しており、O9 と `commit` の照合で読み口が 2 つになるので不採用。(c) 概要が既に event を読む X(v) の task の event だけから導く → 採用。
5. 新しい状態・分岐・fallback・port: 状態は無し（読むたびに導く）。分岐は `Scope::experiment_links` の 2 つの bool（O5 は notebook、O9 は git を足す）。
   repo の名前が task の `repos` に無いときは repo の id の文字を使う（`RepoRef` は行が消えても名前を持つので、通常は起きない）。
6. 撤去対象: `commit` の参照の `task_id: None` 固定、O5・O9・O10 の task だけの link、ADR の「backend は log を出さない」「task_id を付けない」の記述。
7. 不変条件: 概要は読むだけ（`project_overview_is_read_only`）、別の案件の資料を出さない（`project_overview_other_project_data_excluded`）、
   link は 7 種と https だけ（`project_overview_url_link_is_https_only`）。新しい 4 試験が drill-down の形を守る。

## 変更前の再現（受け入れ条件 2）
- 手順: 新しい 4 試験を `crates/task-api/tests/project_overview.rs` に先に足し、`crates/task-ops/src/project_overview.rs` を変える前に次を流した。
  `cargo nextest run -p task-api --test project_overview -E 'test(/project_overview_drilldown_/)' --no-fail-fast`
- 結果: exit 100、4 tests run: 0 passed, 4 failed。
  - `project_overview_drilldown_log_link_points_to_latest_run_of_experiment_task`: O9 の entry の `log` の link が `[]`（期待は最後の run 1 件）。
  - `project_overview_drilldown_notebook_link_from_experiment_ipynb_artifact`: O5 の entry の `notebook` の link が `[]`（期待は `analysis/scc.ipynb`）。
  - `project_overview_drilldown_git_link_carries_task_id_from_integration`: O9 の entry の `git` の link が `[]`（期待は `task_id` 付きの git 1 件）。
  - `project_overview_drilldown_fixture_matches_generated`: fixture の file が無く、生成した JSON にも `log`・`notebook`・`git` が無い。
- 変更後: 同じ command で 4 passed。`project_overview` の 23 件すべてが passed（下の「検査」）。

## 試験（`crates/task-api/tests/project_overview.rs`、本物の HTTP の route `GET /api/v1/projects/{id}/overview`）
- `project_overview_drilldown_log_link_points_to_latest_run_of_experiment_task`: 新しい run を先に索引に入れても `started_at` の順の最後を選ぶ。
  O5・O9・O10 に同じ `log` が付き、古い run の id は応答に出ない。run の無い実験の task には `log` が無い。
- `project_overview_drilldown_notebook_link_from_experiment_ipynb_artifact`: O5 に `.ipynb` の `notebook` だけが付き、`report.md` は付かない。
  X(v) でない task の `.ipynb` は応答に出ない。実験の task の無い仮説には `notebook` が無い。
- `project_overview_drilldown_git_link_carries_task_id_from_integration`: `task_integration_upsert`（delivery と sweep の口）で取り込みを記録した実験の
  task の O9 に `task_id` 付きの `git` が付き、取り込みの無い task には付かない。O6 の反証の `commit` の参照は、取り込みの sha と一致した方だけ
  `task_id` を持ち、一致しない方は持たない。
- `project_overview_drilldown_fixture_matches_generated`: 承認済みの戦略・実験の task 2 件（run 2 本・`.ipynb`・取り込みを持つ 1 件と、何も無い 1 件）・
  外部 LLM の案 1 件を seed し、応答の task・run・thread・message の id を初出順に T1・R1・TH1・M1、案件を P1 に置き換え、`generated_at` と
  `approved_at` を固定して `serde_json::to_string_pretty` の形で fixture と比べる。`UPDATE_FIXTURE=1` で書き直す。3 回続けて流して 3 回とも passed
  （id と時刻の置き換えで決定的）。fixture に 26 桁の ULID と置き換えていない時刻は残っていない（grep で 0 件）。
- 既存の試験でこれらを検出できなかった理由: 既存の 19 件は戦略の文書と決定だけを seed し、実験の task の run・成果物・取り込みを置いていなかった。
  Web の e2e は偽 daemon の fixture の link を押しており、本物の backend の形を見ていなかった。

## ADR（受け入れ条件 3）
- `agent-docs/adr/2026-11-23-project-observability-publish.md` の D1.3 の drill-down の列（O5 は task・notebook・log、O7 は「評価の記録を作った run の
  `log` は未実装」に範囲を絞った、O9 は git が取り込まれた task だけ、O10 は X(v) の task ごと）、D1.4 の `notebook`・`log`・`git` の備考、D1.5 の読み取り、
  保証の表の 4 行、「未確認」の drilldown-api の項、「付記（drilldown-api、2026-11-24）」を直した。O7 の `log` は未実装のまま残した。
- 保証の表の試験名: `awk '/^## 保証の表/{p=1;next} /^## /{p=0} p'` で切り出した 4 接頭辞の名前 78 件（close-audit の 74 件と、この WU の 4 件）を
  `git grep -qE "fn <名前>\(" -- crates` で確かめ、78 件すべてが実在した（missing 0）。

## 測定（この WU の差分。基準は `$CELERIS_WU_BASE`、`git diff --numstat`）
- production: `crates/task-ops/src/project_overview.rs` +110 −6。
- test: `crates/task-api/tests/project_overview.rs` +347、fixture `crates/task-api/tests/fixtures/project_overview_drilldown.json` +506（生成物）。
- 文書: ADR +30 −10、この進捗。
- 純増の理由: 欠けていた drill-down の導出を足した。task 全体の LoC は reclose-audit が分類規則を base に固定した command で測る。

## 検査（提出時の HEAD で実行）
- `cargo nextest run -p task-api --test project_overview --no-fail-fast` → 23 tests run: 23 passed（fixture の生成の後）。
- `cargo fmt --all -- --check` → exit 0。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo clippy -p task-api --test project_overview -- -D warnings` → exit 0。
- `bash scripts/dev/test-affected.sh` → 結果は下の「test-affected」。

## test-affected
- 1 回目（この進捗の file を書く前）: exit 1。repo-health の `doc.links` が ADR の付記の link（この file）を指す先が無いと落とした。この file を書いて直した。
- 2 回目（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）: exit 0。
  - repo-health: verdict=pass、fail_grade 0（new 5・worsened 2 は `dup.contract`・`dup.public-surface` の warning で、上の「未確認・引き継ぎ」のとおり dedup-api の範囲）。
  - 対象の crate: celeris・celeris-campaign・celeris-mcp・celerisctl・llm-proxy・task-api・task-dispatch・task-ops・task-worker →
    7136 tests run: 7136 passed、32 skipped。script bundle の SKIP 0 件。
  - web（task-api・task-ops の変更で走る）: tsc -b exit 0、vitest 108 files・771 passed、node --test 110 passed・fail 0。
- `sh scripts/dev/check-doc-links.sh --root .` → exit 0。`sh scripts/dev/progress-index.sh --check` → exit 0。

## commit（件名）
- `strategy-v1 7 drilldown-api: 概要が実験の task の log・Notebook・task 付きの Git の link を出す`
- `strategy-v1 7 drilldown-api: WU の進捗を書く`

## 未確認・引き継ぎ
- `git` の `task_id` は X(v) の task の取り込みとだけ照合する。X(v) でない案件の task が取り込んだ commit を戦略の文書が引いても `task_id` は付かない
  （全 task の event を読まない。D1.5）。
- O7 の評価の記録を作った run の `log` は出さない。`log` は最後の run で、役割（worker・reviewer・planner）で選ばない。
- Web の drill-down の e2e はまだ偽 daemon の fixture の link を押している。この fixture の JSON で e2e を流すのは WU drilldown-e2e の仕事。
- X(v) の task ごとに増えた読み取り（`runs_for_task`・`events_for`）の時間を本番の大きさで測っていない。
- repo-health の `dup.contract`・`dup.public-surface` の新しい finding（`ManifestViolationView`・`SignoffRecord` など）はこの WU の差分ではなく、
  並行の WU dedup-api の範囲である。

## 親の目的への寄与
- observability（確認済み）: 本物の route が、実験の task の最後の run の log・`.ipynb` の Notebook・取り込みの commit（task へ移れる git）を概要に出す。
  4 試験と fixture で確かめた。
- observability（未確認）: 本番の daemon・本物のデータでの画面の確認と応答時間は未確認。Web が fixture の形で移れることの e2e は drilldown-e2e が確かめる。
- 改善 A〜D への直接の寄与は無い（表示だけ）。
