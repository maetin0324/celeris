---
title: "cap-probe: ro_mounts を種別別に検査"
tasks: [01M58VFR9ZS5RG7QNB558DT4XK]
status: done
completed: 2026-10-19
updated: 2026-10-19
---
# cap-probe: ro_mounts を種別別に検査（WU kind-aware-check）

## 最新 main での未解消確認
base 9567eb1b（= main head）の `cap_probe.rs::worker_spec` は全 ro_mounts に `test -d` を生成していた（未解消）。

## 変更
- `MountKind {File, Dir}` / `ro_mount_kinds(&Toolchain)`（host で canonicalize 後の種別。symlink は pin_mount と同じく実体の種別。dangling・特殊 file は載らない）/ `ro_mount_check`。
- file: `test -f P && test -r P`、dir: `test -d P && test -r P && test -x P`。種別不明（host で解決不能）は dir 検査のまま落ちる＝緩めない。
- `worker_spec(expected, kinds, case, harness)`。唯一の呼び出し元 `probe_worker`（B/C・単独 cap-probe 経路もここ）が `ro_mount_kinds(&env.worker)` を渡す。A/B/C は `CapEnv` が a==worker を強制するので同じ意味。
- 試験 `crates/campaign/tests/cap_probe.rs` に `ro_mount_kind_*` 6 件（正常 file、正常 dir、symlink、欠落、種別不一致、読取不能。root では sh 自身の `test -r` を基準に判定し skip しない）。

## 証拠
- `cargo test -p celeris-campaign --test cap_probe`: 19 passed
- `bash scripts/dev/test-parallel.sh`: exit 0（5816 passed, 0 failed）
- `cargo clippy --workspace -- -D warnings`: exit 0
- `cargo fmt --all -- --check`: exit 0

## 未解決・提案
採用 CAP_ENV の種別一覧と pilot への影響は後続 WU report-kinds の範囲。
