---
title: ops-r2 の入力隔離・依頼主体・実験専用 operator session（task 全体のまとめ）
tasks: [01M669QB069GND2ZKA1T97QPY9]
status: done
updated: 2026-10-30
---
# ops-r2 の入力隔離・依頼主体・実験専用 operator session（task 全体のまとめ）

完了日: 2026-10-30

結論: 実装は完了した（main に入る code と試験。fixture・実 bwrap の範囲で確認済み）。配備は未完了で、人の準備（評価器の uid・dir、
測定専用 account の login、K2 の private の扱いの決定）と release 後の実 runtime の確認が残る。**実試行（K2 の正式試行・実 LLM の測定）は 1 件も行っていない。**
K2 を始める前の確認は `docs/ops/ops-r2-k2-preflight.md` の 1 本の手順にまとめた。人の準備が済むまで、その §3（隔離 preflight）は L1 で必ず止まる。

unit ごとの記録は `agent-docs/progress/2026-10-30-ops-r2-isolation-principal/<unit>.md`（isolate・principal・session・probe・stall ほか）。
設計は ADR `agent-docs/adr/2026-10-30-ops-r2-isolation-principal.md`（付記 2026-10-30 を含む）。基準は `$CELERIS_WU_BASE`（`git merge-base HEAD main` と同じ系列）。

## 受入結果（task の受け入れ条件ごと）

| # | 条件 | 結果 | 根拠 |
|---|---|---|---|
| 0 | 実行主体から private grader・旧解答・他試行の出力が読めないことを実 runtime の拒否で確認。評価器は別の信頼境界。deny_readable が残れば preflight が止まる | 実装完了・fixture で確認。実 uid `celeris-eval` での L1 は未確認（人の準備の後） | isolate.md（L1 は持ち主の uid と祖先の書込可否を lstat で判定、同じ uid の chmod 000 で止まる。`CELERIS_USERNS_TESTS=require` の `runtime_probe` 6 passed）、probe.md（CoS run・worker run の両経路で readable=false・listable=false、旧解答を許可表に足すと両経路で `deny_readable`、preflight の `missing` で有料実行 0 件） |
| 1 | operator/MCP principal と出典付き relay。人の token を借りない。判断者・実行者・承認者を区別。人の能動時間は記録区間だけ。新しい権限・恒久承認なし | 実装完了。試験 1 file に人の token が残る（下の残件） | principal.md（`--send` を消し `--relay`＋必須の `--source`、`ops-r1-intake/2` に decided_by・executed_by・approved_by・relayed_by、人の token の `--relay` は 403）、probe.md 3 件目（relay の op は人の時間に数えない、人の op 行の無い試行は値を作らない） |
| 2 | 開始前の実験専用 session と session-map で親/子の transcript を重複なく取り込む。外部 AI の介入は別 op/区間。trial/condition/request と対応。取れない消費は null と理由。schema 再利用、監視基盤なし | 実装完了・fixture で確認。実 Claude Code の起動と transcript の実 path は未確認 | session.md（`ops_r2_session` 4 passed、再取り込みで added=0・byte 一致、`usage_missing`・`transcript_missing`）、probe.md（親と subagent を 2 回ずつ取り込んで message ごとに 1 行） |
| 3 | 実験 LLM・実 HPC 負荷なしの probe で CoS 直接遂行と task 経由の両方、二重計上なし、漏洩時の fail-closed、固定時計の停滞 source→triage→解消。実 LLM で未検証の範囲を明記。CoS 方針・reviewer 経路は不変 | 確認済み（fixture・偽の時計の範囲） | probe.md（`ops_r2_probe` 3 passed）、stall.md（`cos_chat_triage_stagnation` 3 passed、`cos_chat_stagnation_notify` 3 passed、`crates/` と `docs/ops/cos-chat.md` の差分なし） |
| 4 | 回帰試験と test-affected/clippy/fmt。報告に本番設定・OS 権限・credential の範囲、K2 開始前チェック、実装完了と配備完了の区別、証拠・未検証範囲・残件 | この close の検査は下の「証拠」。各 unit の test-affected は exit 0（probe の時点で 2882 passed、9 skipped） | この file の各節、`docs/ops/ops-r2-k2-preflight.md` |

## 実装完了と配備完了

実装完了（main に入る code・試験・文書。この task の branch にある）:

- 入力隔離: `celeris-campaign probe-read`（`celeris.runtime-probe/1`）、`scripts/campaign/ops-r2-sandbox.sh`（B の測定 daemon と operator の L2）、
  正式試行の preflight の `--deny-list`（fail-closed、迂回の flag なし）、L1 の持ち主の判定（`crates/campaign/src/l1_owner.rs`）、
  `check-ops-r1-leak.py --runtime-probe`。
- 依頼主体: `ops-r1-intake --relay --source`、`ops-r1-intake/2`。
- session: `ops-r2-session-prep.sh`・`ops-r2-poll.sh`・`ops-r2-human-span.sh`・`ops-r2-session-map.py`。
- 試験: `crates/campaign/tests/runtime_probe.rs`・`ops_r2_probe.rs`・`ops_r2_session.rs`・`ops_r1_intake.rs`、`scripts/campaign/test_*.py`。
- 文書: `docs/ops/ops-r2-isolation.md`・`ops-r2-operator-session.md`・`ops-r1-intake.md`・`ops-r2-k2-preflight.md`（この close で新規）。

配備完了の条件（どれも未実施）:

1. 人が「本番への移行」の 1〜6 を実行する。
2. この task の変更を含む release を人が作り、`celeris-campaign`（固定した binary）を K2 の測定に使う。
3. release 後の実 runtime で `docs/ops/ops-r2-k2-preflight.md` の §0〜§6 を通す（実 uid `celeris-eval` の L1、本番の deny list、測定専用 account、
   実 Claude Code の `launch.sh` 起動と transcript の実 path、bwrap の中から sudo が効かないこと）。
4. 1〜3 の後に K2 の正式試行を人が承認する。

## 本番への移行

本番 daemon・本番 DB・`~/.config/celeris`・本番の release の config は変えない。人が実行するのは次だけ。sudo が要る項目は人が実行する。
値は例（人の uid `rmaeda`、`EVAL_ROOT=/var/lib/celeris-eval`）。詳しい手順は `docs/ops/ops-r2-isolation.md` §4。

1. 評価器の uid・group（OS 権限）

   ```sh
   sudo groupadd --system celeris-eval
   sudo useradd --system --gid celeris-eval --home-dir /var/lib/celeris-eval --no-create-home --shell /usr/sbin/nologin celeris-eval
   ```

   確認: `id celeris-eval` が uid・gid を出す。`id -nG rmaeda` に `celeris-eval` が無い。

2. `EVAL_ROOT`（0700）・drop box（1733）・outbox（0755）・採点 binary の dir（0755）

   ```sh
   sudo install -d -o celeris-eval -g celeris-eval -m 0700 /var/lib/celeris-eval /var/lib/celeris-eval/private /var/lib/celeris-eval/work /var/lib/celeris-eval/out
   sudo install -d -o celeris-eval -g celeris-eval -m 1733 /var/lib/celeris-eval-inbox
   sudo install -d -o celeris-eval -g celeris-eval -m 0755 /var/lib/celeris-eval-outbox /var/lib/celeris-eval-bin
   ```

   確認: `stat -c '%U %a %n' / /var /var/lib /var/lib/celeris-eval /var/lib/celeris-eval-inbox /var/lib/celeris-eval-outbox` で、
   `/`・`/var`・`/var/lib` が root 持ちで人が書けず、残りが `celeris-eval 700` / `1733` / `755`。人の uid から `ls /var/lib/celeris-eval` が Permission denied。
   `getfacl -p /var/lib/celeris-eval` に追加の ACL が無い。`/local/celeris` の下は使わない（祖先を人の uid が持ち、L1 が `ancestor_owned_by_run_uid` で止まる）。

3. K2 の private を `EVAL_ROOT` に置く（評価材料）

   ```sh
   case=k2-bf-xattr-restart-campaign
   sudo install -d -o celeris-eval -g celeris-eval -m 0700 /var/lib/celeris-eval/private/final /var/lib/celeris-eval/private/final/$case
   sudo install -o celeris-eval -g celeris-eval -m 0400 crates/campaign/fixtures/grader-private/final/$case/private.json /var/lib/celeris-eval/private/final/$case/private.json
   sudo -u celeris-eval sha256sum /var/lib/celeris-eval/private/final/$case/private.json
   ```

   確認: sha256 が承認票の値と一致（repo の現物は `d02126077a47495b121e1e678f055ba4e2493f92d05cd010f2bda32d30694760`、2026-10-30 に計算）。
   K2 の private は git の履歴に残るので、そのまま使うか作り直すかを人が決める（ADR「人に決めてもらう点」1、未回答）。

4. 評価器の起動方法（OS 権限。どちらか 1 つ）

   - 案 1: `sudo visudo -f /etc/sudoers.d/celeris-eval` に `rmaeda ALL=(celeris-eval) NOPASSWD: /var/lib/celeris-eval-bin/grade` の 1 行だけ。
     確認: `sudo -l -U rmaeda` の celeris-eval の行がこの path だけ。採点 wrapper `grade` はまだ repo に無い（残件）。
   - 案 2: sudoers は変えず、人が `sudo -iu celeris-eval` の shell で採点と漏洩検査を走らせる（useradd の shell を `/bin/bash` にする）。確認: その shell で `id -un` が `celeris-eval`。

   採点 binary と `check-ops-r1-leak.py` は `sudo install -o celeris-eval -g celeris-eval -m 0555 <file> /var/lib/celeris-eval-bin/` で置く。
   確認: `stat -c '%U %a' /var/lib/celeris-eval-bin/*` がすべて `celeris-eval 555`。

5. 測定専用の Claude account dir（credential。B の測定 daemon 用と実験専用 operator session 用の 2 つ）

   ```sh
   install -d -m 0700 /local/celeris/measure-accounts/b /local/celeris/measure-accounts/operator
   CLAUDE_CONFIG_DIR=/local/celeris/measure-accounts/b claude          # /login して終了
   CLAUDE_CONFIG_DIR=/local/celeris/measure-accounts/operator claude   # /login して終了
   ```

   確認: 各 dir に `.credentials.json` があり、`projects/`・`memory/` が無い（できていたら消す）。
   `sh scripts/campaign/ops-r2-sandbox.sh --state <空の dir> --release <release> --account <dir> --print-args -- true` が exit 0。
   人が普段使う account dir と既存の credential は変えない・消さない。

6. 測定 daemon の設定（本番の設定ではない。試行ごとに生成）

   `scripts/campaign/ops-r1-measure-config.sh --state <試行ごとに新しい dir> ...` で作る。確認: 生成した daemon.toml の
   `grep -c 'principal = "human:'` が 0、`principal = "operator:ops-r1"` が 1。起動後の `/api/v1/health` の `cos_policy_version` が承認票の方針版と同じ。

7. 確認（人の uid で）: `docs/ops/ops-r2-isolation.md` §4.7 の L1 の確認と、`docs/ops/ops-r2-k2-preflight.md` §3 の preflight が `deny_readable` 0 件。

## 未検証の範囲（論文で使うときの限定）

- 実 LLM: CoS・planner・worker・外部 operator の実モデルで、隔離・relay・session-map・停滞 triage を通していない。CoS の停滞の件への判断と通知の到達も未確認（stall.md）。
- 本物の daemon の B: probe の CoS run・worker run は fixture の shell の子 process。本物の測定 daemon を bwrap の中で起動し、db_guard の namespace が二重で動くかは未確認。
- L1 の実 uid: `celeris-eval` が持つ `EVAL_ROOT` で L1 が通る e2e は無い（試験は別の uid を作れない）。規則は run の uid を注入した単体試験で確認した。
- 実 Claude Code の起動（`launch.sh`）、transcript の実 path、`.credentials.json` を ro で渡したときの OAuth の更新。
- bwrap の中から sudo（setuid）が効かないこと（no_new_privs）。
- 本番の deny list・測定専用 account に対する runtime の拒否。
- 確認したこと（2026-10-30、この close）: `/local/celeris/state/releases` の release 4 件に K2 の private の中身（`false_claim_traps` の key、hidden command の先頭 60 byte）が 0 件。`grader-private` の文字列は release の notes と promote.log の path 名だけ。release に `celeris-campaign` の binary は入っていない。

## 論文で使える証拠

- 隔離: 実 bwrap の中から private・旧解答・他試行の出力が ENOENT で拒否され（open と readdir）、許可外の mount source 0 件（`runtime_probe`・`ops_r2_probe`）。同じ uid の chmod 000 では L1 を通さない（l1-owner）。
- fail-closed: 1 主体で 1 path が読めれば preflight が有料実行・task 作成 0 件で止まる（`tests/preflight.rs` の `one_readable_path_in_any_principal_stops_the_run`）。
- 主体: relay の記録で decided_by・executed_by・approved_by・relayed_by が別の欄に残り、人の token の relay は 403（`tests/ops_r1_intake.rs`）。
- 記録: 親・subagent の transcript の再取り込みで合計不変、区間外は `shared_unattributable`、欠測は null と理由（`ops_r2_session`）。
- 限定: 以上は fixture・in-process router・偽の時計の結果で、実試行の結果ではない。

## 残件

- `crates/celeris/tests/ops_r1_measure.rs` が試験の中だけで人の token（`human:ops-r1-e2e`）を config に足している。relay に書き換える unit が要る（principal.md の提案）。
- 採点 wrapper（drop box → 採点 → 漏洩検査 → outbox）が無い。今の採点は人の uid で `--grader-root` を読む。
- B の測定 daemon を bwrap の中で起動する通し試験（`crates/celeris/tests/`、stub harness）が無い（probe.md の提案）。
- `ops-r2-session-prep.sh` の `launch.sh` と `ops-r2-sandbox.sh` は bwrap の引数を別々に組んでいる（共通化は未着手）。
- `scripts/campaign/ops-r1-measure-config.sh` は `[cos.triage] policy_version = "6"` を書く。現行の既定（"10"）と違うので、K2 の承認票で方針版を決め、測定 daemon の値と揃える必要がある。
- K4 証拠確定 task 01M669QAZE の cov-not-recorded（人の op 0 件 → null）は main に入っている（件名 `integrate wu/cov-not-recorded (phase fixcov)`）が、この task の基準には未取り込み。統合後に D6 と同じ規則か確かめる。

## close の変更

- `close: K2 開始前チェック・人の準備手順・実装完了と配備完了の区別` — `docs/ops/ops-r2-k2-preflight.md`（新規）、
  `docs/ops/mlsys-main-4-ops-r1.md` §7・`mlsys-main-4-pilot-runbook.md` §3・`ops-r2-isolation.md`・`ops-r2-operator-session.md` から参照、この進捗。コード（`crates/`）は変えていない。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

- `bash scripts/dev/test-affected.sh` → exit 0（「試験対象の変更なし（docs のみ等）」。doc 検査の self-test は ok）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- `sh scripts/dev/check-doc-links.sh` → ok。`sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` → ok。`sh scripts/dev/progress-index.sh --check` → ok
- `git diff --name-only $CELERIS_WU_BASE -- crates/` → 0 行（受け入れ条件 2）
- `sh "$CELERIS_WU_SCOPE_PATHS"` → `docs/ops/` の 5 file とこの進捗だけ
- release の確認: `grep -rlaF false_claim_traps <release>`・K2 の hidden command の先頭 60 byte の `grep -rlaF` → `/local/celeris/state/releases` の 4 件とも 0 件

## 提案

- K2 の承認票の雛形（`docs/ops/ops-r1-manifest.template.json` 等）に、deny list・実験専用 session の id・評価器の起動方法・方針版の欄を足す。
- 人の準備の後に、`docs/ops/ops-r2-k2-preflight.md` §0〜§6 を実 runtime で 1 回通し、結果を新しい進捗に残す task を起票する。

## design WU の記録

### 読んだもの

- `crates/campaign/src/isolation.rs`・`preflight.rs`・`ops_r1_intake.rs`・`operator_usage.rs`・`human_ops.rs`・`harness_exec.rs`（A の bwrap）
- `crates/task-worker/src/db_guard.rs`（worker・CoS run の namespace。DB と host 設定を ro にするだけで、他の host の path は見える）
- `crates/task-api/src/chat/mod.rs` の `post_relay`、`crates/task-core/src/chat/mod.rs` の `ChatRelayRequest`・`ChatRelay`
- `scripts/campaign/check-ops-r1-leak.py`、`docs/ops/ops-r1-leak-check.md`・`ops-r1-measure-daemon.md`・`ops-r1-intake.md`・`external-operator.md`・`operator-usage.md`
- `agent-docs/progress/2026-10-29-k4-ops-r1-pilot.md` と `actors.md`・`fix-leak.md`・`measure-e2e.md`・`auth-addendum.md`
- ADR 2026-10-27-relayed-instructions・2026-10-27-human-ops-actor-roles-request-ref・2026-10-29-stagnation-triage-notify-wiring

### 決めたこと（ADR の要点）

- 境界は 2 層。L1 は人の決定 sep-uid（評価器専用 uid `celeris-eval` の 0700 の dir に private grader を置く）。
  L2 は試行の主体（A の agent、B の測定 daemon の CoS と worker、実験専用 operator session）を、許可した path だけを mount した bwrap
  （`--unshare-pid` と新しい `/proc`）で動かす。旧解答・他試行の出力・git に残った private は L2 で塞ぐ。
- B は測定 daemon 全体を bwrap で起動し、試行ごとに `--state` を分ける。CoS run と worker run は daemon の子なので同じ namespace を継ぐ（CoS 直接遂行と task 経由の両方に効く）。
- deny_readable は主体ごとに、その主体を起動するのと同じ包み方の中から `probe-read` で確かめ、1 件でも読めれば preflight の `missing` に入れて正式試行を止める。
- 依頼は既存の `POST /chat/threads/{t}/relays`（`operator:ops-r1` principal）で入れる。`ops-r1-intake` の `--send`（人の token）を消して `--relay` に替え、
  intake 記録 `/2` に出典（本番の message・decision の id、`said_at`、承認票の依頼文 sha256）と decided_by・executed_by・approved_by・relayed_by を持たせる。新しい権限・恒久承認は作らない。
- 人の能動時間は `actor.kind=human` の、start と end がある区間の和集合だけ。人の op 行が 0 件の試行は null（`not_recorded`）。
- 実験専用 operator session は開始前に session id（`--session-id`）・新しい `CLAUDE_CONFIG_DIR`・初期 context・`session-map.base.json` を作る。
  poll と記録は script、LLM の介入は human-op の op 行に分け、最終の session-map は base と op 区間から決定的に作る。schema（human-ops /2、operator-usage /1）は変えない。
- 後続 unit（isolate・principal・session・probe・stall・close）の変えてよい file と、要る OS 権限・credential を ADR の表に書いた。

### 見つけた事実

- K2 と K4 の final の private.json が repo（`crates/campaign/fixtures/grader-private/final/`）に入っており、人の uid のすべての clone と git object から読める。
  L1 だけでは塞げない（ADR の「人に決めてもらう点」1）。
- B の測定 daemon に渡す account dir には過去の session の transcript が入る。測定専用の account dir が要る。
- K4 の measure-e2e の試験（`crates/celeris/tests/ops_r1_measure.rs`）は daemon.toml に人の token（`human:ops-r1-e2e`）を足している。principal unit で relay に替える。

### 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

- `unshare -Ur true` → exit 0
- `bwrap ... --unshare-pid /usr/bin/unshare -Ur /usr/bin/true` → exit 0（bwrap の中で user namespace を作れる）
- 同じ bwrap で `cat /home/rmaeda/workspace/agent-platform/Cargo.toml` → `No such file or directory`
- `git diff --name-only "$CELERIS_WU_BASE" -- crates scripts` → 出力なし（受け入れ条件 2）
- 文書の検査の結果は報告本文に書く

### 未解決

- db_guard の mount 操作が bwrap の中の測定 daemon で通るかは未確認（probe unit で確かめる）。
- release dir に repo の source（fixtures）が入っていないことは未確認（close 段）。
- K4 証拠確定 task 01M669QAZE の人の能動時間の規則（cov-not-recorded）は未統合。統合後に D6 と揃っているかを probe unit が確かめる。

### 提案

- 人に決めてもらう点 2 件（K2 の private の扱い、承認した依頼文の relay）を decisions として出した（result.json）。
