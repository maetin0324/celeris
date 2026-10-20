---
task: 01M5B84ZY6H4001ZYPSGN9VR34
tasks: [01M5B84ZY6H4001ZYPSGN9VR34]
title: 投入・再開・冪等照会と改変/欠落拒否・非対象同一性・canary を実経路で検証（negative）
work_unit: negative
status: done
updated: 2026-10-20
---
# negative: 負例・非対象同一性・canary

記録の本体は同じ dir の `application.md`（record 葉で統合する）。fixture は `infra.md`、適用シナリオは `scenario.md`。

## 変更
- `crates/campaign/tests/ablation_application.rs`: `negative_paths_non_target_identity_and_canary_through_real_daemon` を追加した（約 2s）。spec-on と spec-off を、別々に所有する実 daemon・DB で順に走らせる。B0 は spec-on の `GET /org` で、off はこれを比較元にする。
  - 非対象の基準: B0 の coding 課の実効 profile から `NonTargetPin::profile_pins` で review と安全上限（`deny_tools`・`max_lane`・`max_attempts`。根 cos に置き、両課が継ぐ）を作る。それに固定値（arm B の configs・全役割のモデル・資料・KB・予算・終了条件・外部 grader）を足して `BaselinePin` にする。稼働 org から作った `VariantSpec` を seal し、`verify(&baseline)` が通ることを確かめる。稼働 daemon の coding 課の review と安全上限が B0 と一致することも assert する。
  - 投入・再開・冪等照会（既存の `CelerisExecutor` を実 daemon に向ける）: `submit`・`lookup`・`resolve`・再 `submit`、および新しい executor からの再開は、すべて同じ task 1 件に寄る（`GET /tasks` の件数は 1）。`GET /tasks/{id}` の `campaign.variant` は投入した値と一致する。
  - 通常 POST の再送: 同一 hash なら 200 で同じ id、`content_sha256` が違えば 409、variant id が違っても 409。`variant_org`・`variant`・`selection_sha256` のどれかが欠ければ 4xx。冪等キーを省くと、同じ試行の識別で既存 task に寄る（200・同じ id、新規なし）。新しいキーでも pin が欠けていれば 4xx。どの場合も task は増えない。
  - executor 側での拒否（POST 前、Refused）: selection hash の改変、content hash の形の不正、org pin の欠落、請求と executor の variant 不一致。保存済み variant と違う参照で照会したときは `lookup`・`resolve`・`submit` がいずれも Refused になる。
  - run が起きないこと: `worker_started` とスタブの起動は受理した task だけに出る。
  - 封印照合の再利用: `CelerisExecutor::prove_variant`（`variant_proof::prove_runs`）を、実 daemon の `GET /tasks/{id}`・`/org`・`/config` と `/health` の固定値（`FixedEnv`）に対して呼び、通ることを確かめる。同じ実観測でも、別 instance・別の封印参照・catalog 外・固定なし（env None）の場合は Refused になる。
  - canary: trial 1 の objective に canary を入れる。スタブは `trial-notes.md` を artifacts と作業 dir に残す。同じ daemon で続けて投入した trial 2（別キー・別作業 dir）について、`RunRequest` 全文・`workspace`・`work_dir`・`artifacts_dir` に canary が無いことを assert する。陽性対照として、trial 1 の request と痕跡には canary があることも確かめる。spec-on 環境の canary が spec-off 環境の fixture dir（DB・ログ・作業場所）全体と request に無いことも見る。
  - off / on の同一性: `NonTargetPin` の `diff` が空で値も等しい、coding 課の実効 profile hash が同じ、稼働 daemon の review と安全上限が同じ。`GET /harnesses`（builtin と宣言）、path・port・identity を除いた `GET /config`（builtin 役割 `implementer`・genre・`reviewer.adapter`・委譲と再試行の上限）、`GET /llm/models`、封印 catalog、実 run の selection の (role, harness, source, model) もすべて同じ。selection のモデルは封印した `role_models` に一致する。
  - 内部 review は既定のまま: `reviewer.adapter` は null、`escalate_on_fail` は既定の true、review harness は off / on で同じ。品質が上がったとは主張しない。
  - K2: `cases/final/k2-…/case.json` の `not_judged` が `[analysis, review]` のままであること、`verified stage=review` を主張する報告が `check_scope_claims` で違反になることを確かめる。
- `crates/campaign/tests/ablation_application_support/mod.rs`: スタブが受け取った `RunRequest` の全文を `stub/requests.jsonl` に追記するようにし、objective の写し `trial-notes.md` を artifacts と作業 dir に書くようにした。`read_requests`・`files_containing`・`TRIAL_NOTES` を足した。

## 証拠
- `cargo test -p celeris-campaign --test ablation_application` → 4 passed（22.7s）
- 変異確認（戻し済み）: trial 2 の prompt に canary を入れると「trial 2 RunRequest carries the trial 1 canary」で失敗した。off の根の `max_attempts` を 4 にすると「neg-off: live review / safety differ from B0」で失敗した。
- `cargo clippy -p celeris-campaign --all-targets -- -D warnings` → exit 0
- `bash scripts/dev/test-parallel.sh` → exit 0（5921 passed / 0 failed / 15 ignored）
- `cargo clippy --workspace -- -D warnings` → exit 0、`cargo fmt --all -- --check` → exit 0
- `grep -nE 'thread::sleep|tokio::time::sleep\(' <両 file>` → 該当なし

## 未解決・注意
- fake source には model discovery が無いので、稼働 daemon の `GET /llm/models` は空になる（`catalog_from_models` は空を拒否する）。そのため封印側の env-lock catalog は B0 の固定値（stub × stub）にした。稼働側は off / on の `GET /llm/models` が同じであることだけを比べている。実 catalog から env-lock を作る経路（`fetch_catalog`）は、この fixture では通していない。
- profile の `review.harness` は harness 語彙（`reviewer`）でなければ config の読み込みで落ちる。`deny_tools` も道具の語彙（`docker` 等）に限られる。
- この試験で走る run は worker だけである（atomic な直行経路）。planner run の封印照合は scenario 側の pin の assert に頼っている。
