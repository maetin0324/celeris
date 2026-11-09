---
title: 健全性v2 G3 loop-points reuse-sync — review の再利用と review 前の main 同期で health を束縛どおりに扱う
tasks: [01M6ZDF9Q4BWD9DX6S9KX3KKS1]
status: done
updated: 2026-11-09
completed: 2026-11-09
---
# 健全性v2 G3 loop-points reuse-sync — review の再利用と review 前の main 同期で health を束縛どおりに扱う

WorkUnit `reuse-sync`（task 01M6ZDF9Q4BWD9DX6S9KX3KKS1、unit loop-points の子）。[ADR 2026-11-08 repo health を通常の開発・統合ループに強制する](../../adr/2026-11-08-health-enforcement-loop.md) の D2.5・D2.6 と、(1)(3) のうち review の再利用（ADR 2026-10-27 の reviewer の合格の再利用）と review 前の main 同期（ADR-0118）の経路を配線した。基準は `$CELERIS_WU_BASE`（統合後は `git merge-base HEAD main`）。

## 実装前 brief
1. 根本原因: review-path までで health の項目は review のたびに入口で評価される。`reusable_passes` は定義指紋を `definition_digest(required, None)` で比べるので、health の当たる候補では指紋が合わず必須検査の cmd を再利用しない。束縛の同じ health の証跡（`reusable_health`）を引く経路が無く、同じ tree の別の commit でも検査器を起こす。
2. 責務 owner: 再利用の規則は `task_core::required_checks`（`reusable_passes`・`reusable_health`）。review の項目を作るのは `health_review/review_item.rs`（`evaluate_all`）。
3. 既存の再利用先: `reusable_health`・`HealthReuseKey`・`HealthRequirement`・入口 `gate::health_gate`・`gate::applicable`。
4. 選択肢: (a) `reusable_health` の鍵の policy を呼び出し側が report から作る案は、report は検査器を起こさないと無いので使えない。(b) 採用: policy は base 側で評価される（同じ base・同じ検査器なら同じ値）ので、同じ `(repo, base, 検査器)` の最新の証跡の policy を鍵に使う `reuse_health` を task-core に置く。
5. 新しい状態・分岐・fallback・port: 無し。
6. 撤去対象: `spawn_review` が `reusable_passes` を直接呼んでいた分岐（`evaluate_all` の `reusable_checks` に移した。dispatcher から health_review への辺を作らないため）。互換分岐・flag は残していない。
7. 不変条件: health は (repo, base, head の tree, policy, 検査器) が同じ Pass のときだけ写し、写したら今の `(base, head)` に束縛し直して `reused_from` を書く。それ以外・同期で tree が変わったときは入口で評価し直す。試験は下の 6 件。

## 変更
- 件名「G3 loop-points reuse-sync: health の再利用を束縛どおりにし reused_from を書く」の commit
  - `crates/task-core/src/required_checks.rs`: `reusable_passes` に引数 `health: HealthRequirement` を足し、指紋を `definition_digest(required, health.checker())` で比べる。`HealthCandidate` と `reuse_health(events, &candidate) -> Option<HealthEvidence>` を足した（`reusable_health` を引き、今の `(base_sha, head_sha)` に束縛し直して `reused_from` に元の head_sha を書く）。
  - `crates/task-dispatch/src/health_review/review_item.rs`: `evaluate_all` に同じ task の `events` を渡す。束縛の同じ G2 の評価が無ければ `reuse_health` を引き、合えば検査器を起こさず写し、合わなければ入口 `health_gate` で評価し直す。`reusable_checks` は `evaluate_all` が repo ごとに決める必須検査の再利用で、今の適用判定の検査器を `reusable_passes` に渡す（適用判定が `Applies` でない・tree を読めなければ `NotRequired` で、health の当たる候補の指紋とは合わず再利用しない＝fail closed）。
  - `crates/task-dispatch/src/health_review/input.rs`: `attach` が `events` を受けて `evaluate_all` に渡す。
  - `crates/task-dispatch/src/dispatcher/review_spawn.rs`: `reusable_passes` の呼びを消し（`reusable` は `evaluate_all` が決める）、`attach` に `events` を渡す（行数は減る）。
  - 経路の流れ: `reviewer_reuse`（reviewer を省く）・`defer_stale_review` の後・`ReviewSync` の後は、いずれも同期の後の `(target_sha, reviewed_sha)` を束縛にして `evaluate_all` を通る。reviewer を省いても health の項目は付き、写せなければ評価し直す。head の tree は評価の時点で `git rev-parse <reviewed_sha>^{tree}` を読むので、rebase で tree が変われば前の証跡は引かれない。
- 件名「G3 loop-points reuse-sync: 進捗」の commit: この file。

## 試験（守る不変条件と、既存の試験で検出できない理由）
- `health_enforce_reviewer_reuse_rebinds_to_the_current_candidate`（task-core）: 写した証跡は今の `(base, head)` に束縛し直され `reused_from` に元の head が入る。`reusable_health` は束縛し直しを呼び出し元に任せるので、既存の試験は見ない。
- `health_enforce_reviewer_reuse_invalidates_on_any_binding_change`（task-core）: tree・base・検査器・repo・policy が違う、Fail・Error・dirty は写さない。
- `health_enforce_reviewer_reuse_passes_follow_the_checker_digest`（task-core）: `reusable_passes` の指紋が検査器を含む。
- `health_enforce_reviewer_reuse_same_tree_rebinds_and_writes_reused_from`（task-dispatch。実の git と stub の検査器）: 同じ tree の別の commit は検査器を起こさず写し、`reused_from` を書く。`health_enforce_direct_review_*` は event を渡さないので写す経路を通らない。
- `health_enforce_reviewer_reuse_reevaluates_when_the_binding_differs`: tree・base・検査器・policy・Fail・dirty・別 repo の証跡は写さず評価し直す。
- `health_enforce_reviewer_reuse_required_commands_follow_the_applied_checker`: `reusable_checks` は同じ検査器の指紋の cmd だけを返す。
- `health_enforce_pre_review_sync_rebase_changes_the_tree_and_reevaluates`: rebase で tree が変わる／target が進んで base の sha だけ変わるとき、同期の前の証跡を写さず同期の後の束縛で評価し直す。

## 証拠（提出時の HEAD で実行）
- `cargo nextest run -p task-core --lib -E 'test(/required_checks/)'` → exit 0（48 passed。`health_enforce_reviewer_reuse_` 3 件を含む）
- `cargo nextest run -p task-dispatch --lib --no-tests=fail -E 'test(/health_enforce_/)'` → exit 0（31 passed。`health_enforce_reviewer_reuse_` 3 件・`health_enforce_pre_review_sync_` 1 件を含む）
- `bash scripts/dev/test-affected.sh` → exit 0（nextest 8073 passed・23 skipped、check-health は verdict=pass fail_grade=0 worsened=0、new=1 は他の unit の warning `dup.contract`）。1 回目は deps.cycle（dispatcher → health_review の新しい辺）で落ち、`reusable` の決定を `evaluate_all` に移して直した
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0

## 未解決
- 無し。ただし D3.4（Error で task を Blocked にし受信箱へ）は review-path の未解決のとおり、この unit の範囲外。

## 提案
- 無し。
