---
status: done
completed: 2026-10-18
---
# CAP_ENV toolchain pin を内容 hash へ

- 修正: `crates/campaign/src/cap_probe.rs` の `MountPin` を名前一覧 hash から mount 配下の内容 snapshot hash（`content_sha256`、entries、bytes）へ。env-lock は `celeris-env-lock/3`（旧 /2 は resume 拒否、同じ測定 ID の途中で黙って固定し直さない）。
- 固定範囲・symlink・未検査範囲は ADR 2026-10-17-campaign-lock 付記に記載。/usr 全体走査はせず、上限超過は検証不能。
- 回帰試験: `tests/env_lock.rs` の `same_names_but_changed_nested_content_is_refused_and_unreadable_stops`（実 CLI の env-lock --verify）、`unreadable_file_in_mount_stops_pin`。
- 提案: 本番の既存 env-lock/2 は trial data 前なら削除して取り直す。
