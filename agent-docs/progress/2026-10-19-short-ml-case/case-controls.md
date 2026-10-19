---
task: 01M59KY5FTRJTJ80PACNW3K533
wu: case-controls
status: done
completed: 2026-10-19
---
# case-controls: 短い ML case は延期（deferred）

- 結果: `crates/campaign/cases/dev/short-ml-campaign/status.json` に status=deferred、理由 R1〜R3、未検証項目、dev ラベルを記録。採用ではない。実 pilot 確認・final 汎化は主張しない。
- 検査: `python3 crates/campaign/scripts/verify-short-ml-case.py --check` は延期記録の整合（grader.rs sha256 不変、既存 case 集合不変、local に numpy 無し、証拠 path 存在）だけを検証する。adopted は受理しない。
- 根拠: 前段 inventory（2026-10-19）。local に ML 資産・numpy が無く、既存 grader の experiment 判定は scheduler 証拠＋評価側 observed-jobs のみで local 実行を表せない。grader 変更は禁止。
- 副次変更: status.json だけの dir を `CaseSet::load` が読み飛ばす（ADR 2026-10-19-short-ml-case-deferral-marker）。これが無いと cases_load の 8 試験が落ちた。この 1 箇所が「Rust を変更しない」に触れるが、指定 path を満たすための最小変更。不適なら revert して status.json を別 path に置く。
- 主実験への影響: なし。K2/K4・既存 case/manifest/grader 不変。

## 提案
- sirius の batch job で scheduler 証拠が得られる stdlib-only ML case を別 task で検討（新規資料が要る）。

## 証拠
- `python3 crates/campaign/scripts/verify-short-ml-case.py --check` → exit 0（"ok: deferred record is consistent"）
- `cargo test -p celeris-campaign --test cases_load` → 12 passed（新規 deferral_marker_dir_is_not_a_case 含む）
- `bash scripts/dev/test-parallel.sh` → passed 5816 / failed 1（a_program_pin_and_watch の watch_stop_and_restart…。全体並列負荷下のみで落ち、単独で 2 回 pass。本変更と無関係の時間依存 flaky）
- `cargo clippy --workspace -- -D warnings` exit 0 / `cargo fmt --all -- --check` exit 0
