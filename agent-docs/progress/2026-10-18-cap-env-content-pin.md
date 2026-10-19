---
title: CAP_ENV toolchain pin を内容 hash へ
tasks: [01M57MBMKWCDA66S3RTDZX35ZB]
status: done
completed: 2026-10-18
updated: 2026-10-18
---
# CAP_ENV toolchain pin を内容 hash へ

- 修正: `crates/campaign/src/cap_probe.rs` の `MountPin` を名前一覧 hash から mount 配下の内容 snapshot hash（`content_sha256`、entries、bytes）へ。env-lock は `celeris-env-lock/3`（旧 /2 は resume 拒否、同じ測定 ID の途中で黙って固定し直さない）。
- 固定範囲・symlink・未検査範囲は ADR 2026-10-17-campaign-lock 付記に記載。/usr 全体走査はせず、上限超過は検証不能。
- 回帰試験: `tests/env_lock.rs` の `same_names_but_changed_nested_content_is_refused_and_unreadable_stops`（実 CLI の env-lock --verify）、`unreadable_file_in_mount_stops_pin`。
- 提案: 本番の既存 env-lock/2 は trial data 前なら削除して取り直す。

## 追加（差し戻し対応）: launcher / watch 経由の回帰試験
- `crates/campaign/tests/launcher.rs::a_toolchain_content_change_under_the_same_names_is_rejected_on_resume`:
  実 launcher script・実 CLI・実 daemon 役で測定を完走させた後、cap-env の ro_mounts 配下の同名 file（cc）の内容だけを変える。
  probe（stub）は合格のまま、`watch` は env-lock で拒否され、A 起動 0・task POST 0・台帳不変・env-lock.json 不変。内容を戻すと再開できる。
  `chmod 000`（非 root のみ）の読取不能も `watch` が env-lock で止まる。
