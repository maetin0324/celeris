# CoS config と harness の写像

ADR 2026-10-05-cos-chat-home D2 の `[cos]` と `[cos.triage]` を `CosConfig` に追加した。`max_cos_runs` は既存の `[execution] max_cos_runs`（既定 2、0 は通常枠）をそのまま使い、`[cos]` に重複させない。D4 の retention と attachments は維持する。

ロード時に明示の harness/source/provider/account/model の矛盾と不正値を拒否する。候補が無いだけならロードを通し、`resolve_cos_provider` が理由付き unavailable を返す。これは provider の静的候補を選ぶ純関数であり、quota・cooldown・sticky account・tier のモデル解決は後段 chat-run が担当する。reload は `Config::load` の検証が通ってから新しい `cos` を適用し、不正な新設定では旧設定を保つ。

試験は `cos_chat_config_` で始める。既定値・未知鍵・矛盾・候補無し・tier の利用不可理由・reload の旧値維持と正常更新を固定した。reload の統合試験は範囲 check に合わせて `daemon/admin.rs` に置いた。再試行時の `cargo test -p celeris cos_chat_config_ --lib` は 6 passed、`cargo clippy --workspace -- -D warnings` と `cargo fmt --all -- --check` は exit 0。`CELERIS_TEST_JOBS=2 bash scripts/dev/test-parallel.sh` は 4036 passed、0 failed、13 ignored（nextest 12 skipped と doc 1 ignored）で exit 0。範囲 check も exit 0。

後続の `--all-targets` clippy で `admin.rs` の試験モジュールより後に関数があるとの警告が出たため、試験モジュールをファイル末尾へ移した。`cargo clippy -p celeris -p celerisctl --all-targets -- -D warnings` は exit 0、`cargo test -p celeris cos_chat_config_ --lib` は 6 passed、`cargo fmt --all -- --check` は exit 0。
