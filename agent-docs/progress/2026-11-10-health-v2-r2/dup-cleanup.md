---
title: 健全性v2 R2 unit dup-cleanup — B0 が R2 に割り当てた重複・未参照の整理と userns の内側 UID の定数の一本化
tasks: [01M6TYVJT0Z1CBGNK8VTNK8ZJH]
status: running
updated: 2026-11-11
---
# 健全性v2 R2 unit dup-cleanup — 重複・未参照の整理と userns の内側 UID の定数の一本化

task 01M6TYVJT0Z1CBGNK8VTNK8ZJH の WorkUnit dup-cleanup の記録。基準は `$CELERIS_WU_BASE`。旧 unit dups（範囲検査の開始 snapshot に並行の変更が混じったため superseded）を置き換える。dups の branch の commit は main にも統合先にも入っていないので、その commit は使わずに今の tree から書き直した（dups は定数を `browser_runtime.rs` に置き、`userns.rs`・`backend.rs` の 1000 と `CredentialUseResult` の struct を残していた）。

## 変更

- `phase 1: keep the userns inner UID in one const (browser_launcher::userns::INNER_ID) and drop unreferenced close_and_join, normalize_host_pattern, CredentialUseResult`
  - `crates/task-worker/src/browser_launcher/userns.rs` に `pub const INNER_ID: u32 = 1000;` を置いた（2 map を作る module）。`Mapping::helper_args` の内側 ID は `INNER_ID.to_string()`、`Mapping::expected` は `format!("0 {} 1\n{INNER_ID} {} 1\n", ..)`、holder の `setresgid` / `setresuid` は `INNER_ID`。
  - `browser_launcher/backend.rs` の `maps_match` の期待する map は `vec![userns::INNER_ID, sub_id, 1]`。
  - `browser_runtime.rs`: `SANDBOX_UID`（pub・未参照）と private な `INNER_ID` を消し、`use crate::browser_launcher::userns::INNER_ID`。bwrap の `--uid` / `--gid` は `INNER_ID.to_string()`。spawn の子の `setresgid` / `setresuid` は元から `INNER_ID` を使っている。
  - `browser_launcher_run.rs`: private な `INNER_ID` を消し、同じ定数を use（`runtime_facts` の `outside_of(&uid_map, INNER_ID)`）。
  - 値（1000）と map の形は変えていない。試験の入力の値（`userns.rs` の tests の `"1000"` など）は変えていない。
  - 未参照の削除: `browser_live.rs` の `FrameRelay::close_and_join`、task-core `browser.rs` の `normalize_host_pattern`、`celeris-credentiald/src/lib.rs` の `CredentialUseResult`（struct と唯一の impl `from_result`）。
- `phase 2: record dup-cleanup` — R2 の ADR（`agent-docs/adr/2026-11-10-worker-capability-ports.md`）に付記「2026-11-11、unit dup-cleanup」を足し、この進捗を書いた。

## 消す前の参照の確認

`grep -rn "close_and_join\|normalize_host_pattern\|CredentialUseResult" crates web control-ts scripts` は定義の 3 行だけを返した（`crates/celeris-credentiald/src/lib.rs` の struct と impl、`crates/task-core/src/browser.rs`、`crates/task-worker/src/browser_live.rs`）。参照があって消さなかったものは無い。`SANDBOX_UID` は `crates` の中で定義の 1 行だけだった。

文書の中の名前は残した（書き換えない）:
- `agent-docs/adr/0078-browser-execution-capability.md` は `CredentialUseResult` を LLM に返す結果の interface 名として挙げる。実装の中で作る所も返す所も無かったので、未参照として消した。ADR-0078 は書き換えず、R2 の ADR の付記に書いた。credential broker の挙動は変わらない（呼ばれていない型と関数だけを消した）。
- `agent-docs/adr/2026-10-05-browser-department-web-live-view.md` は `normalize_host_pattern` を当時の作成時の検証として挙げる（歴史的記録）。
- `agent-docs/progress/2026-11-07-health-v2-b0/dup-candidates-build.py` は `SANDBOX_UID` を B0 の候補として挙げる（B0 の記録）。

## 受け入れ条件と証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

| 条件 | コマンド | 結果 |
|---|---|---|
| 0. `const INNER_ID` が 1 つ、`SANDBOX_UID`・`setres[gu]id(1000` が無い | `grep -rn "const INNER_ID" crates/task-worker/src \| wc -l`、`grep -rn "SANDBOX_UID" crates/task-worker/src`、`grep -rnE "setres[gu]id\(1000" crates/task-worker/src` | 1 件・0 件・0 件 |
| 1. bwrap_args・helper_args・expected()・backend.rs の期待する map が同じ定数を使い、値は 1000 | `grep -n "INNER_ID" crates/task-worker/src/browser_runtime.rs crates/task-worker/src/browser_launcher/userns.rs crates/task-worker/src/browser_launcher/backend.rs` | 下の「影響試験」の `bwrap_args`・`userns` の試験が通る（値は 1000 のまま） |
| 2. close_and_join・normalize_host_pattern・CredentialUseResult が消えた | `grep -rn "close_and_join\|normalize_host_pattern\|CredentialUseResult\|fn from_result" crates` | 0 件 |
| 3. ADR の付記と進捗 | この file と ADR の付記 | 書いた |
| 4. test-affected・clippy・fmt | 下の「影響試験」 | 下に書く |

## 影響試験

（実行後に書く）

## 未解決事項

- 無し。

## 提案

- 無し。
