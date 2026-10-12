---
title: WorkUnit の受け入れ検査ログを作業ツリーの外に置く（scope check の自己汚染を防ぐ）
tasks: [01M4NV0KJQ2A1ZY49CTPBQN1C7]
status: done
updated: 2026-10-11
---

# WorkUnit の受け入れ検査ログを作業ツリーの外に置く

verdict: reproduced（WU の検査ログ `work-unit-checks`）／ integration-checks は not-reproduced（shared mode の統合は検査を流さない）。置き場所は 2026-10-11 reopen で `check_log_root` 基準へ揃えた

## 再現（WU repro-fix）

試験（`crates/task-dispatch/src/dispatcher/tests/scope_checks.rs`）。fixture の `.gitignore` は
`.taskd/ runs/ inputs/ artifacts/` だけで、`work-unit-checks/` と `integration-checks/` は入れない。

- `wu_check_log_outside_tree_shared_mode_scope_check_ignores_check_logs` — shared mode（task_dir = repo の作業ツリー）。
  WU `a`（段 build）と WU `c`（段 next）の範囲 check（`git diff` + `git ls-files --others --exclude-standard`）が
  自分の検査ログも前の WU の検査ログも範囲外と数えないこと、ログが `<workspace_root>/<task_id>/work-unit-checks/<key>` にあり
  作業ツリーに無いことを見る。
- `wu_check_log_outside_tree_second_stage_ignores_integration_check_logs` — worktree mode。1 段目の統合の検査が走って
  ログを残し、2 段目の WU `c` の範囲 check がそれを数えないこと、ログ dir がどの task 作業ツリーにも入っていないことを見る。

修正前（base `130f9f34` のコード + 新しい試験）:

```
$ cargo nextest run -p task-dispatch --no-fail-fast wu_check_log_outside_tree   # exit 100
FAIL wu_check_log_outside_tree_shared_mode_scope_check_ignores_check_logs
  WorkUnitChecksFailed key=a ... stdout_tail="out of scope:
  work-unit-checks/a/1791735294545-0.log
  work-unit-checks/a/1791735294545-1.log"   （retry ごとに増え、3 回とも落ちる）
PASS wu_check_log_outside_tree_second_stage_ignores_integration_check_logs
Summary 2 tests run: 1 passed, 1 failed
```

integration-checks が再現しない理由: 統合のログは `ws.task_dir`（`task_workspaces_for` が常に
`<workspace_root>/<task_id>` を返す）の下で、worktree mode では作業ツリーは `<task_dir>/tree` か `<task_dir>/repos/<name>`
にあるので最初から外。shared mode では `parallel_mode().worktrees` が false なので統合の検査は走らない
（`phase_integration.rs` の `let (true, Some(ws)) = ... else` で即 `IntegrationRun::default()`）。よって置き場所は変えていない。

## 決めた置き場所

葉の WU の受け入れ検査のログ = `<workspace_root>/<task_id>/work-unit-checks/<wu_key>/<started_ms>-<index>.log`
（`Dispatcher::check_log_root(task_id)`、`phase_integration.rs`）。worktree mode ではこれは今の task_dir と同じ場所。
旧い `<task_dir>/work-unit-checks`（shared mode では repo の作業ツリーの中）の経路は消した（互換分岐・切替 flag なし）。
統合の検査のログは `<workspace_root>/<task_id>/integration-checks/<wu_key>` のまま（上のとおり既に外）。

ログを読む側（task-api `GET /tasks/{id}/work-units/{key}/check-log`）は event の絶対 `log_path` を読むので、コードの変更は要らない。

## 証拠（修正後）

- `cargo nextest run -p task-dispatch wu_check_log_outside_tree` → exit 0、2 passed
- `cargo nextest run -p task-dispatch` → exit 0、951 passed / 0 failed
- `cargo clippy -p task-dispatch --all-targets -- -D warnings` → exit 0
- 既存試験の追随: `scope_checks.rs` の shared fixture `.gitignore` から `work-unit-checks/` 行を削除、
  `drain_hand_off.rs` は WU 検査ログが `<root>/<task_id>/work-unit-checks/a` の下にあること、
  `integration_check_progress.rs` は統合ログが `<root>/<task_id>/integration-checks/integrate-build` の下にあることを assert。

## verify（2026-10-11、統合後 HEAD `3a7c2794` 相当）

コードは変えていない。`TMPDIR=/tmp` は browser・credentiald の unix socket path 長超過を避けるため（既定の TMPDIR では落ちる）。

- `TMPDIR=/tmp bash scripts/dev/test-parallel.sh` → exit 0。nextest `Summary [129.243s] 5170 tests run: 5170 passed, 13 skipped`（failed 0）。
  doctest は全 crate `ok`（task_core 3 passed、task_worker 1 ignored、他は 0 件）。`CELERIS_TEST_SUMMARY` の passed 5173 / failed 0 / ignored 14。
  test-parallel は `tmp_leftovers: 7`（試験が TMPDIR に 7 件残し、終了時に消した旨の警告。失敗ではない）。
- `cargo clippy --workspace -- -D warnings` → exit 0（`Finished dev profile`、警告 0）。
- `cargo fmt --all -- --check` → exit 0（差分なし）。
- 文書検査 3 本（`agent-docs/README.md` ADR-0128 D7 の land 系 check）:
  - `sh scripts/dev/check-doc-links.sh` → exit 0、`check-doc-links: ok`
  - `sh scripts/dev/check-adr-numbers.sh` → exit 0、`check-adr-numbers: ok (194 files)`
  - `sh scripts/dev/progress-index.sh --check` → exit 0、`progress-index --check: ok`

## 統合ログの移動（2026-10-11 reopen）

日次整理の reviewer（01M4P79NAZ）の指摘: 統合 WU の `ObservedIntegration.log_dir` が `ws.task_dir.join(INTEGRATION_CHECK_LOG_DIR)`
のままで、shared mode では作業ツリーに入りうる。

- **修正前の試験結果**: `wu_check_log_outside_tree_shared_mode_after_integration`（shared mode、`.gitignore` は
  `ignore_runtime_but_not_check_logs`、段 build の WU a に非範囲 check `test -f a/one.txt`、段 next の WU c に
  `git ls-files --others --exclude-standard | grep '^integration-checks/'` があれば exit 1 の範囲 check）を修正前のコードで
  流すと FAIL した。ただし落ちたのは最初の assert「`integrate-build` の `IntegrationCheckStarted` が出る」で
  （`scope_checks.rs:675` panicked、events は `integrate-build` が `integrate`→`integrated` と遷移するだけで `IntegrationCheck*` が無い）、
  c の範囲 check は通っていた（`WorkUnitChecksFailed` 無し、task は Done）。
- **原因の確認**: shared mode は `parallel_mode` が並列 1（`worktrees: false`）に倒し、`start_integration` は
  merge も検査もせずに `on_integration_finished(.., IntegrationRun::default())` を返す（ADR-0074 D1.2）。統合の検査が走るのは
  worktree mode だけで、そこでは `task_workspaces_for` の `task_dir` = `<workspace_root>/<task_id>` = `check_log_root`。
  つまり指摘の「shared mode で統合ログが作業ツリーに入る」経路は今の HEAD には無く、旧 ADR 付記の「共有 mode では統合検査が走らない」は正しかった。
- **修正**: それでも規則を 1 つにするため、`phase_integration.rs` の統合 WU の `log_dir` を
  `self.check_log_root(task_id).join(INTEGRATION_CHECK_LOG_DIR).join(&integ.key)` にした（旧い `ws.task_dir` 基準は削除、分岐なし）。
  定数 `INTEGRATION_CHECK_LOG_DIR` のコメントも「Task の作業ディレクトリ直下」から `check_log_root` 基準へ直した。
  実際のログの場所は worktree mode で変わらないので、本番への移行は不要。
- **試験**: 上の試験は実際の挙動に合わせて「`integrate-build` が integrated になり、統合の検査は流れず（`IntegrationCheckStarted` 無し）、
  c の厳しい範囲 check が通り、task は Done、repo 作業ツリーに `integration-checks/` が無い」を assert する形にした。
  統合の検査が走る worktree mode でのログの場所（`<workspace_root>/<task_id>/integration-checks/integrate-build`）は既存の
  `wu_check_log_outside_tree_second_stage_ignores_integration_check_logs` が見る。
- **読む側・docs**: `check-log` API（`task-api/src/handlers/task_io.rs`）は event の絶対 `log_path` を読むので変更なし。
  fixture（`task-api/tests/files.rs` は `env.workspace(task)` = `<workspace_root>/<task_id>`、`task-api/src/query/tests.rs`・
  `task-ops/src/view/tests.rs` は event の path 文字列だけ）も新しい場所と矛盾しないので変更なし。`docs/api/v1/api.md` の
  `<task_dir>/integration-checks/` を `<workspace_root>/<task_id>/integration-checks/` に、ADR 2026-10-04-integration-check-progress に
  D2 付記、ADR 2026-10-08-work-unit-scope-snapshot の付記を訂正。`agent-docs/guides/work-unit-checks.md` は既に新しい場所を書いていた。
  `crates/task-worker/src/preamble.rs` はログの置き場所に触れておらず矛盾なし。
- **証拠**:
  - `cargo nextest run -p task-dispatch scope_checks integration_check_progress` → 15 passed。
  - `cargo nextest run -p task-dispatch -p task-api -p task-ops --no-fail-fast` → 2162 中 2154 passed、8 failed。落ちたのは
    browser 試験 8 本（`browser_live_cross` 3・`browser_e2e` 4・`browser_waits` 1）だけで、`bind: path must be shorter than SUN_LEN`
    （worker sandbox の長い TMPDIR）による環境起因。本変更と無関係。
  - `cargo clippy --workspace -- -D warnings` → exit 0。

## 未解決事項

- test-parallel の `tmp_leftovers: 7`。試験が TMPDIR に残すエントリは本 WU の範囲外。別 task で原因を確かめる。
- 既定 TMPDIR（`/local/...` 配下の scratch 等）で browser・credentiald の socket path が長すぎて落ちる件は既存の環境制約。本 WU では直していない。

## readers-docs の確認（2026-10-11）

- `task-api` の `check-log` handler は開始 event の絶対 `log_path` を `read_log_tail` に渡す。task directory 内への制限は
  なく、workspace root の `<task_id>/work-unit-checks/...` を読める。`crates/task-api/tests/files.rs` の WU log fixture
  は `env.workspace(task)`（`<workspace_root>/<task_id>`）を使っており、新しい保存先と一致する。
- `task-ops` の WU check progress view は event の `key`・`index`・`cmd` 等を表示するだけで、ログ path を組み立てない。
  既存試験の `/tmp/...` は view 用ダミー event であり配置依存はない。
- `crates/task-worker/src/preamble.rs` および `claude_code/prompt.rs` scope check guidance に log path の記載はなく、
  新しい配置との矛盾はない。
- API 文書と WU checks guide を新しい保存先に追随させた。ADR 2026-10-08 に置き場所の理由と統合ログを変更しない理由を
  付記した。2026-10-04 の hand-off 記録も新しい保存先に更新した。

## 配送差し戻しの診断（2026-10-11）

対象: 配送 `7a2432a2`（本 WU の HEAD）の release prepare 失敗。読んだだけで、`/local/celeris/state` は変更していない。

- 失敗 step: `web-bundle`（`web: tar failed (release blocked)`、17:40:57）。`ERROR: web bundle failed; no release directory created`。
  その前の step（cargo fmt・test-parallel・clippy・cargo build・web の config・pnpm install/typecheck/test/release）は全て exit 0。
- 差し戻しの根拠のログは取れなかった。`.build/tree/.gate-web-bundle.log` は 17:43 に並走の `0722f841` の prepare が
  同じ build tree で上書きし、今は pnpm install の出力（`Done in 411ms`）だけ。`gzip invalid compressed data` / `Unexpected EOF`
  の文言は確認できていない。`7a2432a2.partial` も残っていない（作られた tarball は見られない）。
- 時系列（`/local/celeris/state/releases/.deliveries/` の 2 本の prepare.log と build tree の時刻）:
  - 17:26:01 `7a2432a2` の prepare 開始（build tree を再利用、`CARGO_TARGET_DIR` は `release-build` の共有 target）。
  - 17:34:07 cargo test 終了 → 17:35:40〜17:40:15 `cargo build --release`（この間も同じ target を使用）。
  - 17:36:45 `0722f841` の prepare が `.lock-release` を「持ち主は selfdeploy のスクリプトでなく fd が子に漏れた」と判定し、
    `.lock-release.leaked-20261011T173645Z` へ退けて新しい lock を取った。`7a2432` の prepare は動いていた（ログが 17:40:57 まで続く）。
  - 17:36:56 `0722` の prepare が「stale の target を 407 files（約 60.7 GB）削除」。`7a2432` の release build の最中に同じ target を消した。
  - 17:37:02〜17:42:37 `0722` の cargo test が同じ build tree で走る（`7a2432` の web 段と重なる）。
  - 17:40:15〜17:40:56 `7a2432` の web 段（pnpm install・typecheck・test・release）→ 17:40:57 web-bundle の tar が失敗。
  - 17:42:42〜17:43:10 `0722` の web 段、`0722f841d06c.partial/web/celeris-web-0.1.0-0722f841d06c.tar.gz` の作成まで成功。
- 判断: 原因は並走の prepare による build tree・target・tarball の取り合いと判断する。根拠は、(a) 2 つの prepare が
  同じ `.build/tree` と `release-build` target を使い、lock の直列化が外れていたこと、(b) `7a2432` の build の最中に
  `0722` の prune が target を消したこと、(c) tar の失敗と `0722` の web 段の書き込みが同じ build tree 上で重なる時間帯にあること。
  ただし (c) は時刻の重なりで、tar の入力が書き換わった事実は確認していない。コードの修正は不要と判断する（web/・scripts/ は触らない）。
  取り直しは新しい commit で行う（同じ ref の再試行でなく、本 WU の commit を新しい sha で出す）。
  確かめるには、並走が無い状態で prepare を 1 本だけ回し、gate log を ref 別に残して tar の失敗が再現するかを見る。
- 本 WU の差分が web/・scripts/selfdeploy/ に触れないこと:
  - `git diff --stat $(git merge-base HEAD main) -- web/ scripts/selfdeploy/` → 出力なし（exit 0）
  - `git diff --stat "${CELERIS_WU_BASE:-HEAD}" -- crates/ web/ scripts/` → 出力なし（`CELERIS_WU_BASE` は `fa4c0619`、HEAD の祖先ではない）
  - `git diff --stat fa4c0619 HEAD` → 出力なし。
  - この WU で書き換えたのは本進捗ファイルだけ（`git status` で確認）。

## 提案

- shared mode では `runs/` `inputs/` `artifacts/` `.taskd/` も task_dir = repo の作業ツリーに書かれ、repo の `.gitignore`
  頼みで範囲 check・`git status` を汚さずにいる（試験 fixture も `.gitignore` で隠している）。検査ログと同じく
  `<workspace_root>/<task_id>` に出すかを別 task で決めるとよい（run の作業ディレクトリ・worker の契約に関わるので ADR が要る）。
- selfdeploy の lock 誤判定（`scripts/selfdeploy/lib.sh` の `sd_lock_leaked_holders` / `sd_lock_or_tempfail`）。
  2026-10-11 17:36:45 の `0722f841` の prepare は、まだ動いている `7a2432a2` の prepare（ログが 17:40:57 まで続く）が
  `.lock-release` を持つ間に、それを「持ち主は selfdeploy のスクリプトでなく fd が子に漏れた」と判定して lock を退けた。
  判定は「`selfdeploy/*.sh` を含む cmdline の生きた holder が 1 つも無い」だけなので、持ち主の script が fd を
  閉じた後や cmdline の形が違うと、生きた prepare を死んだ扱いにする疑いがある。その結果、同じ build tree・target を
  2 本の prepare が使った（上の診断）。後続の selfdeploy 課題として、(a) lock に持ち主の pid を書いて生存を確かめる、
  または (b) 退けるのは持ち主 pid が消えたときだけにする、のどちらかを検討する。再現は、prepare を 1 本走らせ、
  その子に fd を継がせた状態で別の prepare を起こす試験で確かめる（実 host 上でなく、一時の lock path で）。
  本 WU ではコードを変えない（selfdeploy は本 WU の範囲外）。

## 本番への移行

なし（config・DB の変更は無い。旧い置き場所に残った過去のログはそのまま。消すかどうかは人の判断）。

## verify（reopen 後、2026-10-11）

対象 HEAD `fdf71403a489`（統合後、コードは変えていない）。作業ツリーは clean。

- `TMPDIR=/tmp bash scripts/dev/test-parallel.sh` → exit 0。nextest `Summary [109.775s] 5199 tests run: 5199 passed, 13 skipped`、
  failed 0。doctest 全 crate ok。`CELERIS_TEST_SUMMARY` は passed 5202 / failed 0 / ignored 14 / nextest_exit 0 / doctest_exit 0。
- `cargo clippy --workspace -- -D warnings` → exit 0（`Finished dev profile`、警告 0）。
- 文書検査 3 本:
  - `sh scripts/dev/check-doc-links.sh` → exit 0、`check-doc-links: ok`
  - `sh scripts/dev/check-adr-numbers.sh` → exit 0、`check-adr-numbers: ok (196 files)`
  - `sh scripts/dev/progress-index.sh --check` → exit 0、`progress-index --check: ok`

### 既定 TMPDIR で落ちた試行（環境起因。コードの失敗ではない）

- `TMPDIR` を run の作業 dir（`.../runs/01M4P9YTSBP8ME2YW7P323TDV3/tmp`）にした試行: 110 failed / 5089 passed。
  browser・credentiald の unix socket path が `path must be shorter than SUN_LEN` で bind 失敗（158 箇所）。
- `TMPDIR=/local/celeris/data/scratch/tt` にした試行: 34 failed / 5165 passed。SUN_LEN が 2 件、残り 32 件は
  `timed out waiting for fixture readiness`（60s）。同時刻の host load average が 40〜53（24 CPU）。
  socket 長は `test-parallel.sh` が `mktemp -d` を TMPDIR の下に切るので、短い scratch では足りなかった。
- 負荷を待って（load1 が 20 台まで下がった時点）`TMPDIR=/tmp` で取り直したところ全件通った。

## 結論

受け入れ条件 0〜2 を満たす。統合ログの置き場所（`check_log_root` 基準）の変更は既存の試験 2 本を含めて
全体試験に通っている。試験の失敗は環境（socket path 長・shared host の負荷）によるもので、本 WU の変更は関係しない。
