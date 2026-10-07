---
title: 葉 api — task-api の信頼端末の端点（登録・検証と回転・一覧・失効）
tasks: [01M4ADMXWYHPSVEJBJ5JPCR6PS]
status: done
updated: 2026-10-07
completed: 2026-10-07
---
# 葉 api: task-api の信頼端末の端点

## やったこと

- `crates/task-api/src/browser_trusted_devices.rs` を足し、`/api/v1/browser/trusted-devices` の 4 本を router に載せた。
  - `POST` 登録（201）、`POST …/verify` 検証と回転（`readonly: true` は probe 用で何も書かない）、`GET` 一覧、`DELETE …/{id}` 失効。
  - すべて管理系の Bearer と web の Ed25519 assertion を要する。payload は `DeviceClaims`（`purpose` で端点を区別、`deny_unknown_fields`）。
    本文・path の値は claims と一致しなければ 403 `not_owner_session`。
  - 署名の検証は `browser_live::verify` の前半を `verify_signature` に切り出して共有した（鍵・ring の検証は変えない。Live View の挙動も同じ）。
  - GET・DELETE は header `x-celeris-assertion-payload` / `x-celeris-assertion-signature` で assertion を受ける。
  - 応答に hash を返さない。上限 409 `device_limit`、拒否 403 `device_rejected`（理由は区別しない）、不正 422 `device_invalid`、
    未知 id の失効 404 `device_not_found`。
- `ApiState` に注入できる時計 `ApiClock`（UNIX 秒）と `with_clock` を足した。端末の期限と assertion の期限はこの時計で判定する。
- schema（`docs/api/v1/api-v1.schema.json`）を `UPDATE_SCHEMA=1` で再生成し、gui（`pnpm gen:types`）・web（`gen-types.mjs`）の生成型を再生成した。
- `docs/api/v1/gui-api.md` に表の #187〜190 と §3.128 を足した。
- ADR に付記（api 葉の実装突き合わせ）を足した。D2 の `…/resume` は `…/verify`、`…/{id}/revoke` は `DELETE …/{id}` になった（WU の objective に合わせた）。

## 証拠

- `cargo test -p task-api trusted_device`: 新しい 9 本（`tests/trusted_devices.rs`）＋既存 1 本、すべて合格。
  - 登録→検証→回転、旧秘密の再提示で失効、期限切れ（偽の時計）、失効後の拒否、assertion 不正の拒否（別鍵・改ざん・用途違い・owner でない・期限・
    本文との不一致・RelayClaims）、上限 5、一覧に hash が無い、readonly 検証で DB と events が変わらない、誤った秘密の拒否。
- `bash scripts/dev/test-parallel.sh`: exit 0（passed 4320、failed 0、ignored 14）。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。
- gui `pnpm typecheck`: exit 0。

## 未解決事項

- web の `pnpm typecheck` は store 葉の未解決事項（web/api/realtime の EventKind 4 種）のまま。web 葉で足す。
- `gui/docs/celeris-api-v1.md`（`scripts/sync-gui-docs.sh` の写し）はこの葉の前からずれている。範囲外なので触っていない。

## 提案

- なし。
