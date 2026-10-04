---
tasks: [01M3YNFVFZG5AFFM0RS05X22ZT]
status: done
updated: 2026-10-02
---

# cleanup-api / protocol: worker-protocol.md を task-worker の protocol.rs と照らして直す

## 処理

| 処理(削除/統合/移動/修正) | 旧パス | 新パス or - | 理由 | 最後の commit |
|---|---|---|---|---|
| 修正 | docs/protocol/worker-protocol.md | - | `crates/task-worker/src/protocol.rs`・`subprocess.rs`・`adapter.rs` と食い違う欄・規則を直した。crates から参照されるので場所はそのまま | eb7392bd |

## 直した点

- 冒頭: Phase 60a・53・v2〜v4・28・30・33・41・35・43・49・52 の経緯の箇条を削り、正本（protocol.rs と schema）、
  `PROTOCOL_VERSION = 4`、追加のみでは版を上げない運用、経緯の参照先 ADR の一覧に縮めた。
- 壊れたリンク `../adr/` を `../../agent-docs/adr/` に直した。`docs/DESIGN.md`・`docs/PROGRESS.md`（P-24・P-53a）・
  「DESIGN 原則」「DESIGN §5.6/§6」への言及を削るか `docs/SPEC.md` に置き換えた。`docs/gui/api.md` §3.70 は
  `../api/v1/gui-api.md` へのリンクにした（節番号は api-list WU の整理で変わるので書かない）。
- §1: 終端に `yielded` / `budget_exhausted` / `wait` を追加。アダプタの一覧から実在しない `dsh` / `openai-compat` を削り、
  実在する `acp` / `aider` / `paperqa` / `local-deep-research` に直した。
- §2: 版の行を `run.protocol = 1` → `4` に。未知 `type` は「JSON として妥当だが WorkerMessage に読めない行は
  `error{retryable:false, "protocol violation: …"}`」（`subprocess.rs`）に合わせた。空行の破棄も追記。
- §3.1: 例の `protocol` を 2 → 4。`protocol` 行の v1〜v4 の経緯を削除。`cargo_target_dir` を追加。
  `standing_rules` の「Phase 26 が埋める。今は常に空」を現状（dispatcher が担当宛て + 全員向けを埋める）に直した。
  `organization[]` に `skills/harnesses/tools` を追加。表に無かった context の欄（`review`・`available_genres`・
  `subject_genre`・`clusters`・`profile`・`mode`・`knowledge`・`active_projects`・`session`・`session_diff`・`skills`・
  `continuation`・`work_unit`・`execution_planner`・`decision_requests`・`browser`/`browser_policy`）を追加。
- §4: WorkerMessage の全 10 variant を `{"type":"<name>",…}` の例つきの表で冒頭に置き、未記載だった `wait` を §4.8 として追加
  （欄は protocol.rs の `Wait`、検証は `parse_wait_request`）。
- §5: 終端の一覧に yielded/budget_exhausted/wait を追加。打ち切り規則を現行（`stop_run`: cancel・割り込みも
  SIGTERM → `kill_grace_secs` → SIGKILL をプロセスグループへ。旧記述の「kill_on_drop で子だけ SIGKILL」は廃止済み）に直した。
- §6.1: heartbeat の説明のメッセージ列挙を WorkerMessage 全体の参照に。
- §7: 手書きの schema 抜粋（`worker-protocol-v1` の古い形）を削り、`worker-protocol.schema.json` などを正とする旨とリンクだけ残した。
- §8: 例の `protocol` 2 → 4、`check` を実際の直列化 `{"type":"command",…}` に、必須の `artifacts_dir` を追加。
- §9: 対象アダプタを claude-code/codex/acp/aider に。結果ファイルの優先順を `question > wait > summary > yield`（`adapter.rs`）に直し、
  `wait` の 2 つの書き方を追記。
- §10: `ReviewRequest` の `decisions/answers/checks` と verdict の `repair` ヒントを追記。DESIGN への言及を削除。

## 証拠コマンドと結果

- `grep -n "PROTOCOL_VERSION: u32" crates/task-worker/src/protocol.rs` → `31:pub const PROTOCOL_VERSION: u32 = 4;`。文書は `PROTOCOL_VERSION = 4`・例は `"protocol":4`。
- variant ごとの `grep -c '{"type":"<v>"' docs/protocol/worker-protocol.md` → progress 6 / comment 2 / delegate 3 / artifact 4 /
  question 3 / done 5 / error 5 / yielded 3 / budget_exhausted 3 / wait 4（全 10 variant が 1 件以上）。
- `grep -cE 'DESIGN\.md|PROGRESS\.md|\]\(\.\./adr/' docs/protocol/worker-protocol.md` → 0。
- 文書の相対リンク全件（`grep -oE '\]\([^)#]+'` を docs/protocol から `test -e`）→ 19 件すべて存在。
- `git diff --stat` は docs/protocol/worker-protocol.md と本記録だけ（crates/・web/・gui/・scripts/・schema は無変更）。
- 文書だけの変更なので cargo test / clippy は走らせていない（コードに差分なし）。

## 未解決

- §3.1 末尾の「v4 の前置き」の段落と §9 の各節には Phase 番号つきの経緯の文が残っている（内容は現行と食い違わないので据え置いた）。
- §9 の前置きの順序・プロンプト文面（`preamble::render` の節の並び）は前置きの実装を全件照合していない。
- §10.1 の `Plan` run（`plan.json`）は旧来の計画経路。実行計画（`execution-plan.schema.json`）の planner 出力の説明はこの文書に無い。

## 提案

- §10 に実行計画（`celeris.execution-plan/3`）の planner 出力を 1 節足すか、該当 ADR（0072/0079）への参照に寄せる。
- §3.1 の前置きの段落と §9 の経緯の文は、次の整理で ADR 参照に縮める。
