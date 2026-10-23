---
title: 全 harness が共通の必須規則に届くよう指示の正本と配布経路を揃える
tasks: [01M5KWPP3YPZMZ71PZP6HAGNFD]
status: done
updated: 2026-10-23
completed: 2026-10-23
---

# 共通 agent 指示の正本と配布

- 基準 SHA（着手時の main）: 13d64acd1780。attempt 2 の途中で main e3b1ace40fc6（健全性ゲート task）を no-ff で取り込んだ（衝突なし）。この記録の統合基準は main `a3933dc2`。
- 検証対象（attempt 4 の再検証）: commit「phase 1: attempt 4 の進捗記録」（rebase 前。件名で参照）。最終の検証対象は末尾の「final-sync」節を見ること。
- 設計: [ADR 2026-10-23-required-rules-prompt-delivery](../adr/2026-10-23-required-rules-prompt-delivery.md)。

## 各 harness が自分で読む file（実測。配布物を読んで確認、CLI は起動していない）

| harness | 版 | 読む file | 何が届くか（この変更後） |
|---|---|---|---|
| Claude Code | 2.1.303 | `CLAUDE.md`（worker は project 設定を外す引数を付けない） | worktree の CLAUDE.md を直接読む |
| Codex | 0.162.1 | `AGENTS.override.md`/`AGENTS.md` のみ（既定 `project_doc_fallback_filenames = []`） | prompt（stdin）先頭の CLAUDE.md 全文 |
| Pi | 0.84.2 | 各 dir で `AGENTS.override.md`・`AGENTS.md`・`AGENTS.MD`・`CLAUDE.md` の最初の 1 つ | prompt（stdin）先頭の CLAUDE.md 全文（AGENTS.md が無ければ pi 自身も CLAUDE.md を読む） |
| opencode（ACP） | 配布 binary | `AGENTS.md`・`CLAUDE.md`・`CONTEXT.md` の最初の 1 つ | `session/prompt` の文面先頭の CLAUDE.md 全文 |

Codex は CLAUDE.md を一切読まず、Pi・opencode は AGENTS.md があると CLAUDE.md を読まない。よって main の時点で Codex には規則が届いていなかった。

## attempt 1 の差し戻しと attempt 2 の変更

attempt 1 は CLAUDE.md を AGENTS.md の `celeris:rules` 節へ写した。差し戻し理由: CLAUDE.md→AGENTS.md の symlink の repo では写すたびに節が入れ子で増える。AGENTS.md を追跡する repo では追跡ファイルを書き換え、AGENTS.md を ignore しない repo では未追跡ファイルを残す。試験も関数を直接呼ぶ単体試験だけだった。

attempt 2:
- `crates/task-worker/src/required_rules.rs`（新規）: `preamble` で CLAUDE.md の全文を見出し付きの前置きにする。CLAUDE.md が無ければ空。壊れた symlink・読めない・空は Err になる。`codex.rs`・`pi.rs`・`acp.rs`（CoS の再組み立てを含む）は prompt の先頭に置き、Err なら harness を起動せず run を落とす。
- 作業ツリーには何も書かない。attempt 1 の `skills::deliver_required_rules`・`rewrite_rules_section`・`RULES_BEGIN/END` とその試験は撤去し、`skills.rs` は main と同じに戻した。
- `CLAUDE.md`・`agent-docs/README.md`・`docs/architecture-map.md`: 配布経路の説明を prompt 前置きに直した（「AGENTS.md は tracked ではない」は agent-platform について、と明記）。
- attempt 1 から引き継いだ変更: CLAUDE.md の誤った symlink 説明と、実装済み機能と衝突する禁止事項（Web UI・リモート実行・予算管理）を直し、初期 ADR は歴史的記録だと明記した。`workspace.rs` の説明を実装に合わせ、死んだ `RemoteWorkspace`・`RemoteUnsupported` を削除した。地図に「共通 agent 指示と配布」節を足した。歴史的 ADR は書き換えていない。

## 試験（fresh worktree を `git worktree add` で切り、stub CLI で adapter の run を通す）

fixture は `test_support::fresh_worktree_with_rules`。この repo の実際の CLAUDE.md を commit した repo から worktree を切る。AGENTS.md の形は 4 通り（なし、人が書いた tracked file、AGENTS.md→CLAUDE.md、CLAUDE.md→AGENTS.md）。
- `codex/tests.rs::required_rules_reach_codex_from_a_fresh_worktree`、`pi/tests.rs::required_rules_reach_pi_from_a_fresh_worktree`、`acp/tests.rs::required_rules_reach_acp_from_a_fresh_worktree`: run を 2 回回す。毎回、stub が受け取った prompt に CLAUDE.md の全文がちょうど 1 度ある（ACP は `session/prompt` の JSON を解いて確認）。AGENTS.md は不変（marker 外のユーザー指示を維持）。`git status --porcelain --ignored --untracked-files=all` は空。
- `claude_code/tests.rs::required_rules_reach_claude_code_from_a_fresh_worktree`: claude は worktree を cwd として起動され、そこに正本と同じ CLAUDE.md が見える。`--setting-sources`・`--bare` は付かない。前置きは足さない。作業ツリーは不変。
- `{codex,pi,acp}/tests.rs::*_run_fails_when_required_rules_are_unreadable`: 壊れた symlink または空の CLAUDE.md で run が `required rules delivery failed` の Err になり、stub は起動しない。
- `required_rules/tests.rs`: 前置きの形、欠落、壊れた symlink、空、読めない（ディレクトリ）、4 通りの worktree で 3 回呼んでも作業ツリーが不変、この repo が CLAUDE.md を追跡し AGENTS.md を追跡しないこと。

## 検証（コマンドと結果）

- `cargo nextest run -p task-worker required_rules`: 12 passed。
- `bash scripts/dev/check-health.sh`（main 取り込み後）: exit 0（source-size: failure 0、地図: 449 パス OK、doc-links・adr-numbers OK）。
- `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv`: ok。
- `bash scripts/dev/test-affected.sh`・`cargo clippy --workspace -- -D warnings`・`cargo fmt --all -- --check`: 結果は下の「最終検査」。
- 全 workspace・e2e・browser・userns の試験は走らせていない（ADR 2026-10-20-full-tests-at-release-only）。

## 最終検査（main 取り込み後の作業ツリー）

- `bash scripts/dev/test-affected.sh`: exit 0（対象 crate: celeris celeris-campaign celerisctl llm-proxy task-api task-dispatch task-worker、4464 passed / 15 skipped）。
  - 取り込み後の 1 回目は `celeris::root_done_summary restart_does_not_duplicate` が負荷下で落ちた（"Discord once" left 3）。単独で回すと 3 回とも通った。`post_webhook` には再試行が無いので、原因は別の試験が手放した port を偽 webhook が拾い、その試験の HTTP 要求を本文として数えたこと、と判断した。偽 webhook は `POST /hook` だけを数え、他の要求には 404 を返すように直した（`crates/celeris/tests/root_done_summary.rs`）。直した後は上のとおり exit 0。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。

## integration_repair の rebase

integration_repair でまず main `01a1753d` に rebase し、その後 main `a3933dc2` に再度 rebase した。2回目では次の衝突を解消した。
- `crates/celeris/tests/credential_wait_notify.rs`: branch 側の superset fake webhook（`POST /hook` のみ記録し、EOF/他 request を通知として数えない）を残し、main 側の回帰試験も保持した。
- `docs/architecture-map.md`: 6列の表を維持し、main が変更した cell を写した。main で追加された DAG A2 行も6列にし、関連試験に `crates/campaign/tests/dagbench_harness.rs` を記載した。

## attempt 3（試験の flaky 2 件の修正）

attempt 2 の審査で `bash scripts/dev/test-affected.sh` が exit 100 になった（指示配布の変更とは関係しない試験が負荷下で落ちた）。attempt 3 の rebased commit は 「phase 1: 試験の echo listener と ablation fixture の port 衝突を直す（attempt 3）」（rebase 前。件名で参照）。
- `task-worker browser_egress::tests::egress_test_loopback_connect_allowed_only_when_listed`（403）: echo listener は最初の 1 byte を echo したら閉じていた。別の試験 process が、自分で bind して手放した port に byte を送ると、この listener がそれを echo して閉じ、proxy の connect は拒否されて 403 になる。listener は試験が abort するまで全接続を echo するように直した。byte を送る stray 接続を試験に足し、この状況を決定的に作る。
- `celeris-campaign::ablation_application spec_off_and_on_apply_through_real_dispatcher_and_worker`（attempt 3 の 1 回目の test-affected で発生）: 空き port を確かめてから daemon が bind するまでの間に、他の process が同じ port を取った（`Address already in use`）。fixture（`crates/campaign/tests/ablation_application_support/mod.rs`）は、listen の前に bind の衝突で終わったときだけ、新しい port で起動し直す（最大 5 回）。stdout・stderr の EOF まで読み切ってから判定する（`Signal::LogClosed`）。ほかの失敗は従来どおり daemon.log 付きで落ちる。
- 検査（attempt 3 の最終作業ツリー）: `cargo nextest run -p task-worker -E 'test(/browser_egress::tests::egress_test_loopback/)'` 2 passed、`cargo nextest run -p celeris-campaign --test ablation_application` 4 passed、`bash scripts/dev/test-affected.sh` exit 0（4470 passed / 16 skipped、source-size failure 0、地図 449 パス OK）、`cargo clippy --workspace -- -D warnings` exit 0、`cargo fmt --all -- --check` exit 0。

## attempt 4（条件 3: 地図の関連試験・不変条件）

attempt 3 の審査で条件 3 だけが不合格だった。地図のうち、関連試験と不変条件を持っていたのは「共通 agent 指示と配布」の 4 行だけだった。attempt 4 の rebased commit は 「phase 1: architecture-map の全節に関連試験・不変条件の列を足し、列の形を地図検査で強制する（attempt 4）」（rebase 前。件名で参照）（その進捗記録 commit は 「phase 1: attempt 4 の進捗記録」（rebase 前。件名で参照））。実装（`crates/` の本体）には触っていない。
- `docs/architecture-map.md`: 全節（task-core・task-dispatch・task-worker・task-ops・task-api・celeris・周辺 crate・Web・共通指示・ソース健全性）の表を同じ 6 列（subsystem | owner | entry point | ADR / 設計 | 関連試験 | 不変条件）に揃えた。119 行すべてに、実在する試験 file・module のパスと、不変条件 1 行（根拠 ADR 付き）を書いた。対象は TaskStore と events（`store/tests.rs` の `append_event_*`）、Dispatcher の状態遷移（`dispatcher/tests/*.rs`）、worker adapter（`{claude_code,codex,pi,acp}/tests.rs`）、task-api（`crates/task-api/tests/*.rs`）、celeris daemon（`classify_tick`・`instance_handoff` 等）、web（`*.test.ts(x)`・`web/e2e`）など。ADR やソースの全文は貼っていない。
- `scripts/dev/check-architecture-map.py`: 既存のパス実在検査に、表の形の検査を足した。見出しは 6 列、各行の関連試験には backtick のパスが 1 つ以上、不変条件は空でないこと。形の試験 `scripts/tests/test_architecture_map.py` に負例 3 つ（関連試験なし、不変条件が空、4 列の見出し）を足した。
- 回帰試験の flaky を 1 件直した。attempt 4 の 1 回目の test-affected で `celeris::credential_wait_notify wait_notify_register_verify_and_resume_once_through_the_fake_webhook` が落ちた（webhook の数 left 3 / right 1）。原因は attempt 2 の `root_done_summary` と同じで、偽 webhook が要求の前の EOF と他の試験の要求を通知として数えていた。直し方: `POST /hook` だけを数え、他の要求には 404 を返す。記録は応答の前に確定する。回帰として、stray 接続（EOF と `GET /other`）を送ってから数える段を試験に足した。
- 検査:
  - `python3 scripts/dev/check-architecture-map.py`: exit 0（708 パス）。関連試験を消した負例は exit 1。
  - `python3 scripts/tests/test_architecture_map.py`: OK。
  - `cargo nextest run -p celeris --test credential_wait_notify`: 3 回とも 1 passed。
  - `bash scripts/dev/test-affected.sh`: exit 0（4470 passed / 16 skipped、source-size failure 0、地図 708 パス OK）。
  - `cargo clippy --workspace -- -D warnings`: exit 0。
  - `cargo fmt --all -- --check`: exit 0。
- 全 workspace・e2e・browser・userns の試験は走らせていない。

## 再検証（統合後 HEAD 「phase 1: attempt 4 の進捗記録」（rebase 前。件名で参照））

- `bash scripts/dev/test-affected.sh`: exit 0。対象 crate の差分なしのため Rust crate 試験は対象外。health 内の Python 試験 38 件と architecture-map 用試験 1 件が成功し、地図の実在パス 715 件を確認。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `cargo fmt --all -- --check`: exit 0。

## 条件 1 の差し戻し対応（正本 CLAUDE.md の欠落を run 前に検出）

- 差し戻し: `required_rules::preamble` は CLAUDE.md が無いと `Ok("")` を返し、正本を持つ repo で消えても Codex・Pi・ACP は規則なしで起動していた（旧試験 `preamble_is_empty_without_a_rules_source` が固定）。
- 変更（commit「phase 1: 必須規則の正本欠落を run 前に検出する」）: CLAUDE.md が無いとき cwd で `git ls-files --error-unmatch -- CLAUDE.md`（index）と `git cat-file -e HEAD:./CLAUDE.md`（HEAD）を tokio::process で確かめ、どちらかが追跡していれば `required rules source is missing: <path>` の Err。adapter は `required rules delivery failed` で harness の起動前に落ちる。空の前置きは git でない dir と CLAUDE.md を追跡しない repo だけ。壊れた symlink・読めない・空は従来どおり Err。ADR D2 に付記。
- 試験: 旧試験を削除し、`preamble_fails_when_the_tracked_rules_source_is_missing`（worktree で削除／index にだけある正本）と `preamble_is_empty_where_no_rules_source_is_tracked`（git でない dir／CLAUDE.md なしの repo）を追加。`codex`・`pi`・`acp` の各 `*_run_fails_when_tracked_required_rules_are_missing` は `fresh_worktree_with_rules` から CLAUDE.md を消して adapter.run し、Err と stub の marker file 不在を確かめる。
- 検証（基準 HEAD 「phase 1: rebase 後の検証 SHA に進捗記録を直す」（rebase 前。件名で参照） の上の作業ツリー）:
  - `cargo nextest run -p task-worker -E 'test(/required_rules|rules_source|rules_are_unreadable|preamble/)'`: 51 passed。
  - `bash scripts/dev/test-affected.sh`: exit 0（4484 passed / 17 skipped、source-size failure 0、地図 715 パス OK）。
  - `cargo clippy --workspace -- -D warnings`: exit 0。
  - `cargo fmt --all -- --check`: exit 0。

## main で既に解消済みだった項目

- なし。13d64acd でも CLAUDE.md に symlink の説明と「Web UI・リモート実行・予算管理の禁止」が残り、Codex 経路は CLAUDE.md を読んでいなかった（上の実測表）。

## 残件・提案

- 実 CLI（Codex・Pi・opencode）での実機確認はしていない（LLM 呼び出しを伴う）。stub が受け取った prompt の内容までを試験で確かめた。
- Codex の skills 一覧（`skills::deliver_agents_md`、ADR-0127）は従来どおり AGENTS.md の `celeris:skills` 節を書く。そのため skill がある Codex run だけは、AGENTS.md を追跡する repo で作業ツリーを変える（main からの既存の挙動）。提案: skills 一覧も Pi・ACP と同じく prompt に載せ、AGENTS.md を書かないようにする（別 task）。
- 対象外: aider など上の 4 経路以外の adapter には前置きを足していない。


## progress-metadata-repair（attempt 3）

- 根拠: `bash scripts/dev/progress-index.sh --check` が示した既存8文書。title は各本文の `# ` 見出し、tasks は既存 front matter または参照 ADR / 同案件の task 記録、status と updated は各文書の `completed` 記録を根拠に補完した。既存の正しい欄・本文・実装・検査スクリプトは変更していない。
- 変更範囲: `agent-docs/progress/2026-10-21-{cargo-lock-regenerate,grader-input-and-max-turns,preflight-env-lock-check}.md` と `agent-docs/progress/2026-10-22-{comment-on-human-gate,dynamic-config,harness-balance-lane-routing,jev-classification-before-launch,plan-issue-answer-replans}.md` の front matter のみ。
- 基準 main: `f01e41e9bf43`。検証対象 SHA: 「phase 1: 必須規則の正本欠落を run 前に検出する」（rebase 前。件名で参照）。
- `bash scripts/dev/progress-index.sh --check`: exit 0。
- `python3 scripts/dev/check-architecture-map.py`: exit 0（715 path）。`bash scripts/dev/check-doc-links.sh docs/architecture-map.md`: exit 0。
- `bash scripts/dev/test-affected.sh`: exit 0（health の 38 件と architecture-map の 1 件が成功。Rust 対象試験は docs のみのため対象なし）。
- 残件: なし。

## rules-missing-detect（run 01M5MB8PHA0H745H3JEJB6F73A 最終確認）

- 検証対象 SHA: 「docs: fill existing progress metadata from records」（rebase 前。件名で参照）。WU 基準: 「WU の範囲基点（WU tracked scope base、rebase 前）」（rebase 前。件名で参照）、main との merge-base: `a3933dc2222d`（main の祖先）。
- commit「phase 1: 必須規則の正本欠落を run 前に検出する」の実装・試験・ADR D2 付記が既に存在することを確認した。追跡済み正本の欠落は Err、規則のない repo / 非 git dir は空の前置き。Codex・Pi・ACP の adapter.run 試験はエラーと stub 起動 marker の不在を確認する。今回、実装の追加変更はない。
- 地図は TaskStore/events、Dispatcher、worker adapter、task-api、daemon、web を含む全節が同じ6列で、関連試験と ADR を根拠とする不変条件を持つ。`python3 scripts/dev/check-architecture-map.py`: exit 0（715 パス）。追加修正なし。
- `bash scripts/dev/test-affected.sh`: exit 0（4484 passed / 17 skipped。health の38試験と地図の1試験も成功）。渡された WU 基準を変更せず実行した。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `cargo fmt --all -- --check`: exit 0。
- 今回の変更範囲はこの進捗への検証結果追記のみ。ログは run の成果物ディレクトリの rules-missing-final-{test,clippy,fmt}.log。受け入れ条件の残件なし。

## rules-missing-verify（run 01M5MDEMJKBZJEGPP6E5TDZJTF）

- 条件 1（欠落の検出）の根拠: commit「phase 1: 必須規則の正本欠落を run 前に検出する」の `required_rules::preamble` が、作業ツリーに CLAUDE.md が無く git（index または HEAD）が追跡しているとき `required rules source is missing` の Err を返す。Codex・Pi・ACP の adapter は `required rules delivery failed` で harness 起動前に落ちる。実装は今回変えていない。
- 試験は既にあった（`fresh_worktree_with_rules` で git に commit した CLAUDE.md を作業ツリーから消し、adapter.run の Err と stub の started 印 file 不在を確かめる）。ただし受け入れのコマンド `cargo test -p task-worker --lib required_rules` の filter に当たらなかったので、名前を `codex_run_fails_when_tracked_required_rules_are_missing`・`pi_run_fails_when_tracked_required_rules_are_missing`・`acp_run_fails_when_tracked_required_rules_are_missing` に改め、地図・ADR・この進捗の参照も直した（地図は 6 列のまま）。
- 検証対象: 基準 HEAD「phase 1: 正本欠落検出の最終検証を記録する」（rebase 前。件名で参照）の上の試験名変更 commit（「phase 1: 正本欠落の adapter 試験を required_rules の filter に載せる」）。main との merge-base `a3933dc2222d`。
  - `cargo test -p task-worker --lib required_rules`: exit 0（16 passed。既存 13 本と新しい名前の 3 本）。
  - `bash scripts/dev/test-affected.sh`: exit 0（4484 passed / 17 skipped、地図 715 パス OK）。
  - `cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。
  - 文書検査: `check-architecture-map.py`・`check-doc-links.sh docs/architecture-map.md`・`progress-index.sh --check`・`check-doc-layout.sh scripts/dev/docs-layout.tsv`・`check-adr-numbers.sh` がすべて exit 0。
- 残件: なし（上の「残件・提案」の Codex skills 一覧の件は別 task）。

## main-sync-3（main への同期、run 01M5MEXJZACBKJ4AECZ7W398RM）

- rebase 先: main の「docs: record verify smoke stall diagnosis and main fix merge」（作業中に main が「docs: replan 上限の ADR と進捗を agent-docs へ移す」から進んだので 2 度 rebase した。2 度目は進捗 2 file だけの差で衝突なし）。
- 衝突と解き方:
  - `crates/celeris/tests/root_done_summary.rs`・`crates/celeris/tests/credential_wait_notify.rs`・`crates/task-worker/src/browser_egress/tests.rs`: main 側（偽 server の迷い込み接続を数えない共通 reader、echo listener の補修）を採った。branch の同種補修は main と重複するので落とした（この 3 file は main との差分 0）。
  - `docs/architecture-map.md`: branch の 6 列表を採り、main が変えた cell（実行計画行の entry point に `plan_limits` を追加）を写した。両側の先頭 4 列を python で切り出して比べ、main の 125 行がすべて HEAD に残ることを確かめた（欠落 0）。main が新しく足した行は無かった。
  - agent-docs/progress の他 task の 7 file（と無衝突だった 1 file）: main 版を採り、この branch の差分から外した。main との差分に残る進捗はこの file だけ。
  - `crates/campaign/tests/ablation_application_support/mod.rs` の port 衝突補修は main に無いので残した。実装（required_rules.rs と adapter）は変えていない。
- 同期後 HEAD での検査（全 workspace・browser/userns・release 手順は回していない）:
  - `git merge-base --is-ancestor main HEAD`: exit 0。衝突 marker なし。
  - `cargo test -p task-worker --lib required_rules`: exit 0（16 passed）。
  - `bash scripts/dev/test-affected.sh`: exit 0（6379 passed / 18 skipped、地図 716 パス OK）。
  - `cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。
  - 文書検査: `check-architecture-map.py`・`check-doc-links.sh`・`progress-index.sh --check`・`check-doc-layout.sh scripts/dev/docs-layout.tsv`・`check-adr-numbers.sh`・`scripts/tests/test_architecture_map.py` がすべて exit 0。
- 残件: なし（Codex skills 一覧の AGENTS.md 書き込みは別 task）。

## final-sync（main-sync-4、run 01M5MH82QB4B3DFFBY25ZE4CPV）

- 前回の提出 HEAD「Merge branch 'main' into celeris/01M5KWPP3YPZMZ71PZP6HAGNFD」は main の必須検査 task（task-dispatch・delivery など 30 file）を取り込んだ merge だった。この節でその merge を含む HEAD を検証した。
- 同期: 最新 main `d637b1caa30e`（「phase 1: progress-hashfree — 提出時の HEAD の再検査の結果を記録する」）を no-ff merge した（commit「Merge main into celeris/01M5KWPP3YPZMZ71PZP6HAGNFD (K4 map row)」）。既存 commit の hash を保つため rebase はしていない。
- 衝突: `docs/architecture-map.md` だけ。branch の 6 列表を採り、main が 4 列で足した K4 セル有効性採点の行を 6 列にした（関連試験 `crates/campaign/tests/k4_cells_validity.rs` ほか、不変条件と根拠 ADR 2026-10-23 k4-cells-validity-grading）。両側の先頭 4 列を比べ、main の新しい行はこの 1 行だけで、ほかの行は欠けていないことを確かめた。実装は変えていない。
- 進捗の commit 参照: rebase 前の hash 8 個（HEAD の祖先でないもの）を件名に置き換えた。残した hash は HEAD の祖先である main の commit だけ。
- 検証対象: この節を追記した commit「phase 1: main-sync-4 — K4 地図行を 6 列で取り込み、進捗の旧 hash を件名にする」（main `d637b1caa30e` を祖先に持つ）。追記前の merge 直後の作業ツリーで実行した（差はこの節の追記だけ）:
  - `git merge-base --is-ancestor main HEAD`: exit 0。
  - `bash scripts/dev/test-affected.sh`: exit 0（2643 passed / 7 skipped、地図 726 パス OK）。
  - `cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。
  - `cargo test -p task-worker --lib required_rules`: exit 0（16 passed）。
  - 文書検査: `check-architecture-map.py`・`check-doc-links.sh docs/architecture-map.md`・`progress-index.sh --check`・`check-doc-layout.sh scripts/dev/docs-layout.tsv`・`check-adr-numbers.sh`・`scripts/tests/test_architecture_map.py` がすべて exit 0。
- 残件: なし（Codex skills 一覧の AGENTS.md 書き込みは別 task）。
