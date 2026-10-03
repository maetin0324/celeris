---
title: /local への hot データ移行（ADR-0136）
tasks: [01M3Z08A0T81ZQ60XVR62XJMPD, 01M3ZPGSEK4H50ZQ7XA01S8JNY]
status: done
updated: 2026-10-03
---
# /local への hot データ移行（ADR-0136）

> 旧 `docs/PROGRESS.md`（main 4c354a7f）の節を ADR-0128 D6 に従い sync-main-3（task 01M40D0QW6HX3XEZK3GBCQV5ZM）がここへ移した。本文は元のまま（リンクだけ新配置へ直した）。

WorkUnit ごとの再検査の記録は [2026-10-02-local-hot-data/](2026-10-02-local-hot-data/) の下（旧 `docs/progress/local-hot-data-*.md` から移した）。

## /local への hot データ移行（ADR-0136）

- 完了日: 2026-10-03。task `01M3ZPGSEK4H50ZQ7XA01S8JNY` の verify WorkUnit。
- main を最初に merge し、`crates/celeris/src/config/mod.rs`、launcher 身元確認関連の3ファイル、`scripts/selfdeploy/install-units.sh`、およびこの文書の衝突を統合した。main の SCM_CREDENTIALS 対応・web-LAN unit を維持し、/local 用 storage validation・launcher-skew 診断・hot root レンダリングも保持。
- 検証の証拠と未解決事項を以下に記録する。人が行う本番切り替えは [移行手順書](../../docs/ops/local-hot-data-migration.md) の手順であり、この WorkUnit は本番 host を変更しない。
- 初回の `git merge main` で対象範囲の6ファイルに衝突が出た。config、launcher client/test、`install-units.sh` は両側の機能を保持して解消し、`docs/PROGRESS.md` は既存の節を併合した。
- latest main (`3527c8e3`) を merge し、merge commit `6778b791` で確定した。`git merge-base --is-ancestor main HEAD` は exit 0。
- 検証結果（2026-10-03）:
  - `cargo fmt --all -- --check` → exit 0。
  - `cargo clippy --workspace -- -D warnings` → exit 0。
  - `for t in scripts/selfdeploy/tests/*.sh; do bash "$t" || exit 1; done` → exit 0。hot-root unit・移行 script dry-run/rollback・release/verify/web-follow を含む全 suite が pass。
  - `cargo test --workspace` → exit 101。`crates/celeris/tests/instance_handoff.rs` の5件が失敗。うち3件は db guard の user namespace probe が `Operation not permitted`。残り2件 `a_newer_release_takes_over_while_the_old_one_finishes_its_run` と `a_stale_heartbeat_promotes_the_standby` は引継ぎを観測できず timeout。
  - `cargo test -p celeris --test instance_handoff` → exit 101。同じ5件を単独再実行でも再現: 3件 (`verify_mode_never_dispatches_and_never_touches_daemon_instances`, `normal_mode_does_not_inject_the_smoke_builtins`, `starting_the_same_release_twice_exits_three`) は db guard user namespace probe が `Operation not permitted`、2件 (`a_newer_release_takes_over_while_the_old_one_finishes_its_run`, `a_stale_heartbeat_promotes_the_standby`) は handoff/standby 観測 timeout。コード修正は行っていない。
- 指定の差分範囲 check `test -z "$(git diff --name-only $(git merge-base HEAD main) | grep -vE '^(crates/|scripts/selfdeploy/|deploy/systemd/|config/|docs/|tests/e2e/tests/api_scenarios.rs)')"` → exit 1。許可外に gui/web と `tests/e2e/tests/phase7_scenarios.rs` があり、先行工程の差分をこの verify WorkUnit が安全に取り除けない。
- 未解決事項: user namespace 制約が解消された環境で `instance_handoff` を再検証すること。2件の handoff timeout はこの sandbox で単体でも再現した環境要因として次の review で判定が必要。browser ptrace 実試験 `cargo test -p task-worker --test browser_launcher_ptrace launcher_chrome_denies_daemon_uid_ptrace -- --nocapture` は exit 101（Chrome PID を20秒以内に確認できず、起動 launcher responder uid=65534）。人が host launcher を更新して同試験を再検証する必要がある。

### verify WorkUnit（2026-10-03、main 3527c8e39ee2）

- `git merge-base --is-ancestor 3527c8e39ee2c51b646e8aa6ddb61e63c55587c7 HEAD` → exit 0。作業ツリーの `gui/`・`web/`・`tests/e2e/tests/phase7_scenarios.rs` は同 main と一致する。過去の誤解決で欠けた main ファイルも復元した。
- `cargo fmt --all -- --check` → exit 0。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `for t in scripts/selfdeploy/tests/*.sh; do bash "$t" || exit 1; done` → exit 0。migration dry-run/rollback を含む全 scripts/selfdeploy suite が pass。
- `cargo test --workspace --no-fail-fast` → exit 101（25 test targets）。`instance_handoff` は user namespace probe の `Operation not permitted` 3件と引継ぎ観測 timeout 2件。`browser_launcher_ptrace::launcher_chrome_denies_daemon_uid_ptrace` は responder uid=65534、Chrome PID 未観測。これらは指定どおり sandbox 制約として扱い、修正していない。
- 同 workspace 実行では e2e daemon 起動試験も worker db guard の user namespace `Operation not permitted` で失敗し、実 browser 試験の複数箇所が `unshare: Operation not permitted` で失敗した。sandbox 制約によるものとしてコード変更なし。負荷 flaky と判断できる単独失敗はこの実行で切り分けられなかった。
- 範囲 check（`git diff --quiet 3527c8e39ee2c51b646e8aa6ddb61e63c55587c7 -- gui web tests/e2e/tests/phase7_scenarios.rs` と staged tree の同等 check）→ 作業ツリーは exit 0。範囲外の復元は main と一致させた。
- 本番 host は変更していない。本番切り替えは [移行手順書](../../docs/ops/local-hot-data-migration.md) に沿って人が実施する。

### sync-close WorkUnit（2026-10-03、main ea2d9d3252816d01030143f631d3676d9f2d4c2c）

- `main` を `--no-ff` で取り込んだ。`docs/PROGRESS.md` は両側の節を保持し、`install-units.sh` は /local の unit 描画と main の sccache unit 撤去を合わせた。`api_scenarios.rs` の重複した待機処理は main の進捗待ちを採用した。`docs/progress/local-hot-data-verify.md` の `merged-main` を取り込んだ完全 SHA に更新した。
- `cargo fmt --all -- --check` → exit 0。`cargo clippy --workspace --all-targets -- -D warnings` → exit 0。
- `for t in scripts/selfdeploy/tests/*.sh; do bash "$t" || exit 1; done` → exit 0。main で撤去された sccache unit を `install_units_hot_dir.sh` が期待しないよう修正した。drop-in ディレクトリを含む migration 試験も通過した。
- `cargo test --workspace --no-fail-fast` → exit 101。ログの集計は 3418 passed / 53 failed / 13 ignored（helper の試験結果行を含む）、20 test targets が失敗。`instance_handoff` は 8 passed、`api_scenarios` は 11 passed。失敗は e2e daemon の db guard と実 browser / launcher の user namespace がこの sandbox で `Operation not permitted` になる既知の環境制約に集中した。範囲内の試験失敗は確認されなかった。負荷 flaky を理由とする変更はしていない。
- `git diff --name-only ea2d9d3252816d01030143f631d3676d9f2d4c2c -- . ':(exclude)crates/**' ':(exclude)scripts/selfdeploy/**' ':(exclude)deploy/systemd/**' ':(exclude)config/**' ':(exclude)docs/**' ':(exclude)tests/e2e/tests/api_scenarios.rs'` → 出力なし。task が触らない path は merged-main と同一。本番 host への操作はしていない。
### sync-final WorkUnit（2026-10-03、main 40189604024aebe248f603b61dcaffb6ee58dc78）

- `git merge --no-ff main` で最新 main を取り込んだ。衝突した `docs/PROGRESS.md` は両側の記録を保持した。`docs/progress/local-hot-data-verify.md` の `merged-main` をこの完全 SHA に更新した。`git merge-base --is-ancestor main HEAD` → exit 0。範囲外パスを同 SHA と照合した `git diff --exit-code 40189604024aebe248f603b61dcaffb6ee58dc78 -- . ':(exclude)crates/**' ':(exclude)scripts/selfdeploy/**' ':(exclude)deploy/systemd/**' ':(exclude)config/**' ':(exclude)docs/**' ':(exclude)tests/e2e/tests/api_scenarios.rs'` → exit 0。
- `cargo fmt --all -- --check` → exit 0。`cargo clippy --workspace --all-targets -- -D warnings` → exit 0。
- `cargo test --workspace --no-fail-fast` → exit 101。ログの `test result` 126 行の合計は 3428 passed / 53 failed / 13 ignored（内部 helper の結果行も含む）。cargo は 20 test targets failed と報告。失敗はこの run sandbox で user namespace の生成が `Operation not permitted` となる worker DB guard・実 browser 試験、および launcher 実セッション試験に集中した。`instance_handoff` はこの実行では 8 passed。範囲内のコード変更を要する失敗は確認されなかった。
- `for t in scripts/selfdeploy/tests/*.sh; do bash "$t" || exit 1; done` と同じ各 script の個別実行 → 15 本すべて exit 0。`migrate_to_local_test.sh` も exit 0。switch の途中失敗時には `switch_failed` trap が自動で旧状態へ戻す。失敗注入 2 ケース（`install-units.sh` 失敗、delta 後の新 tree 欠損）で `config.toml`・`paths.env`・symlink・DB の復元を確認した。unit の drop-in ディレクトリも復元される。
- 本番 host の操作はしていない。全試験と selfdeploy 試験のログはこの run の成果物ディレクトリに保存した。

main ea2d9d32 取り込み、selfdeploy 試験全 pass（work unit `sync-ea2d`）。
