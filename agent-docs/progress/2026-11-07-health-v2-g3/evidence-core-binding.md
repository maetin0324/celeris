---
title: 健全性v2 G3 evidence core-binding — definition_digest v3 と health の再利用・失効・delivery 照合
tasks: [01M6YJF7HJ9J0EQHRM5SN5TWV4]
status: done
updated: 2026-11-09
---
# 健全性v2 G3 evidence core-binding — definition_digest v3 と health の再利用・失効・delivery 照合

完了日: 2026-11-09

WorkUnit `core-binding`（task 01M6YJF7HJ9J0EQHRM5SN5TWV4）。[ADR 2026-11-08 repo health を通常の開発・統合ループに強制する](../../adr/2026-11-08-health-enforcement-loop.md) の D2.4〜D2.7 を `task_core::required_checks` に実装した。基準は `$CELERIS_WU_BASE`（統合後は `git merge-base HEAD main`）。型と適用判定（D1.1・D2.1〜D2.3）は先行の unit `core-types`、経路への配線（health の要求を delivery に渡す・`reusable_health` を呼ぶ）は後続の `delivery-verify` と `loop-points` が行う。

## 変更
- 件名「G3 evidence core-binding: definition_digest v3・reusable_health・delivery の health の照合」の commit
  - `crates/task-core/src/required_checks.rs`
    - `definition_digest(cmds, health: Option<&CheckerId>)`: domain を `celeris.required-checks/3` に上げた。入れる順は cmd の数・長さ付きの cmd 列、`health` が `Some`（適用判定が `Applies`）なら続けて `HEALTH_CHECK_ID` と `checker.sha256`。hash は `health::domain_sha256`（domain・0 byte・長さ付きの欄）を使い、v2 の手書きの hasher は消した（v2 の domain を読む経路は無い）。
    - `HealthRequirement { NotRequired | Applies { checker, reviewed_tree } }`: delivery の照合に渡す health の要求。`checker()` が指紋に入れる検査器を返す。
    - `HealthReuseKey { repo_id, base_sha, head_tree, policy_sha256, checker_sha256 }` と `reusable_health(events, &key) -> Option<&HealthEvidence>`（D2.5〜D2.7）。
    - `reusable_passes`: 引数は変えていない。指紋は `definition_digest(required, None)` で比べ、health の項目（`cmd == HEALTH_CHECK_ID`）は返さない（health の再利用は `reusable_health` だけが決める）。
    - `verify_required_checks(..., required, health: HealthRequirement)`: `Applies` なら `verify_health` が health の項目を照合する（下の「照合の条件」）。
  - 呼び出し元の機械的な追従（health の要求は渡さない）: `crates/celeris/src/delivery.rs`（`HealthRequirement::NotRequired`）、`crates/task-dispatch/src/review/required.rs`（`definition_digest(cmds, None)`）、試験 `crates/celeris/src/delivery/tests.rs`・`crates/task-dispatch/src/dispatcher/tests/git_workspace.rs`（`definition_digest(.., None)`）。
  - `crates/task-core/src/required_checks/tests.rs`: 既存の試験は `verify`・`digest`（health を要らない形）の薄い wrapper で呼ぶようにした（判定は変えていない）。`health_evidence_` の試験を 19 件足した。

### 再利用の条件（`reusable_health`）
- 読む正本は `RequiredChecksEvaluated.results[].health`・`WorkUnitCheckFinished.health`・`IntegrationCheckFinished.health` の 3 か所だけ。`PhaseIntegrated.checks[].health` は段の統合の記録の写しなので読まない（core-types の「event の正本の決定」）。
- 鍵 `(repo_id, base_sha, head_tree, policy_sha256, checker.sha256)` に束縛された証跡のうち、event の並びで最新の 1 件を選ぶ。その 1 件が verdict Pass・`reviewable == true`・`report_sha256` が空でない、のすべてを満たすときだけ返す。満たさなければ、それより前の Pass も返さない。
- `checker` の無い証跡（検査器の無い Error・NotApplicable）は鍵に束縛されない。
- 返った証跡を今の `(base_sha, head_sha)` に束縛し直し、`reused_from` を書くのは呼び出し元（loop-points）。

### 照合の条件（`verify_required_checks` の `Applies`）
今までの照合（今の `target_sha`・`reviewed_sha` に束縛された最新の `RequiredChecksEvaluated`・指紋・不合格・網羅）に、次を足した。
- `cmd == HEALTH_CHECK_ID` の項目があり、`health` の証跡を持つ。
- 証跡の `repo_id` が照合する repo と一致する。
- `head_tree` が `reviewed_tree`（呼び出し元が `git rev-parse <reviewed_sha>^{tree}` で渡す）と一致する。`reviewed_tree` が空なら不合格。
- `checker.sha256` が要求の検査器と一致する（source の違いは見ない。D2.3 で base と同梱は同じ式）。
- `report_sha256` が空でない。`reviewable == true`（D2.7）。
- verdict が Pass。PolicyOnly は不合格（下の「ADR・objective とのずれ」の 3）。Fail・Error・NotApplicable も不合格。
- `PhaseCheckResult.pass` が true でも verdict が Pass でなければ不合格（verdict が決める）。

## 実装前 brief
1. 根本原因: health の結果は core-types で event に載ったが、定義指紋が検査器の版を含まず、再利用・delivery の照合が health の束縛（tree・policy・検査器・report）を見ない。このままでは検査器・policy が変わっても古い合格が通る。
2. 責務 owner: `task_core::required_checks`（ADR 2026-10-23 の束縛・再利用・照合の純粋関数の正本）。
3. 既存の再利用先: `latest_bound`（束縛の最新の証跡）、`health::domain_sha256`（domain 付き長さ付き hash）、`HealthEvidence`・`CheckerId`・`HEALTH_CHECK_ID`（core-types）、既存の試験の fixture `evidence()`・`required()`。
4. 選択肢: (a) health の照合を delivery 側に書く案は、review・delivery で二重実装になるので採らない。(b) `reusable_passes` に health を足す案は、引数を変えられない（呼び出し元 `dispatcher/review_spawn.rs` は範囲外）うえ、鍵が `(target, reviewed)` で tree を使えないので採らない。(c) 採用: 別の関数 `reusable_health` と、`verify_required_checks` の引数 `HealthRequirement`。
5. 新しい状態・分岐・fallback・port: 新しい状態・fallback・port は無し。分岐は `HealthRequirement` の 2 値（`Applies` のときだけ health を照合）で、ADR D2.6 の決定そのもの。互換分岐（v2 の指紋を受け取る経路）は置いていない。
6. 撤去対象: v2 の domain `celeris.required-checks/2` の指紋の計算（`required_checks.rs` の手書きの hasher と `sha2` の import）。v2 の指紋の証跡はすべて不一致で失効する（ADR D2.4 の想定どおり、昇格の後の配送は 1 回だけ再 review）。
7. 不変条件と試験: 下の表。

## 試験ごとの守る不変条件と、既存の試験で検出できない理由（規約 10）
既存の試験（`tests.rs` の 11 件、`health_tests.rs` の 12 件）は、health の項目の無い必須検査の束縛と、適用判定・型・指紋の式だけを見る。`reusable_health` と `HealthRequirement` は今回新しく作ったので、既存の試験はどれもこれを呼ばない。

| 試験 | 守る不変条件 | 既存の試験で検出できない理由 |
|---|---|---|
| `health_evidence_definition_digest_is_v3_with_checker` | 指紋は v3 の domain で、`Applies` なら cmd 列の後に `HEALTH_CHECK_ID` と `checker.sha256` が入る。cmd の数が先に入るので、cmd に health の項目を並べた定義と取り違えない | `definition_digest_depends_on_order_and_content` は cmd の順と中身だけを比べ、domain の版と health の欄を見ない |
| `health_evidence_definition_digest_changes_with_checker_version` | 検査器の版が変われば指紋が変わり、delivery は指紋の不一致で拒否する。同じ版なら base と同梱で同じ指紋 | 既存の試験の指紋は検査器を入力に持たない |
| `health_evidence_v2_definition_digest_is_invalid` | v2 の指紋の証跡は照合も再利用もされない（互換分岐が無い） | `ambiguous_multiline_commands_and_pre_v2_evidence_cannot_be_reused` は v2 より前の改行区切りの指紋だけを見る |
| `health_evidence_same_tree_reuses_pass` | 同じ tree の別の commit でも、3 つの正本のどれにある合格も写せる。写した証跡（`reused_from` あり）も最新の合格として写せる | 既存の再利用（`reusable_passes`）は commit の SHA で引くので、tree での再利用を試せない |
| `health_evidence_head_change_invalidates_pass` | `head_tree` が変われば写さない。delivery は候補の tree と違う証跡を拒否する | 既存の試験は tree を持たない |
| `health_evidence_base_change_invalidates_pass` | `base_sha` が変われば写さない | 既存の試験の base の変化は `RequiredChecksEvaluated.target_sha` の束縛だけで、WU・段の統合の証跡を見ない |
| `health_evidence_policy_change_invalidates_pass` | `policy_sha256` が変われば写さない | 既存の試験に policy の欄が無い |
| `health_evidence_checker_change_invalidates_pass` | 検査器の版が変われば写さない。指紋が今の検査器でも、項目の証跡が別の検査器なら delivery は拒否する | 既存の試験に検査器の欄が無い |
| `health_evidence_repo_change_invalidates_pass` | 別の repo の証跡は写さず、項目の `repo_id` が照合の repo と違えば拒否する | `changed_candidate_base_or_repo_invalidates_evidence` は event の `repo_id` だけを見て、証跡の中の `repo_id` を見ない |
| `health_evidence_report_digest_change_invalidates_pass` | 同じ束縛で後から別の report の証跡が記録されたら前の合格は返さない（最新の証跡が決める）。後の Fail・report の無い証跡があれば写さない。report の無い証跡は delivery に使わない | 既存の試験に report の digest が無い |
| `health_evidence_fail_is_neither_passed_nor_reused` | Fail は合格にも再利用にもならず、後に記録されれば前の合格も写さない。`PhaseCheckResult.pass` が true でも verdict が決める | 既存の試験は `pass` の真偽しか見ない |
| `health_evidence_error_is_neither_passed_nor_reused` | Error（exit 2・report が無い等）は合格にも再利用にもならない（J4） | 同上 |
| `health_evidence_unknown_is_neither_passed_nor_reused` | report/1 の rule の unknown（exit 2 → Error）は合格にも再利用にもならない | 同上 |
| `health_evidence_unsupported_is_neither_passed_nor_reused` | report/1 の rule の unsupported（exit 2 → Error）は合格にも再利用にもならない | 同上 |
| `health_evidence_policy_only_is_not_passed_without_bound_approval` | PolicyOnly は再利用されず、承認を束縛から判定できないので delivery でも合格にしない | 既存の試験に verdict が無い |
| `health_evidence_dirty_report_is_not_reused_or_delivered` | `reviewable == false`（dirty な作業ツリー）の証跡は再利用にも delivery にも使わない（D2.7） | 既存の試験に `reviewable` が無い |
| `health_evidence_phase_integrated_health_ignored` | `PhaseIntegrated.checks[].health` は読まない。写しの合格では写さず、写しの不合格は正本の判定を変えない | core-types は読む側が無いので、この試験を置けなかった（core-types の進捗に記載） |
| `health_evidence_applies_requires_health_item` | `Applies` なら health の項目と証跡が要る。health を要らない照合は検査器の版入りの指紋の証跡を受け取らない | 既存の照合は health の要求を引数に持たない |
| `health_evidence_reusable_passes_never_copies_health` | `reusable_passes` は health の項目を返さない（health の再利用の口は `reusable_health` 1 つ） | 既存の `only_same_binding_passes_are_reusable` に health の項目が無い |

## ADR・objective とのずれ
1. 指紋に cmd の数を先に入れた。ADR D2.4 は「cmd 列の後に `HEALTH_CHECK_ID` と `checker.sha256` を長さ付きで足す」だけを書く。そのままでは、cmd が `["celeris:repo-health/1", "<sha>"]` の定義（health なし）と、cmd が空で health ありの定義が同じ指紋になる。数を入れて区別した（試験 `health_evidence_definition_digest_is_v3_with_checker`）。
2. 再利用の鍵と「最新の証跡」。ADR D2.5 の鍵は `(repo_id, base_sha, head_tree, policy_sha256, checker.sha256)` で、report の digest を含まない。objective に従い、同じ鍵の証跡のうち最新の 1 件だけを判定に使う。そのため、同じ鍵で後から別の report（`report_sha256` の違う証跡）が記録されると、前の Pass は返らない。後の証跡が Pass なら後の証跡を返し、Fail・Error・PolicyOnly・dirty・report なしなら何も返さない。dirty（`reviewable == false`）な証跡も最新として数える（同じ鍵の後の評価が合格でない事実を無視しない。fail closed）。
3. PolicyOnly の承認は判定できないので不合格にした。ADR D2.6 は「PolicyOnly で G2 の `policy_review_gate` の承認がある」なら通す。しかし G2 の承認は `Approval` の子 task の状態（`health_review/policy_review.rs` の `approval_verdict`）と review の verdict の理由の文で表され、`(repo, target_sha, reviewed_sha)` に結び付いた event は親の task の events に無い（子の title は `reviewed_sha` の先頭 12 文字だけ）。純粋関数の `verify_required_checks` からは判定できないので、PolicyOnly は不合格のままにした（「提案」の 1）。
4. 照合に `repo_id` と `reviewable` の検査を足した。objective の照合の列挙（項目の有無・tree・検査器・report digest・verdict）には無い。`reviewable` は D2.7（dirty な report は delivery の証跡に使わない）、`repo_id` は束縛の J2 から足した。
5. `HealthRequirement::Applies` に `reviewed_tree` を持たせた。objective は「Applies の `CheckerId` か、要らないか」を渡すと書く。`head_tree` が `reviewed_sha` の tree と一致するかを純粋関数で確かめるため、tree を呼び出し元から受け取る（git を引数にしない）。
6. 照合は証跡の `base_sha`・`head_sha` を event の `target_sha`・`reviewed_sha` と比べない。D2.6 の照合は event の束縛と tree・検査器・verdict を求め、health の base が `target_sha` か merge-base かは配線（loop-points）が決めるため。

## 証拠（提出時の HEAD で実行）
- `out=$(cargo test -p task-core --lib health_evidence_ 2>&1) …`（計画の check 1）: exit 0、`test result: ok. 31 passed`（core-types の 12 件 + 今回の 19 件）。
- `cargo test -p task-core --lib required_checks && cargo test -p celeris --lib delivery && cargo test -p task-dispatch --lib review`（check 2）: exit 0、42 / 39 / 210 passed。
- `cargo clippy --workspace -- -D warnings`: exit 0（vendored `deno_core` の既存の deprecated 警告 1 件だけ）。
- `cargo fmt --all -- --check`: exit 0。
- `bash scripts/dev/test-affected.sh`: exit 0。check-health は `verdict=pass exit=0 fail_grade=0 new=1 worsened=0`（作業ツリーの評価なので `reviewable=false`）。nextest は 8012 passed・23 skipped、web は 104 files・711 tests passed、test-ts-verify は 20 passed。
- 範囲 check（`sh "$CELERIS_WU_SCOPE_PATHS"` と計画の正規表現）: 範囲外の path なし。

## 未解決事項
- `reusable_health` と `HealthRequirement::Applies` を呼ぶ経路はまだ無い。delivery は `NotRequired` を渡す（配線は `delivery-verify`、review・WU・段の統合は `loop-points`）。
- `reusable_passes` は `definition_digest(required, None)` で比べる。review が `Applies` の指紋を記録するようになると、`reusable_passes` は一致しなくなり必須検査を再実行する。`reusable_passes` の引数に health の要求を足すのは、`review_spawn.rs` を範囲に持つ `loop-points` の仕事。

## 提案
1. PolicyOnly を delivery で通すには、G2 の承認（`Approval` の子 task の done）を `(repo, target_sha, reviewed_sha)` に結び付けて親の task の events に残す必要がある。既存の event を使うなら、`RequiredChecksEvaluated` の health の項目に承認の子 task の id を残す案がある。どちらにするかは loop-points の設計で決め、ADR の付記に書く。
2. `reusable_passes` に `HealthRequirement` を渡す（引数を足す）変更を loop-points の write_set に入れる。
