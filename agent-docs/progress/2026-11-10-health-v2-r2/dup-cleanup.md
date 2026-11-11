---
title: 健全性v2 R2 unit dup-cleanup — B0 が R2 に割り当てた重複・未参照の整理と userns の内側 UID の定数の一本化
tasks: [01M6TYVJT0Z1CBGNK8VTNK8ZJH]
status: done
updated: 2026-11-11
completed: 2026-11-11
---
# 健全性v2 R2 unit dup-cleanup — 重複・未参照の整理と userns の内側 UID の定数の一本化

完了日: 2026-11-11

task 01M6TYVJT0Z1CBGNK8VTNK8ZJH の WorkUnit dup-cleanup の記録。基準は `$CELERIS_WU_BASE`。旧 unit dups（範囲検査の開始 snapshot に並行の変更が混じったため superseded）を置き換える。dups の branch の commit は main にも統合先にも入っていないので、その commit は使わずに今の tree から書き直した（dups は定数を `browser_runtime.rs` に置き、`userns.rs`・`backend.rs` の 1000 と `CredentialUseResult` の struct を残していた）。

## 変更

- `phase 1: keep the userns inner UID in one const (browser_launcher::userns::INNER_ID) and drop unreferenced close_and_join, normalize_host_pattern, CredentialUseResult` — 定数を `browser_launcher/userns.rs` に置き、未参照の 3 つを消した。定数の置き場所は次の commit で `browser_runtime.rs` に移した。
- `phase 1: own INNER_ID in browser_runtime so the launcher keeps depending on the runtime (no runtime -> launcher edge, health deps.cycle)` — 定数を `browser_runtime.rs` の `pub(crate) const INNER_ID: u32 = 1000;` に移した。
- `phase 2: record dup-cleanup`（と結果を書いた後続の記録の commit） — R2 の ADR に付記「2026-11-11、unit dup-cleanup」を足し、この進捗を書いた。

提出時の HEAD の形:

- `crates/task-worker/src/browser_runtime.rs`: `pub(crate) const INNER_ID: u32 = 1000;`（唯一の定義）。`SANDBOX_UID`（pub・未参照）は消した。bwrap の `--uid` / `--gid` は `INNER_ID.to_string()`。spawn の子の `setresgid` / `setresuid` は元から `INNER_ID`。
- `browser_launcher/userns.rs`: `use crate::browser_runtime::INNER_ID`。`Mapping::helper_args` の内側 ID は `INNER_ID.to_string()`、`Mapping::expected` は `format!("0 {} 1\n{INNER_ID} {} 1\n", ..)`、holder の `setresgid` / `setresuid` は `INNER_ID`。
- `browser_launcher/backend.rs`: `maps_match` の期待する map は `vec![INNER_ID, sub_id, 1]`。
- `browser_launcher_run.rs`: private な `INNER_ID` を消し、`use crate::browser_runtime::INNER_ID`（`runtime_facts` の `outside_of(&uid_map, INNER_ID)`）。
- 値（1000）と map の形は変えていない。試験の入力の値（`userns.rs` の tests の `"1000"` など）は変えていない。
- 未参照の削除: `browser_live.rs` の `FrameRelay::close_and_join`（`#[cfg(test)]`、試験からも呼ばれていない）、task-core `browser.rs` の `normalize_host_pattern`、`celeris-credentiald/src/lib.rs` の `CredentialUseResult`（struct と唯一の impl `from_result`）。

### 定数の置き場所を objective と変えた理由

objective は置き場所を「map を作る browser_launcher（userns.rs か mod.rs）」としていた。最初はそのとおり `browser_launcher/userns.rs` に置いたが、`bash scripts/dev/test-affected.sh` の health が `deps.cycle`（必須）で exit 1 になった（`verdict=fail worsened=1`）。`browser_launcher` は元から `browser_runtime` に依存する（`backend.rs` が `RuntimeSpec`・`EgressRelay`・`UsernsMode`・`listening_tcp` を use）ので、`browser_runtime` が `browser_launcher::userns::INNER_ID` を use すると `browser_runtime → browser_launcher` の辺が増え、base の 6 単位の循環が `browser_launcher`・`browser_runtime` を含む 8 単位に広がった。

定数を `browser_runtime.rs` に置けば、`browser_launcher → browser_runtime` と `browser → browser_runtime` は既にある辺なので、module の依存の辺は増えない（health は `worsened=0`）。意味の上でも、sandbox の内側の ID を決めるのは bwrap を起動する runtime（ADR-0105）で、launcher はそれを subuid へ map する側（ADR-0115）なので、依存の向きに沿って runtime を正本にした。受け入れ条件 0・1 は置き場所を指定していない。health の policy・checker は変えていない。

## 消す前の参照の確認

`grep -rn "close_and_join\|normalize_host_pattern\|CredentialUseResult" crates web control-ts scripts` は定義の 3 行だけを返した（`crates/celeris-credentiald/src/lib.rs` の struct と impl、`crates/task-core/src/browser.rs`、`crates/task-worker/src/browser_live.rs`）。参照があって消さなかったものは無い。`SANDBOX_UID` は `crates` の中で定義の 1 行だけだった。

文書の中の名前は残した（書き換えない）:
- `agent-docs/adr/0078-browser-execution-capability.md` は `CredentialUseResult` を LLM に返す結果の interface 名として挙げる。実装の中で作る所も返す所も無かったので、未参照として消した。ADR-0078 は書き換えず、R2 の ADR の付記に書いた。credential broker の挙動は変わらない（呼ばれていない型と関数だけを消した）。
- `agent-docs/adr/2026-10-05-browser-department-web-live-view.md` は `normalize_host_pattern` を当時の作成時の検証として挙げる（歴史的記録）。
- `agent-docs/progress/2026-11-07-health-v2-b0/dup-candidates-build.py` は `SANDBOX_UID` を B0 の候補として挙げる（B0 の記録）。

## 受け入れ条件と証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

| 条件 | コマンド | 結果 |
|---|---|---|
| 0. `const INNER_ID` が 1 つ、`SANDBOX_UID`・`setres[gu]id(1000` が無い | `grep -rn "const INNER_ID" crates/task-worker/src \| wc -l`、`grep -rn "SANDBOX_UID" crates/task-worker/src \| wc -l`、`grep -rnE "setres[gu]id\(1000" crates/task-worker/src \| wc -l` | 1・0・0（定義は `browser_runtime.rs` の 1 行） |
| 1. bwrap_args・helper_args・expected()・backend.rs の期待する map が同じ定数を使い、値は 1000 | `grep -n "INNER_ID" crates/task-worker/src/browser_runtime.rs crates/task-worker/src/browser_launcher/userns.rs crates/task-worker/src/browser_launcher/backend.rs crates/task-worker/src/browser_launcher_run.rs`、`cargo nextest run -p task-worker --lib -E 'test(/userns\|bwrap\|runtime_facts\|maps_match/)'` | 4 file とも `INNER_ID` を使う。試験 10 件合格（`helper_arguments_and_subordinates` は helper の引数の `"1000"` を、`userns_args_tests` の 3 件は bwrap の引数を、`runtime_facts_follow_the_observation` は map から引く runtime UID を見る。値は 1000 のまま） |
| 2. close_and_join・normalize_host_pattern・CredentialUseResult が消えた | `grep -rn "close_and_join\|normalize_host_pattern\|CredentialUseResult\|fn from_result" crates` | 0 件 |
| 3. ADR の付記と進捗 | `grep -n "CredentialUseResult" agent-docs/adr/2026-11-10-worker-capability-ports.md agent-docs/progress/2026-11-10-health-v2-r2/dup-cleanup.md` | ADR の付記「2026-11-11、unit dup-cleanup」とこの file に書いた |
| 4. test-affected・clippy・fmt | 下の「影響試験」 | 3 つとも exit 0 |

## 影響試験（提出時の HEAD で実行）

- `bash scripts/dev/test-affected.sh` → exit 0。対象は task-core・celeris-credentiald の逆依存を含む 13 crate と web（typecheck+unit）・ts-verify。
  - repo-health: `verdict=pass exit=0 new=0 worsened=0`（`deps.cycle` を含む必須 rule が pass）。
  - nextest: `8197 tests run: 8197 passed (20 slow), 23 skipped`（553 秒）。
  - web unit: `104 passed (104)` file・`712 passed (712)` 試験。ts-verify: `20 passed, 0 failed`。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- 範囲: `sh "$CELERIS_WU_SCOPE_PATHS"` の変更 path は ADR・この進捗と、objective が挙げた 7 file（`crates/celeris-credentiald/src/lib.rs`、`crates/task-core/src/browser.rs`、`crates/task-worker/src/browser_launcher/{backend,userns}.rs`、`crates/task-worker/src/{browser_launcher_run,browser_live,browser_runtime}.rs`）だけ。

## 未解決事項

- 無し。

## 提案

- 無し。
