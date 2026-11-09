---
title: 健全性v2 G3 evidence delivery-verify — delivery の照合で適用判定を呼び、health の証跡を必須にする
tasks: [01M6YJF7HJ9J0EQHRM5SN5TWV4]
status: done
updated: 2026-11-09
---
# 健全性v2 G3 evidence delivery-verify — delivery の照合で適用判定を呼び、health の証跡を必須にする

完了日: 2026-11-09

WorkUnit `delivery-verify`（task 01M6YJF7HJ9J0EQHRM5SN5TWV4）。[ADR 2026-11-08 repo health を通常の開発・統合ループに強制する](../../adr/2026-11-08-health-enforcement-loop.md) の D1.2 の「delivery の merge candidate」の行と D2.6 を `crates/celeris/src/delivery.rs` に実装した。基準は `$CELERIS_WU_BASE`（統合後は `git merge-base HEAD main`）。適用判定・証跡の型は `core-types`、照合の純粋関数（`verify_required_checks` の `HealthRequirement`）は `core-binding`、daemon が同梱の検査器を gate に設定する配線は並行の `bundle-path` が担う。

## 変更
- 件名「G3 evidence delivery-verify: delivery の照合で適用判定を呼び health の証跡を必須にする」の commit
  - `crates/celeris/src/delivery.rs`
    - `tick` が `task_dispatch::health_review::gate::bundled_checker()` を 1 回だけ読み、`tick_with(store, config, now, bundled)` に渡す。delivery の中で `bundled_checker()` を読むのはここだけ（`grep -n "bundled_checker()" crates/celeris/src` は `delivery.rs:209` の 1 行）。
    - `advance(..., bundled: Option<&CheckerId>)` → `rereview_before_merge(store, repo, old, bundled)` → `check_required_evidence(store, repo, d, bundled)`。
    - `check_required_evidence` は基底（`d.target_sha`）について `health::repo_facts(true, target, bundled, git_text)` で事実を集め、`health::applicability` を呼ぶ。
      - `Applies { checker }`: `git rev-parse <reviewed_sha>^{tree}` を読み、`HealthRequirement::Applies { checker, reviewed_tree }` を `verify_required_checks` に渡す（tree・検査器の版・report の digest・reviewable・verdict Pass を `core-binding` の `verify_health` が照合する）。tree を読めなければ再 review の理由にする。
      - `NotApplicable`: `HealthRequirement::NotRequired`（health の項目を要らない）。
      - `Unavailable { reason }`: 「基底 <sha> の repo health を評価できません（<reason>）」を再 review の理由にして返す（合格にしない）。
      - 案件名・repo 名・`self_improve`・selfdeploy（`self_repo`）・task の mode で分岐しない。`mode = prototype` が外すのは `workspace.toml` の必須検査だけで、health の要求は残る。
    - ff の前の照合（`check_candidate` と `check_required_evidence`）を `advance` から `rereview_before_merge` に移した。`advance` は base の 393 行から 380 行になった（下の「ADR・objective とのずれ」の 3）。
  - `crates/celeris/src/delivery/tests.rs`（既存の試験の追従。判定は弱めていない）
    - 同梱の検査器の fixture `bundled()`（source Bundled、sha256 `b`×64）。一時 repo の基底には検査器が無いので、既存の試験は `bundled()` を引数で渡して `Applies` で照合する。`configure_bundled_checker` は呼ばない。
    - `advance(.., now)` の 52 か所を `advance_bundled(.., now)`（`advance(.., Some(&bundled()))` の 1 行の wrapper）に、`tick` の 2 か所を `tick_with(.., Some(&bundled()))` に替えた。
    - `record_evidence`・`record_evidence_with_definition` は repo の path を受け、`bundled()` で評価した Pass の health の項目（`head_tree` は候補の tree）を足し、定義指紋を `definition_digest(definition, Some(&bundled()))` で作る。不合格・別の束縛・必須検査の欠けの各試験は、health の項目が揃った上で元の理由で拒否される。
    - `stored_delivery_in_mode`・`merge_queued_in_mode`（task の mode を選ぶ fixture。既存の `stored_delivery`・`merge_queued_without_evidence` はこれを Production で呼ぶ）、`health_evidence`・`health_item`・`record_results`・`tree_of`。
  - `crates/celeris/src/delivery/health_evidence_tests.rs`（新規。`tests.rs` の子 module `health_evidence`）: `health_evidence_delivery_` の試験 8 件。

## 実装前 brief
1. 根本原因: delivery の照合（`check_required_evidence`）は `HealthRequirement::NotRequired` を固定で渡していた。そのため health の証跡が無い・古い・不合格の候補も main に ff されていた（D1.2 の delivery の経路が適用判定を通らない）。
2. 責務 owner: `celeris::delivery::check_required_evidence`（ADR D1.2 の表の delivery の行の入口）。照合の条件そのものは `task_core::required_checks::verify_required_checks`（`core-binding`）が持つ。
3. 既存の再利用先: `health::repo_facts`・`health::applicability`（`core-types`）、`verify_required_checks` と `HealthRequirement`（`core-binding`）、`gate::bundled_checker()`（`checker-gate`）、delivery の `git_text`・`check_candidate`・`request_rereview`、既存の試験の fixture。
4. 選択肢: (a) `check_required_evidence` の中で `bundled_checker()` を読む案は、試験が process の OnceLock を共有して干渉するので採らない。(b) `advance` で読む案は、試験が `advance` を直接呼ぶので同梱の検査器を与えられない。(c) 採用: delivery の入口 `tick` で 1 回読み、`tick_with`・`advance`・`check_required_evidence` へ引数で渡す。照合の条件を delivery 側に書き直す案は `core-binding` と二重実装になるので採らない。
5. 新しい状態・分岐・fallback・port: 新しい状態・fallback・port は無し。分岐は適用判定の 3 値（ADR D1.2 の決定そのもの）。`tick_with` は `tick` の本体を引数付きにしただけ。
6. 撤去対象: `check_required_evidence` の `HealthRequirement::NotRequired` の固定（`core-binding` が置いた機械的な追従）。互換分岐・新旧切替は置いていない。
7. 不変条件と試験: 下の表。既存の delivery の試験 35 件は health の項目を揃えた上で元の判定のまま通る。

## 試験ごとの守る不変条件と、既存の試験で検出できない理由（規約 10）
既存の delivery の試験は `HealthRequirement::NotRequired` の照合だけを通っていた。`core-binding` の `health_evidence_` の試験は `verify_required_checks` を純粋関数として呼ぶので、delivery が適用判定を呼ぶか・何を渡すか（基底の事実・同梱の検査器・候補の tree）を見ない。

| 試験 | 守る不変条件 | 既存の試験で検出できない理由 |
|---|---|---|
| `health_evidence_delivery_applies_requires_the_health_item` | `Applies` の repo では、health 導入前の証跡（指紋に検査器の版が無い）・health の項目を欠く証跡・証跡の無い health の項目のどれでも ff しない | 既存の delivery の試験は `NotRequired` で照合していたので、health の項目が無くても ff した。`core-binding` の試験は delivery が `Applies` を渡すかを見ない |
| `health_evidence_delivery_rejects_evidence_for_another_tree` | health の証跡の `head_tree` が候補 `reviewed_sha` の tree と違えば ff しない（delivery が git で読む tree を渡す） | `core-binding` の試験は tree を文字列で渡す。delivery が `reviewed_sha` の tree を読んで渡すことは見ない |
| `health_evidence_delivery_rejects_another_checker_version` | health の項目の検査器が今の同梱の検査器と違えば ff しない。古い版で揃えた証跡（指紋も古い版）も ff しない | 既存の delivery の試験に検査器の版が無い。delivery が引数の同梱の検査器を適用判定に渡すことは他の試験が見ない |
| `health_evidence_delivery_prefers_the_base_checker_over_the_bundled_one` | 基底に検査器がある repo では基底の検査器で照合する（D1.4・J3）。同梱の検査器の Pass では ff せず、基底の検査器の Pass で ff する | 既存の delivery の試験の一時 repo には検査器が無い。`core-types` の試験は事実を手で作るので、delivery が基底（`target_sha`）の事実を git で集めることを見ない |
| `health_evidence_delivery_rejects_error_and_fail_evidence` | verdict が Error・Fail・PolicyOnly の証跡、項目の `pass` が true の Error、dirty な作業ツリーの report、report の digest の無い証跡では ff しない | 既存の delivery の試験に verdict・`reviewable`・report の digest が無い |
| `health_evidence_delivery_unavailable_is_rejected` | 基底にも同梱にも検査器が無ければ、合格の証跡があっても ff しない。基底の事実を読めなければ、同梱の検査器があっても合格にしない（J4） | 既存の delivery の試験は適用判定を呼ばないので `Unavailable` が起きない |
| `health_evidence_delivery_non_celeris_repo_passes_with_bundled_pass` | profile も検査器も無い Celeris でない repo でも、同梱の検査器の Pass の証跡があれば ff する（適用判定は repo の事実だけで決まる） | 既存の delivery の試験は health の結果を見ずに ff していた。Celeris でない repo を `Applies` で通す経路は他の試験に無い |
| `health_evidence_delivery_prototype_still_requires_health` | `mode = prototype` の task でも health の証跡が無ければ ff せず、Pass の証跡があれば ff する | 既存の delivery の試験は Production の task だけを使う。prototype の扱いは `workspace.toml` の必須検査を外す分岐だけで、health を外さないことは見ていない |

## ADR・objective とのずれ
1. 同梱の検査器の引数は `Option<&CheckerId>`（objective は `Option<CheckerId>`）。`health::repo_facts` が `Option<&CheckerId>` を受けるので、写しを作らずに渡した。
2. `bundled_checker()` を読むのは `check_required_evidence` の直接の呼び出し元 `advance` ではなく、delivery の入口 `tick`。試験は `advance` を直接呼ぶので、`advance` で読むと試験が同梱の検査器を与えられない。`tick` の本体を `tick_with` に分け、`tick_advances_only_projects_with_auto_integrate` は `tick_with` に `bundled()` を渡す。
3. ff の前の照合を `advance` から `rereview_before_merge` に移した。objective に無い変更。`advance` は base で 393 行あり、引数を 1 つ足すだけで check-health の `size.responsibility-growth`（関数の行が閾値 200 を超えて増えた）が fail になった。照合を切り出して 380 行にした。
4. `Unavailable` は再 review の理由の文として返す。ADR D1.2 は「health の項目を `error` の結果として足す」と書くが、delivery には結果の列が無い（列は review が記録する）。合格にしない点は同じ。
5. `NotApplicable` の経路は delivery の試験で通していない。配送の行は Celeris が手元で統合する作業ツリーにしか無いので `integrated_by_celeris` は常に true で、`NotApplicable` は Git 管理でないときだけになる。その場合は `check_candidate`・`candidate_required_checks` が先に失敗する。分岐は ADR D1.2 どおり実装した（`NotRequired` を渡す）。
6. PolicyOnly は通さない（`core-binding` の「ADR・objective とのずれ」の 3 を引き継ぐ）。
7. 試験は objective の 6 件に、基底の検査器を優先する試験と prototype の試験を足して 8 件にした。

## 証拠（提出時の HEAD で実行）
- `cargo nextest run -p celeris --lib -E 'test(/health_evidence_delivery_/)'`: exit 0、8 passed。
- `cargo nextest run -p celeris --lib -E 'test(/delivery::/)'`: exit 0、43 passed（既存 35 件 + 新規 8 件）。
- `bash scripts/dev/test-affected.sh`: exit 0。対象 crate は `celeris`・`celerisctl`。check-health は `verdict=pass exit=0 fail_grade=0 new=1 worsened=0`（作業ツリーの評価なので `reviewable=false`）。`new=1` は warning の `dup.contract`（`task-core` の `UnavailableReason` が `route_select/availability.rs` と同名。`core-types` の変更で、この unit の変更ではない）。nextest は 1001 passed・2 skipped。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `cargo fmt --all -- --check`: exit 0。
- `grep -rn "bundled_checker()" crates/celeris/src`: `crates/celeris/src/delivery.rs:209` の 1 行だけ。
- 範囲 check（`sh "$CELERIS_WU_SCOPE_PATHS"`）: `crates/celeris/src/delivery.rs`・`crates/celeris/src/delivery/tests.rs`・`crates/celeris/src/delivery/health_evidence_tests.rs`・この進捗 file だけ。

## 未解決事項
- review（`task-dispatch` の `review/required.rs`）はまだ health の項目を記録せず、定義指紋も `definition_digest(cmds, None)` で作る。この unit の後は、Git 管理のどの repo の配送も health の証跡が無いので再 review に戻る。review が health の項目を記録する配線は後続の `loop-points` が行う。G3 の木の子は親の branch に入るので main には届かないが、G3 を main に取り込むのは `loop-points` の後にする必要がある。
- daemon が同梱の検査器を設定するまで（並行の `bundle-path`）は、基底に検査器の無い repo の配送は `Unavailable`（`checker_missing`）で再 review に戻る。

## 提案
1. G3 の final review の受け入れに「review が記録する必須検査の証跡に health の項目があり、delivery の `check_required_evidence` を通る」ことを 1 本の試験で確かめる項目を入れる（`loop-points` の `health_gate_path_delivery`）。この unit の試験は証跡を手で作るので、review と delivery の間の食い違いは見ない。
