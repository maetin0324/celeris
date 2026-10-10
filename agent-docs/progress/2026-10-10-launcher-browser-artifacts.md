---
title: "launcher runtime の screenshot・download を run の browser/output に渡す（launcher protocol 8）"
tasks: [01M4JB01XH0GQBTG7PMCK2QTTY]
status: done
updated: 2026-10-10
---

# launcher runtime の browser artifact transfer

- 完了日: 2026-10-10（コード。本番の launcher 再 build・差し替えは operator 作業: `docs/ops/browser-launcher-host-setup.md`
  「launcher protocol 8 への更新」）
- 発端: manaba の課題監視（task 01M4GYJ3XGJNWZQDF35F1MDE0H）で授業資料の PDF を agent が読めなかった。launcher runtime では
  `LauncherExecutor` が screenshot / download を失敗にしていた（ADR 2026-10-09 credential username / post-login 付記 2026-10-09 8）。
- 決定: 同 ADR の付記 2026-10-10e。Live View の frame は v9 に送った（ADR 2026-10-10-browser-launcher-live-view-frames D5）。

## 変更

- `browser_launcher/protocol.rs`: `PROTOCOL_VERSION` 7 → 8、`fetch_artifact` / `artifact`、`ArtifactKind`（先頭 byte で判定）、
  上限（10 MiB/file、32 件、32 KiB chunk）、`ErrorCode::ArtifactRejected`。
- `browser_launcher/server.rs`・`backend.rs`: 自 session が生成した名前だけを `O_NOFOLLOW` で読み、型・長さを検査して返す。
  policy に screenshot / download が無い session は拒否。
- `browser_launcher/client.rs`: chunk の取得。診断用の生応答抜粋は artifact 応答では伏せる。
- `browser_launcher_run.rs`: `LauncherExecutor` が chunk を集め再判定し、shim の名前で `browser/output/` に新規 0600 で書く。
  v8 未満・超過・取消は固定理由（`browser_launcher_protocol_artifacts_required` ほか）で失敗、run に progress `browser.artifact`。
- `browser_cli.py`: 固定理由を agent に返す。download を先頭 byte で `download-<hex>.pdf` 等に hard link し、応答 `file` に返す。
- `browser_action.py`: 成功した screenshot / download の file を 0644（別 UID の launcher が読む）。

## 証拠

| 条件 | コマンド | 結果 |
|---|---|---|
| 新規試験（launcher・artifact） | `TMPDIR=/tmp/… cargo nextest run -p task-worker --lib -E 'test(/launcher\|artifact\|excerpt/)'` | 144 passed |
| 全体 | `TMPDIR=/tmp bash scripts/dev/test-parallel.sh` | exit 0、5035 passed / 0 failed / 14 ignored |
| release gate と同じ | `CELERIS_USERNS_TESTS=1 CELERIS_ISOLATION_TESTS=require TMPDIR=/tmp bash scripts/dev/test-parallel.sh` | exit 0、5035 passed / 0 failed（この run では `unshare -Ur true` が成功） |
| clippy | `cargo clippy --workspace -- -D warnings` | exit 0 |
| fmt | `cargo fmt --all -- --check` | exit 0 |

試験（`crates/task-worker/src/browser_launcher_run_tests.rs` ほか）:

- `launcher_artifacts_reach_the_run_output_through_the_v8_transfer`: 複数 chunk の PDF と PNG が byte 一致で output に届く（0600）。
- `launcher_shim_download_lands_in_the_run_output_as_a_readable_pdf`: 実 shim → action server → 偽 v8 launcher。`file` が
  `download-<hex>.pdf`、events.jsonl に中身・ref が出ない。protocol 7 では shim が `browser_launcher_protocol_artifacts_required`。
- `launcher_artifacts_refuse_types_sizes_counts_and_canceled_downloads`: HTML・ELF・不明・PDF の screenshot・10 MiB 超・取消・件数。
- `launcher_artifacts_fail_closed_with_a_reason_below_protocol_8`: 旧版では launcher に何も届かず file も無い。
- `launcher_serves_only_names_its_session_produced`、`artifact_responses_carry_no_secrets_and_diagnostics_withhold_bytes`、
  `launcher_artifact_reader_is_bounded_typed_and_refuses_symlinks`、`artifact_chunks_are_withheld_from_diagnostics`。

## 未解決事項

- 実 launcher（uid 995・subuid の sandbox）での往復は host で人が確かめる（運用手順の確認 1〜3）。
- launcher 側の 32 件の上限（`RuntimeSession`）は偽 backend の試験では通らない（daemon 側の上限は試験済み）。
- 全体試験は TMPDIR が長いと Unix socket の path 上限（108 byte）で既存の launcher・credentiald 試験が落ちる（この run の既定
  TMPDIR、`/tmp/cl-tp` でも `browser_e2e` 3 件）。`TMPDIR=/tmp` で通る。変更とは無関係。

## 提案

- test-parallel.sh の `$logdir/tmp` の入れ子が socket path を長くする。短い固定 path（例 `/tmp/ctp.XXXX`）にするか、socket を使う
  試験の fixture を `/tmp` 直下の短い dir にそろえる別 task。
