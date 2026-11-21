---
title: userns の試験 7・9 の移行用の分岐 CELERIS_SECOND_STAGE_TESTS（ADR G7）を撤去する
tasks: [01M7ZYFB4YSJHWR7SF97G501S8]
status: done
updated: 2026-11-21
---
# userns の試験 7・9 の移行用の分岐（ADR G7）の撤去

完了日: 2026-11-21。基準は `$CELERIS_WU_BASE`（WorkUnit）・`git merge-base HEAD main`（task）。

## 結論
G7 の撤去の条件は満たされた（ADR G7 の節に operator の確認を記録）。試験 7・9 は G2・G3 の形に戻り、strict の env を読まない。
release.sh は env を立てない。gate.json の `gate_env` の `CELERIS_SECOND_STAGE_TESTS` の欄は**消した**（理由は ADR G7。値が常に `require` で、env 自体が無くなるため）。
G4・G5 の外し方と `host_tests` の記録は残した。

## WorkUnit の要約
- cred-test（[cred-test.md](2026-11-21-userns-gate-g7-removal/cred-test.md)）: `injection_peer_uid.rs` の `second_stage_strict()`・`LAUNCHER_ENV` の読み取り・
  旧挙動の試験 `test9_keeps_old_behaviour_without_the_gate_env_under_no_new_privs` を消した。env 無しの子で NoNewPrivs 1 の FAIL を見る試験を足した。
  `cargo test -p celeris-credentiald --test injection_peer_uid` は 7 passed・2 ignored。失敗注入（panic を return 相当に変更）で 3 件が FAILED。
- worker-test（[worker-test.md](2026-11-21-userns-gate-g7-removal/worker-test.md)）: `db_guard_tests.rs` の `second_stage_strict()`・旧挙動の試験
  `ssh_test_keeps_old_behaviour_without_the_gate_env` を消した。`cargo test -p task-worker --lib ssh_` は 9 passed・1 ignored。失敗注入で子が PASS して親が FAILED。
- ops-docs（この文書）: 下の変更。

## 変更（ops-docs）
- `scripts/selfdeploy/release.sh`: `export CELERIS_SECOND_STAGE_TESTS=require`・G7 の注記・`gate_env` の欄を消した（`gate_env` は 4 つ）。
- `scripts/selfdeploy/tests/release_host_tests_gate.sh` と `fixtures/release-gate-tree.sh`: runner が見た env を確かめる部分と、`CARGO_ENV_LOG` の口を消した。
  `gate_env` は 4 つの欄だけと完全一致で確かめる（欄が増えれば落ちる）。G4・G5・G6 の確認（filter、`off_by`、`host_tests`）はそのまま。
- `docs/ops/selfdeploy.md`: G7 の説明と `gate_env` の欄の列挙から env を消した。
- ADR `2026-11-21-release-gate-second-stage-skip.md`: G7 を撤去済みと書き直し（経緯・条件の確認・消したもの・欄を消した理由・最初の gate が落ちない見込み）。
  G2・G3・3.5・G6・7 節の注記と I6・I7 を消した。
- 進捗 `2026-11-21-userns-skip-gate.md`（U2）: 本番への移行 5 の command から env を外した。1・2 は当時の手順なので撤去済みの注を足した。

## 撤去を含む最初の release の gate が落ちない見込み
その gate は current（G7 を含む）release.sh で走り、`CELERIS_SECOND_STAGE_TESTS=require` を立て、NoNewPrivs 1 では試験 9 を filter で外す。
撤去後の試験 7 は env を読まず、ssh がある host で本体が走って PASS する。env は試験が読まない値になるだけで結果は変わらない。
`release_host_tests_gate.sh` は撤去後の release.sh で exit 0（NoNewPrivs 0 は filter なし、1 は試験 9 だけ外れる）。
その gate.json で `host_tests.sudo.run=false`・`off_by="no_new_privs"`、`host_tests.ssh.run=true` を確かめる（本番への移行 2）。

## 本番への移行（人が実行する手順）
config・DB・unit の変更は無い（`~/.config/celeris`・systemd の unit・`paths.env`・本番 DB は触らない）。release.sh・verify.sh・promote.sh は走らせていない。
1. この変更を含む release の gate.json を読む。`<sha12>` は release の sha12。期待: `gate_env` の key に `CELERIS_SECOND_STAGE_TESTS` が無い（この release 自身の gate は
   current の旧い release.sh で走るので欄が残る。無いのは次の release の gate から）、`host_tests.sudo.run=false`・`off_by="no_new_privs"`、`host_tests.ssh.run=true`。
   `python3 -I -c 'import json,sys; g=json.load(open(sys.argv[1])); print(json.dumps({k:g[k] for k in ("host_tests","gate_env")}, indent=1))' $CELERIS_STATE_DIR/releases/<sha12>/gate.json`
2. 試験 9 の本体は daemon の gate では走らない。operator が login shell（NoNewPrivs 0、sudo 可）で流す。strict の env は要らない（欠けは常に FAIL）。
   sudo が要るので sandbox では走らせていない（未確認）:
   `CELERIS_USERNS_TESTS=1 TMPDIR=/tmp cargo nextest run -p celeris-credentiald --test injection_peer_uid --no-capture --status-level all -E 'test(=real_process_with_another_uid_is_rejected_by_dac_then_peer_uid_mismatch)'`
   期待: 1 passed。FAIL したら 2 段目の前提（sudo・probe の行）が欠けている。

## 証拠
提出時の HEAD で実行（Rust のコードはこの WorkUnit で変えていない。clippy は走らせていない）。
- acceptance 0: `git grep -n CELERIS_SECOND_STAGE_TESTS -- crates scripts docs` → 出力なし、exit 1（0 件）。ADR・進捗の記録は agent-docs 側にある。
- `for t in scripts/selfdeploy/tests/release_*.sh; do bash $t; done` → 12 本すべて exit 0（`release_host_tests_gate: ok` を含む）。
- `bash scripts/dev/test-affected.sh` → exit 0（試験対象の変更なし。repo_health_gate: all checks passed）。
- `sh scripts/dev/progress-index.sh --check` → ok。`sh scripts/dev/check-doc-links.sh` → ok。`cargo fmt --all -- --check` → exit 0。
- 範囲 check（`CELERIS_WU_SCOPE_PATHS`）: 変更 path は ADR・進捗 2 件・`docs/ops/selfdeploy.md`・release.sh・shell の試験 2 件だけ。

## 未解決
- 試験 9 の host 実行（sudo が要る）は operator に任せる。本番への移行 2 の command。
