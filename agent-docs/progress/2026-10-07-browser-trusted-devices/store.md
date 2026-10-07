---
title: 葉 store — task-core の信頼端末（migration 0057・store・Event 4 種）
tasks: [01M4ADMXWYHPSVEJBJ5JPCR6PS]
status: done
updated: 2026-10-07
completed: 2026-10-07
---
# 葉 store: task-core の信頼端末

## やったこと

- ADR `agent-docs/adr/2026-10-07-browser-trusted-devices.md` を人の決定に合わせて直し、状態を「採用」にした。
  - `device-method`: 今は cookie。表に `method` 欄を持たせ、https 化後に passkey を足せる形にする。
  - `device-policy`: 90 日・使うと延長・絶対上限なし・上限 5 台・使うたびに回転。
  - 親の進捗ファイルの「未解決事項」にある「30 日＋絶対 90 日と読んだ」は、この決定で置き換わった。
- migration `0057_browser_trusted_devices.sql`。表は `browser_trusted_devices`。
  - 全ブランチを走査した。main 系が 0055（cos_run_credentials）と 0056（cos_triage）を使っている。
  - そのため `RESERVED_VERSIONS` に 55 と 56 を足した。main を取り込んだら外す。`SCHEMA_VERSION` は 57。
  - 版数を固定している試験 5 file を 57 に直した。
- 型を `crates/task-core/src/trusted_device.rs` に置いた（ここは純粋で I/O なし）。
  - 定数: TTL 90 日、上限 5、名前 64 文字。
  - `is_expired` と `extended_expiry`。疑似 task `trusted_device_event_task_id()`。
- store を `crates/task-core/src/store/trusted_devices.rs` に置いた。`SqliteStore` の inherent method で、時刻 `now` は引数で渡す。
  - `trusted_device_register`: 上限超過は `Rejected(Limit)`。
  - `trusted_device_verify_and_rotate`: 読み切り→判定→更新→event を 1 つの IMMEDIATE transaction で行う。
    - 現行の hash と一致: 回転・延長し、最終使用を更新する。
    - 回転前の hash と一致: 端末を `reuse` で失効させる。
    - 未知・不一致・失効済み・期限切れ: 拒否する。
  - `trusted_device_verify_readonly`: probe 用で、何も書かない。
  - `trusted_device_list` と `trusted_device_get`: hash は返さない。
  - `trusted_device_revoke`。
- `Event::TrustedDeviceRegistered` / `Used` / `Revoked` / `Rejected` を足した。
  - actor・device id・理由だけを載せ、秘密も hash も載せない。未知の id は `device_id` を載せない。
  - task-api の `EVENT_TYPES` は 73 にし、`event_type_name` も足した。
  - `UPDATE_SCHEMA=1` で `docs/api/v1/{event,api-v1}.schema.json` を再生成した。
  - gui の `pnpm gen:types` と web の `gen-types.mjs` も流した。
  - web/api/realtime の event-kinds・invalidation-map に 4 種を足した（sets は空）。
  - gui-api.md に説明を足した。

## 証拠

- `cargo test -p task-core trusted_device`: 11 passed / 0 failed。
  - 登録・回転・使い回しで失効・期限切れ・失効後拒否・上限・絶対上限・readonly・events に秘密が無い・不正 hash。
- `cargo test -p task-api trusted_device`: event 名の一致の試験 1 本。
- `bash scripts/dev/test-parallel.sh`: exit 0（passed 4311、failed 0、ignored 14）。
  - 初回は `delivery::tests::migration_0042_fills_gaps_in_main_schema_41_database` が落ちた。期待する版数の一覧に 57 を足して直した。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- web `pnpm typecheck`: exit 0。`vitest run api/realtime`: 60 passed。gui `pnpm typecheck`: exit 0。

## 未解決事項

- main を取り込むと 0055・0056 が実在になる。そのとき `RESERVED_VERSIONS` から外し、試験の版数一覧も直す。
- 時刻は UNIX 秒（i64）。API 葉は web へ返すときの形をここに合わせる。

## 提案

- なし。
