---
title: 健全性v2 R5 reclass — 地図の brace path を読み、地図が参照する target を r6 に分類し直す
tasks: [01M6TYVJT53598CPKMGSQGA7XM]
status: done
updated: 2026-11-10
completed: 2026-11-10
---
# 健全性v2 R5 reclass — 地図の brace path を読み、地図が参照する target を r6 に分類し直す

完了日: 2026-11-10

WorkUnit `reclass`（段 bundle）。`classify.py` が `docs/architecture-map.md` の path を地図の検査器と同じ集め方で読むように直し、`inventory.json` の leaf と束を直した。repo のコード（`crates/`・`scripts/`）と地図は変えていない。基準は `$CELERIS_WU_BASE`。

## 変更
- 件名「R5 reclass: classify.py が地図の path を check-architecture-map.py の collector で集め、地図が参照する target を r6 にする」の commit — [classify.py](classify.py)・[inventory.json](inventory.json)・[test_classify.py](test_classify.py)（新規）・[ADR](../../adr/2026-11-10-test-target-bundles.md)（束ねの表・D2・D4・付記）・[inventory.md](inventory.md)（訂正の 1 行）・この file。

## 実装前 brief
1. 根本原因: `classify.py` の `external_refs` が地図の各行を path の部分一致（`crates/…/tests/x.rs` か `` `…/tests/x.rs` ``）で探していた。地図は `crates/celerisctl/tests/{no_migrate,cli_resolution,bin_smoke}.rs` のような brace の path と、path の直後の `（…）` の中の相対 path を使う。検査器はこれを展開して実在を確かめるが、`classify.py` は見落とした。その結果、地図が参照する 19 target が bundle・adjust の leaf に入り、束ねた leaf の health gate が `arch-map.wrong_path` で止まった。`--check` も同じ探し方の再走査と比べていたので気づかなかった。
2. 責務 owner: 地図の行が指す path の決め方は `scripts/dev/check-architecture-map.py`（`collect_backtick_paths`・`collect_md_link_paths`・`resolve_link`・`TABLE_ROW_RE`）が正本。leaf の決め方は `classify.py` の `assign` と `--check`。
3. 既存の再利用先: 検査器を importlib で読み（`spec_from_file_location`、`exec_module` の前に `sys.modules` に登録。`@dataclass` が要る）、その関数で表の行の path を集める。
4. 選択肢: (a) brace と括弧の展開を `classify.py` に写す — 同じ仕様の二重実装になるので採らない。(b) 検査器を呼ぶ — 採った。(c) 地図か検査器を直す — operator の指示（地図の検査を弱めない）と write set の外なので採らない。
5. 新しい状態・分岐・fallback・port: 無し。`keep_reasons`（規則 1 の理由を返す。`assign` から切り出した）と `map_rule_errors`（`--check` の規則 2）を足した。
6. 撤去対象: 部分一致の探索（`external_refs` の中の旧い loop）を消した。
7. 不変条件: 地図の表の行が挙げる target は、規則 1 の理由が無ければ r6（動かさない）。`--check` が地図を集め直して確かめ、`test_classify.py` が brace・括弧・相対リンクの形で確かめる。

## inventory.json の差（base_sha 2166a26556364c94851dc30f5f6a6cf5ad0b98bc のまま）
- 地図が参照する target は 63 件から 124 件になった。旧い探索が見つけた 63 件はすべて新しい集め方でも見つかる（減った参照は 0 件）。表の外の説明文だけにある test target は 0 件で、表の行だけを数えても旧い結果より狭くならない。
- r6 になった target は 20 件。段 bundle・adjust から 19 件: task-api 15（auth_and_guards・browser_doctor・browser_live・browser_site_policy_db・chat_stream・cluster_connect_admin・cos_chat_attach_pin・cos_operations・lifecycle・organization・reports・request_lock_counts・task_management・task_tree・tree_adopt）、celeris 1（unpromoted_release）、celerisctl 3（bin_smoke・cli_resolution・no_migrate）。もう 1 件は task-api の `execution` で、1 件だけの組（`singleton_bundle_group`）として keep だった。規則 2 は 1 件だけの組の判定より先なので r6 にした（どちらも動かさない leaf なので、束ねの作業は変わらない）。
- 新しく参照が見つかった 61 件のうち残りの 41 件は keep の理由（数字・process global・名前で呼ばれる suite・束ねる leaf の無い crate など）を先に持ち、leaf は keep のまま。`external_refs.architecture_map` の欄だけが増えた。ほかに 7 件（runner_checker・broker・notify・manifest・chat_api・cos_triage・decisions）は参照の行が増えた。
- 束は同じ規則（D1・D2、`make_bundles`）で組み直した。件数が減った鍵の束は名前と中身が変わった。
  - task-api bundle-api-a の cap-cos-delivery は 18 件から 15 件になり、3 束（`_a`・`_b`・`_c`）から 2 束（`cos_delivery_suite_a` 8 件・`_b` 7 件）になった。
  - task-api bundle-api-b の cap-browser-eval-surface は 10 件から 6 件になり、`browser_eval_surface_suite_a`・`_b` が `browser_eval_surface_suite` 1 つになった（adjust-api の同名の束は browser_site_policy_db が r6 になって消えたので、名前が空いた）。
  - task-api bundle-api-b の cap-cos-delivery（8 件）は件数が同じで、名前だけ `cos_delivery_suite_d` から `cos_delivery_suite_c` になった（bundle-api-a の束が 1 つ減ったため）。
  - task-api の `unmapped`（18 件から 15 件）は `unmapped_suite_a`〜`_c` が `_a` 8 件・`_b` 7 件になった。`task_plan_suite` の 4 件はすべて r6 になり、束が消えた。
  - celeris `cos_delivery_suite` は 8 件から 7 件、celerisctl `browser_eval_surface_suite` は 7 件から 4 件になった。
- 1 件だけになった束は無かった（ADR の規則で keep にする target は増えていない）。
- 数: 段 bundle は 95 target・19 束から 77 target・15 束になった（task-api 56 → 9、celeris 15 → 4、celerisctl 6 → 2）。段 bundle の後の test target は task-api 113 → 66（lib の unittest を足した binary は 114 → 67）、celeris 55 → 44、celerisctl 18 → 14。段 final の adjust-api（4 件 → 1 束）を足すと task-api は 63。
- summary: task-api は adjust-api 4・bundle-api-a 18・bundle-api-b 38・keep 17・r6 36、celeris は bundle-celeris 15・keep 28・r6 12、celerisctl は bundle-ctl 6・keep 5・r6 7。ほかの crate は変わらない。
- `rules.leaf_order` は `classify.py` の docstring から作るので、規則 2 の説明の変更が入った。
- 作り方: 既存の `inventory.json` の各 record の `external_refs` を base_sha の再走査（新しい `base_fields`）で置き換え、`conflicts` は既存のまま `assign` と `make_bundles` をやり直し、`summary` と `rules.leaf_order` を作り直した（一時の Python。repo には置かない）。`branches`・`main_at_generation`・`conflicts` は生成時のまま。

## `--write` との比較（R2〜R4 の branch の変化。inventory.json に入れなかった差）
`python3 classify.py --write "$TMPDIR/…/fresh.json" --base 2166a26556364c94851dc30f5f6a6cf5ad0b98bc` を一時 file に出し、上の inventory.json と比べた。main は生成時と同じ（`main_at_generation` のまま）。352 target の leaf と bundle はすべて一致し、違いは次の 3 つだけだった。
- `branches`: R2 の branch `celeris/01M6TYVJT0Z1CBGNK8VTNK8ZJH`（変更 file 5 → 21）と WU branch `dups`（head が動いた、11 file のまま）、新しい WU branch `celeris-wu/01M6TYVJT0Z1CBGNK8VTNK8ZJH/adapter-settings`（21 file）。R3 の branch `celeris/01M6TYVJT2HG9NY5FW9FMCZS9A`（71 → 72 file）と新しい WU branch `celeris-wu/01M6TYVJT2HG9NY5FW9FMCZS9A/record`（72 file）。
- `conflicts`（13 target）: R3 の `branch_changes` が 2 → 3 件になったもの 9 件（task-api の browser_site_policy_db・cos_operations_domains・cos_ops_admin_config・cos_ops_projects_cron・cos_triage・project_auto_integrate・project_cos_guard・project_plan、celeris-mcp の mcp_integration。どれも既に競合ありで、leaf は変わらない）。R2 の `branch_changes` 0 → 2 件と `planned_by_adr` の増加が task-api の browser_e2e・browser_h3_injection（どちらも keep のまま）。R3 の ADR が新しく名指したもの 2 件（celeris の trial_context_canary_e2e、celerisctl の org_migrate_v2）。
- `leaf_reasons`（2 target）: trial_context_canary_e2e は `conflict_without_adjust_leaf:R2` が `R2,R3` に、org_migrate_v2 は `conflict_without_adjust_leaf:R3` が足された。どちらも数字を含む名前で keep のままで、leaf は変わらない。
- これらは段 final の sync-main が R2〜R4 の main 入りの後に `classify.py --write` をやり直すときに入る（ADR の「段 final」の節）。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `python3 agent-docs/progress/2026-11-10-health-v2-r5/classify.py --check agent-docs/progress/2026-11-10-health-v2-r5/inventory.json` → exit 0（352 targets、20 bundles、0 error）。直す前の inventory.json に当てると 88 error（規則 2 の 20 件と、再走査と違う `external_refs` の 68 件）で exit 1。
- `python3 agent-docs/progress/2026-11-10-health-v2-r5/test_classify.py` → exit 0（12 tests OK）。`archmap_paths` を旧い探し方（brace を含まない backtick の path だけ）に差し替えて同じ試験を流すと 17 failures（subtest を含む）で落ちる（一時の差し替え。作業ツリーは変えていない）。
- 受け入れ条件 3: ADR の段 bundle の表 15 行・段 final の adjust の行・keep と r6 の表を `inventory.json` の `crate`・`leaf`・`bundle`・`capability_key`・`fixture_key`・target の集合・summary と比べる一時の Python で、不一致 0 件。
- `python3 scripts/dev/check-architecture-map.py` → exit 0（990 件）。`git diff --quiet "$CELERIS_WU_BASE" -- docs/architecture-map.md scripts/dev/check-architecture-map.py` → exit 0（地図と検査器は変えていない）。

## 検査（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `bash scripts/dev/test-affected.sh` → exit 0（「試験対象の変更なし（docs のみ等）」。repo-health verdict=pass、new=0・worsened=0）
- `bash scripts/dev/check-health.sh --base "$CELERIS_WU_BASE"` → exit 0（verdict=pass、new=0・worsened=0、doc-only）
- `cargo clippy --workspace -- -D warnings` → exit 0（71 秒）
- `cargo fmt --all -- --check` → exit 0
- `python3 agent-docs/progress/2026-11-10-health-v2-r5/classify.py --verify-leaf r6 …inventory.json`・`--verify-leaf keep …` → exit 0（keep と r6 の 262 target が元の path にある）
- `sh scripts/dev/check-doc-links.sh` → ok、`sh scripts/dev/check-adr-numbers.sh` → ok（415 files）、`python3 scripts/dev/check-human-text.py --root .` → exit 0（0 findings）、`sh scripts/dev/progress-index.sh --check` → ok
- `{ git diff --name-only "$CELERIS_WU_BASE"; git ls-files --others --exclude-standard; } | python3 agent-docs/progress/2026-11-07-health-v2-b0/slices-check.py --write-set R5 agent-docs/progress/2026-11-07-health-v2-b0/slices.json` → `write-set R5: ok`
- `sh "$CELERIS_WU_SCOPE_PATHS"` → ADR・classify.py・inventory.json・inventory.md・reclass.md・test_classify.py の 6 path だけ

## 未解決事項
- 段 bundle の leaf（bundle-api-a・bundle-api-b・bundle-celeris・bundle-ctl）は `inventory.json` の束の名前で作業する（`classify.py --verify-leaf <leaf>`）。この unit で束の名前が変わったので、leaf の objective に古い束の名前（`cos_delivery_suite_d`・`unmapped_suite_c`・`task_plan_suite`・`browser_eval_surface_suite_a`・`_b` など）が書かれていれば、`inventory.json` と ADR の表が正しい。
- R6 は r6 の 55 target（task-api 36・celeris 12・celerisctl 7）を地図と一緒に動かす。

## 提案
- 無し。
