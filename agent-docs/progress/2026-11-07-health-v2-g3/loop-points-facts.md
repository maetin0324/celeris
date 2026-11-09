---
title: 健全性v2 G3 loop-points facts — repo_facts の rev-parse の読み取りの失敗を facts_unreadable にする
tasks: [01M6ZDF9Q4BWD9DX6S9KX3KKS1]
status: done
updated: 2026-11-09
---
# 健全性v2 G3 loop-points facts — repo_facts の rev-parse の読み取りの失敗を facts_unreadable にする

完了日: 2026-11-09

WorkUnit `facts`（task 01M6ZDF9Q4BWD9DX6S9KX3KKS1 の unit loop-points の子）。evidence（01M6YJF7HJ9J0EQHRM5SN5TWV4）の final review が残した advisory 2 を直した。[ADR 2026-11-08 repo health を通常の開発・統合ループに強制する](../../adr/2026-11-08-health-enforcement-loop.md) の D1.1 の fail closed に合わせ、`task_core::required_checks::health::repo_facts` は、Git 管理でないと確かめられたときだけ `git = false`（`NotApplicable: not_a_git_repo`）にし、他の rev-parse の失敗は `unreadable`（`Unavailable: facts_unreadable:<理由>`）にする。基準は `$CELERIS_WU_BASE`（統合後は `git merge-base HEAD main`）。

## 実装前 brief
1. 根本原因: `repo_facts` は `git rev-parse --show-toplevel` が失敗すると `git --version` を試し、それが通れば「Git 管理でない」にしていた。時間切れ・壊れた作業ツリー（gitdir の消えた worktree）・所有者の検査（`dubious ownership`）でも `--version` は通るので、Git 管理の repo が `not_a_git_repo` になり health の項目が足されない（fail open）。症状は全経路の適用判定に出る。
2. 責務 owner: 事実を集める `repo_facts`（`crates/task-core/src/required_checks/health.rs`）。判定の `applicability` は変えない。
3. 既存の再利用先: `HealthRepoFacts.unreadable` と `applicability` の順序（読めない事実 → Git 管理でない）。closure の形（非 0 exit は stderr を `Err` に入れる）は `celeris::delivery::git_text` と `health_review/gate_tests.rs` の `git_in` で同じ。
4. 選択肢: (a) closure を exit code 付きの型に変える（全 caller が変わり、範囲外）。(b) stderr の git の文で「Git 管理でない」を確かめる。(b) を採った。exit code 128 は他の fatal と区別できないので (a) でも文を見る必要がある。
5. 新しい状態・分岐・fallback・port: 定数 `NOT_A_GIT_REPO_STDERR`（`"not a git repository (or any "`）だけ。git が親 dir まで探して見つからなかったときの 2 つの文（`... (or any of the parent directories): .git` と `... (or any parent up to mount point ...)`）に合い、壊れた gitfile・`GIT_DIR` の誤り（`not a git repository: '<path>'`）には合わない。
6. 撤去対象: `git --version` の probe（`repo_facts` の中）と、試験の偽の git の `--version` の腕・許可した引数の `--version`。
7. 不変条件: `not_a_git_repo` は git が Git 管理でないと答えたときだけ。それ以外の rev-parse の失敗は同梱の検査器があっても `facts_unreadable` で合格にしない。試験は下の 3 件。

## 変更
- 件名「G3 loop-points facts: rev-parse の読み取りの失敗を facts_unreadable にする」の commit
  - `crates/task-core/src/required_checks/health.rs`: `repo_facts` は rev-parse の `Err` が `NOT_A_GIT_REPO_STDERR` を含むときだけ `git = false` のまま返し、それ以外は `unreadable = "git rev-parse --show-toplevel を読めない: <stderr か理由>"` にする。`--version` の probe を消した。`HealthRepoFacts.git` と `repo_facts` の doc comment を直した。
  - `crates/task-core/src/required_checks/health_tests.rs`:
    - 偽の repo の「Git 管理でない」の文を実の git の文（`fatal: not a git repository (or any of the parent directories): .git`）にした。旧い文（`fatal: not a git repository`）は実の git が出さない文で、新しい判定では `facts_unreadable` になる。既存の `health_evidence_non_git_repo_is_not_applicable` の検査の中身は変えていない。
    - 欄 `rev_parse_error`（rev-parse が返す任意の失敗）を足した。
    - 試験 3 件を足した。
      - `health_enforce_facts_confirmed_non_repo_is_not_applicable`: git の 2 つの文と、実の git を Git 管理でない一時 dir で起こした場合（`LC_ALL=C`・`GIT_CEILING_DIRECTORIES`）が `not_a_git_repo` になり、base を見ない。既存の試験は偽の文しか見ないので、実の git の文と合うかを検出しない。
      - `health_enforce_facts_transient_rev_parse_failure_is_unavailable`: 時間切れ（`celeris::delivery` の文）・壊れた gitfile・`dubious ownership`・cwd の消失・空の stderr と、実の git の gitdir の消えた worktree が、同梱の検査器があっても `facts_unreadable:git rev-parse --show-toplevel を読めない: ...` になる。旧い実装はこれらを `not_a_git_repo` にしていた。
      - `health_enforce_facts_git_cannot_start_is_unavailable`: 偽の git の起動失敗・`celeris::delivery` の起動失敗の文・実の無い実行 file が `facts_unreadable` になり、git の呼び出しは rev-parse の 1 回だけ（別の probe が無い）。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `cargo nextest run -p task-core --lib -E 'test(/health_enforce_facts_|health_evidence_/)'` → exit 0（34 passed。health_enforce_facts_ の 3 件を含む）
- `bash scripts/dev/test-affected.sh` → exit 0（nextest 8040 passed・23 skipped、web の vitest 711 passed・tsc -b 通過、check-health は verdict=pass exit=0 fail_grade=0 worsened=0）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- `sh "$CELERIS_WU_SCOPE_PATHS"` → 変えた path は `health.rs`・`health_tests.rs`・この進捗 file だけ
- `sh scripts/dev/progress-index.sh --check` → exit 0

## 未解決
- check-health の new の 1 件（`dup.contract`、severity warning、`UnavailableReason` が `route_select/availability.rs` と同名）は、`health.rs` を作った unit `core-types` からある finding で、この unit の変更ではない（base の main には `health.rs` がまだ無いので new と数えられる）。fail にはならない。

## 提案
- git の文は locale で翻訳されうる。翻訳された文は `NOT_A_GIT_REPO_STDERR` に合わず `facts_unreadable` になる（合格にはならないが、Git 管理でない作業場所でも health が `error` になる）。`repo_facts` に渡す closure（`celeris::delivery::git_text` と、unit `entry` が作る経路共通の入口）が git を `LC_ALL=C` で起こすようにするとよい。どちらもこの unit の範囲外なので、unit `entry`・`delivery-path` で扱うことを提案する。
