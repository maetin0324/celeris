# /local hot データ移行: 工程 core 統合後の再検査（core-recheck）

対象 task: 01M3Z08A0T81ZQ60XVR62XJMPD（ADR-0136 local-hot-data-layout）。検査日: 2026-10-03。
対象 HEAD: 9581f68a（integrate wu/launcher-skew、paths-config・scripts-paths・units・e2e-stable・launcher-skew 統合済み）。
各コマンドは 1 回だけ流した（流し直しなし・CPU を焼く負荷なし・本番 host 操作なし）。コード変更はしていない。

## 試験ごとの結果

- cargo test --workspace --no-fail-fast: pass（exit 0、3335 passed / 0 failed / 14 ignored）・原因: なし・対応: なし
- cargo clippy --workspace -- -D warnings: pass（exit 0）・原因: なし・対応: なし
- cargo clippy --workspace --all-targets -- -D warnings: pass（exit 0）・原因: なし・対応: なし
- cargo fmt --all -- --check: pass（exit 0）・原因: なし・対応: なし
- cargo test -p celeris config（paths-config の check）: pass（exit 0、98 passed）・原因: なし・対応: なし
- bash scripts/selfdeploy/tests/local_state_layout.sh（scripts-paths の check）: pass（exit 0、"ok local state layout and legacy defaults"）・原因: なし・対応: なし
- bash scripts/selfdeploy/tests/install_units_hot_dir.sh（units の check）: pass（exit 0、"install_units_hot_dir: ok"）・原因: なし・対応: なし
- scripts/selfdeploy/tests/*.sh（全 11 本）: pass（全て exit 0）・原因: なし・対応: なし
- cargo test -p task-worker --test browser_launcher_ptrace（launcher-skew の check）: pass（exit 0、5 passed、launcher_chrome_denies_daemon_uid_ptrace も ok）・原因: 以前の integrate-core 失敗は host launcher の版ずれ（Protocol）。人が host を main の launcher に戻し、launcher-skew が版ずれを原因つき skip にした・対応: 追加なし
- cargo test -p e2e --test api_scenarios（e2e-stable の check）: pass（exit 0、11 passed）・原因: 以前の失敗は writes_from_celerisctl_and_api_while_celeris_ticks_fast_never_hit_database_is_locked と daemon_view_shows_in_flight_runs_and_cooldowns_and_throttle_is_recorded の負荷 flaky。e2e-stable で出来事待ちに直し済み・対応: 追加なし

## 結論

統合後の HEAD で決定的な失敗も flaky も再現しなかった。integrate-core の 3 回目の失敗（ログなし）は、上記 2 種（launcher 版ずれ・api_scenarios 負荷 flaky）か run sandbox の環境要因と見られ、本 task の変更（crates/celeris の config・hot 配置、scripts/selfdeploy、deploy/systemd、api_scenarios）が原因の失敗は見つからなかった。
