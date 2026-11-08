---
title: 健全性v2 G2 dispatch policy-unknown — policy の状態を決められない candidate を人の承認なしに合格させない
tasks: [01M6VTFG9K5HEE817EW05JASMZ]
status: done
updated: 2026-11-08
---
# 健全性v2 G2 dispatch policy-unknown — policy の状態を決められない candidate を人の承認なしに合格させない

完了日: 2026-11-08

WorkUnit `policy-unknown`（final review の差し戻し (a)(b) の修正）。基準は `$CELERIS_WU_BASE`（範囲 check）と `git merge-base HEAD main`（check-health・test-affected）。設計は [ADR 2026-11-07 健全性を設計と review に組み込む](../../../adr/2026-11-07-health-review-integration.md) の I8・D6.4・D4.3 と「fallback: 新しく置かない。読めないときは fail closed」。

## 変更

件名「G2 policy-unknown: policy の状態を決められない candidate を不明として記録し、人の承認なしに合格させない」の commit。

- `crates/task-core/src/model/events.rs`: `Event::HealthReviewInputSkipped` に serde default の欄 `target_sha`・`reviewed_sha`・`policy`（`HealthPolicyState` の JSON の写し）を足した。新しい Event の種類・状態・config 欄・切替 flag は足していない。生成物 `docs/api/v1/api-v1.schema.json`・`docs/api/v1/event.schema.json`（`UPDATE_SCHEMA=1`）・`web/api/generated/{schema.json,types.ts}`（`node web/scripts/gen-types.mjs`）と `docs/api/v1/api.md` の event の説明を更新した。
- `crates/task-dispatch/src/health_review/input.rs`（材料作り）: base に検査器（`repo_health.py`）がある candidate repo は、policy の状態を必ず candidate の SHA に結び付けて event に残す。
  - 方針 `Present` で report を作れない（report を書かない・600 秒の時間切れ・壊れた report・schema 違い）: これまでどおり reviewer に `findings.verdict = "error"` と理由を渡し、`policy.source = "unavailable"`（定数 `POLICY_UNKNOWN`。不明）の材料を `HealthReviewInputBuilt` に記録する（以前は記録しなかった）。`find_recorded` は不明の材料を再利用しないので、次の review で report を作り直す（D4.3 の再利用は既知の材料だけ）。
  - 方針の読み込みが `Unavailable`・`--print-review-policy` の失敗・base の検査器を出せない（`with_base_checker` の失敗）・diff を取れない: reviewer には渡さず（従来どおり）、`HealthReviewInputSkipped` に理由と `policy = {source: "unavailable"}`（不明）を残す。
  - 方針が `Absent`（base の profile に `[review]` が無い）で検査器はある: reviewer に渡す health 文脈は変えず（Skipped）、report を 1 回走らせて policy の状態を `HealthReviewInputSkipped.policy` に残す。同じ candidate に既知の記録があれば report を走らせない。report を作れなければ不明。
  - base に検査器が無い repo は従来どおり Skipped（`policy` なし）で、関門は掛からない。
- `crates/task-dispatch/src/health_review/policy_review.rs`（D6.4 の関門）: `policy_changes` が `HealthReviewInputBuilt.input.policy` と `HealthReviewInputSkipped.policy` の両方を candidate `(repo, target_sha, reviewed_sha)` ごとに読む（`candidate_policy`）。既知の記録があれば最後の既知の状態で決め（`changed` なら承認を求める）、既知の記録が無く不明の記録があれば不明として承認を求める。記録を読めない event も不明に数える。`PolicyChange` は `repo_id・target_sha・reviewed_sha・files・diff（reviewer に渡した材料だけ）・unknown（理由）` を持つ。承認の子の題は従来どおり `Policy change approval: <repo_id> @ <sha12>`、本文は不明なら `policy state unknown: <理由>` を書く。人の承認の verdict の reason は `policy state unknown (<理由 300 字まで>)`。不明には D6.3 の reviewer の policy verdict を求めない（`verdict.rs` は `policy.changed` しか見ないので変えていない）。
- `dispatcher/` の本体の行は変えていない（試験の file `dispatcher/tests/health_review_input.rs` だけ）。`scripts/dev/repo-health/`・`scripts/dev/check-health.sh` は変えていない。LLM は呼ばない。
- `docs/protocol/worker-protocol.md`: report を作れないときの `findings`・`policy.source = unavailable` と、不明には `health.policy` を求めず dispatcher が人の承認を求めることを書いた。
- 試験:
  - 新規 `crates/task-dispatch/src/health_review/policy_review/unknown_tests.rs`（prefix `health_review_policy_unknown_`、5 件）。一時 git repo の main に stub の `repo_health.py` を commit し、`build_health_input` が stub を本当に起こして残した event を `policy_review_gate` が読む。
    - `candidates_wait_for_a_human`（受け入れ条件 0）: report を書かない・壊れた report・`--print-review-policy` の失敗・absent で report を書かない、の 4 つの candidate で、reviewer の合格だけの outcome に承認の子（題は candidate の SHA の鍵、本文に不明の理由と candidate の SHA）を 1 件作り、`pending_children` が 1、`resolve_human_approvals` が `None`（合格しない）。同じ candidate の再判定で子を作り直さない。
    - `human_approval_passes_and_rejection_replans`（受け入れ条件 0）: 承認で `policy_rereview` が true、やり直しの判定で全部合格（`health:policy approved by a human: policy state unknown (…)`）。却下は `classify_review_failure` が `Substantive`。
    - `rebuilt_report_decides`: 不明の後に同じ candidate の既知（変更なし）の材料が記録されれば承認を求めない。
    - `absent_policy_change_needs_approval`（受け入れ条件 1）: absent の repo で policy の file を変えた candidate は、`HealthReviewInputBuilt` を作らない（reviewer に health 文脈を渡さない）まま承認を求め、本文に `policy files: <file>`。
    - `no_checker_and_normal_change_pass_without_approval`（受け入れ条件 2）: 検査器の無い repo、Present・absent で policy 不変の通常変更は承認の子を作らず、verdict も変えない。
  - 流れ全体 `dispatcher/tests/health_review_input.rs` の `health_review_policy_unknown_flow_missing_report_waits_for_a_human`: report を書かない stub で `spawn_review` から流すと、reviewer が合格を書いても task は `reviewing` のまま、承認の子が 1 件（題に candidate の SHA、本文に不明の理由）。
  - 既存の `health_review_input_failed_report_is_not_recorded` は新しい扱いに合わせて `health_review_input_failed_report_is_recorded_as_unknown` に改めた（不明の材料を記録し、2 回目も report を走らせる）。`health_review_input_policy_absent_is_skipped`（単体・流れ全体の 2 件）は、report が 1 回走り Skipped に policy の状態が残ることを確かめるように改めた。

## 判断

- 不明の記録に新しい Event の種類を足さず、reviewer に材料を渡した repo は `HealthReviewInputBuilt`、渡さなかった repo は `HealthReviewInputSkipped` に serde default の欄を足して残した。Built は「reviewer に渡した材料」の記録なので、渡していない repo（方針を読めない・absent）に Built を書くと意味が変わるため。
- 同じ candidate に既知と不明の記録がある場合は既知を優先した。base（`target_sha`）が同じなら検査器と方針も同じで、report の policy の状態は candidate ごとに 1 つに決まるため（不明は一時的な失敗）。
- `with_base_checker` 自体の失敗（target を解決できない等）は、base に検査器があるかを確かめられないが fail closed として不明にした。

## 証拠（提出時の HEAD で実行）

コードの検査は件名「G2 policy-unknown: …」の commit（この進捗 file を足す前。コードは提出時の HEAD と同じ）で実行した。進捗 file の commit の後に文書検査と health_review_ の試験を再実行した。

- `cargo nextest run -p task-dispatch -E 'test(/health_review_/)'` → exit 0、62 passed（うち `health_review_policy_unknown_` 6 件）
- `bash scripts/dev/test-affected.sh` → exit 0。check-health は verdict=pass・new=0・worsened=0。web の tsc -b と vitest 711 件・node --test 113 件が合格。nextest 7,931 件 passed（23 skipped、293.1 秒）。script bundle SKIP 0 件。
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- `sh scripts/dev/check-doc-links.sh && sh scripts/dev/check-adr-numbers.sh && sh scripts/dev/progress-index.sh --check && python3 scripts/dev/check-architecture-map.py` → exit 0
- 範囲: `sh "$CELERIS_WU_SCOPE_PATHS"` の変更 path は `crates/`・`docs/api/v1/`・`docs/protocol/`・`web/api/generated/`・この進捗 file だけ。

## 未解決

- reviewer run の無い final review（`Check::Reviewer` の無い task）には関門が掛からない。並行の WorkUnit `policy-no-reviewer` の範囲。
- 同期の無い repo・remote workspace の repo は candidate の SHA が決まらないので記録も関門も無い（従来どおり）。
- reviewer run を起こしたのに材料作りの非同期 task が panic した（`spawn_blocking` の失敗）ときは event が残らず、関門は掛からない。
- ADR の付記（D4.3「記録せず」の文）と architecture-map の行は、後の WorkUnit `reclose` で新しい扱いに合わせる。

## 提案

- absent の repo でも policy の file を変えた candidate を reviewer に知らせる（health 文脈に policy の状態だけ載せる）かは、`reclose` で ADR に決めて書くとよい。今回は「reviewer に渡す health 文脈は今のまま」の指示に従った。
