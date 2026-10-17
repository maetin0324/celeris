---
task: 01M54HYGQNYDP2MGXXS7AY7164
wu: campaign
status: done
completed: 2026-10-17
---
# campaign: evidence_mode を POST・record・checker へ
- 実装: manifest.rs（effective_evidence_mode・check_resolved）、celeris_exec.rs（POST 本文・観測）、record.rs、checker.rs。
- 証拠: `cargo test -p celeris-campaign` 全 pass、`cargo clippy -p celeris-campaign --all-targets -- -D warnings` exit 0。
- 検証区分: mock 検証済み（Fake transport）。実経路は e2e unit。
- 本番への移行: なし（manifest の欄は互換）。
