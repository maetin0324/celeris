---
tasks: [01M669QB069GND2ZKA1T97QPY9]
---
# ADR 2026-10-30: 次の研究測定（ops-r2・K2 等）の入力隔離、依頼主体、実験専用 operator session の記録

状態: 採用（設計）。実装は後続 unit（isolate・principal・session・probe・stall・close）。この ADR ではコードを変えない。

## 背景

K4 の ops-r1 pilot（`agent-docs/progress/2026-10-29-k4-ops-r1-pilot*`）で次の 3 点が残った。

1. 漏洩検査（`scripts/campaign/check-ops-r1-leak.py`）は 4 回とも canary・private 文字列の hit が 0 件だったが、
   `deny_readable` の hit が残った（fix-leak の実物確認で grader-private のディレクトリと中の 8 file、計 10 件。
   すべて `same_uid`）。A の agent・B の CoS と worker・外部 operator・評価器が全員、人と同じ uid で動くため、
   chmod では防げない。検査は host から見た mode で判定し、run の sandbox が狭める効果は数えない。
2. B の依頼 message は DB と `ops-r1-intake` で `author=human:rmaeda` だが、送ったのは外部 operator だった
   （人の principal の token を operator が作って使った。actors.md の intake-executor、auth-addendum の逸脱）。
   `ops-r1-intake` は `author` が `human:` 以外の message を拒否するので、operator principal の relay では記録できない。
3. 外部 operator の消費は、人が普段使う Claude Code の session（`~/.claude/projects/...`）から取り込んだ。
   測定と関係ない作業と混ざり、session-map は試行の後に書いた。人の能動時間は区間が 0 件で測れていない。

人の決定 `iso-boundary`（ADR-0079 D7）: **評価器専用の別 uid を人が作り、評価材料をその uid の 0700 の場所に置く。**
計画承認の回答: 「sep-uid（評価器専用 uid・0700 dir を人が 1 回準備）前提で進め、人の準備手順を close 段に含める」。

### 既存の機構（読んだもの）

| 機構 | 場所 | 今の働き |
|---|---|---|
| 試行 dir の生成 | `crates/campaign/src/isolation.rs` `prepare_trial` | repo・materials・kb を `withheld_names` を除いて複製し、入力 hash を記録 |
| A の sandbox | `crates/campaign/src/harness_exec.rs` `Sandbox::args` | bwrap（`--unshare-pid --unshare-ipc --new-session --clearenv`）。見えるのは `/usr`・`/etc` 等の ro、新しい `/proc`、空の `/tmp`、workspace と試行 state dir（rw）、`.credentials.json`（ro）、実行ファイル、toolchain の ro mount と socket だけ |
| worker・CoS run の namespace | `crates/task-worker/src/db_guard.rs` | user+mount namespace で DB dir と host 設定を ro にするだけ。**他の host の path はすべて見える**（同じ uid） |
| preflight | `crates/campaign/src/preflight.rs` `check`・`run_gated` | 不足が 1 件でもあれば有料実行・task 作成を 0 件で止める。隔離の可読判定は持たない |
| 漏洩検査 | `scripts/campaign/check-ops-r1-leak.py`、`docs/ops/ops-r1-leak-check.md` | 内容（sha・文字列・canary）の出現検査と、host から見た可読判定（`deny_readable`・`same_uid`） |
| B の受付 | `crates/campaign/src/ops_r1_intake.rs`、`docs/ops/ops-r1-intake.md` | `--bind-message`（人が送った message）と `--send`（人の token で送る）。`author` が `human:` 以外は拒否 |
| 取り次ぎ | `POST /api/v1/chat/threads/{t}/relays`（ADR 2026-10-27-relayed-instructions） | `operator:<name>` principal だけが投稿でき、人の token は 403。`relay.relayed_by`・`said_at`・`scope`（task・期限）を記録。MCP は `console_instruct` の `relay` |
| 生記録 | `crates/campaign/src/human_ops.rs`（`celeris.eval-human-op/2`） | `actor`・`decided_by`・`executed_by`・`approved_by`・`credential`・`request_ref`・`policy_ref`・`missing`（null の理由） |
| operator 消費 | `crates/campaign/src/operator_usage.rs`、`docs/ops/operator-usage.md` | transcript と session-map から `celeris.operator-usage/1` を作る。dedup_key=`op:<sessionId>:<message.id>`、欠測は null、未対応は `shared_unattributable` |
| 測定 daemon | `docs/ops/ops-r1-measure-daemon.md`、`scripts/campaign/ops-r1-measure-config.sh` | 本番 release の binary を別 config で起動。token は `operator:ops-r1` だけ |

### 見つかった追加の事実

- **K2 の private は git に入っている。** `crates/campaign/fixtures/grader-private/final/k2-bf-xattr-restart-campaign/private.json`
  （K4 の `k4-cmp4-validity-analysis` も同じ）。repo のすべての clone・worktree と git object から読める。
  評価器の uid に写しても、人の uid で読める clone が残る限り境界にならない。
- B の測定 daemon に渡す `--claude-accounts` の account dir には、その account の過去の session の transcript
  （`projects/`）と memory が入る。CoS・worker の `CLAUDE_CONFIG_DIR` がその dir なら、過去の試行の解答に届く。
- この run（2026-10-30、Celeris worker の中）で `bwrap` と、bwrap の中の `unshare -Ur` が通った（下の「確認」）。
  db_guard の mount 操作まで bwrap の中で通るかは未確認。

## 決定

### D1. 主体ごとの到達経路と塞ぎ方

境界は 2 層にする。

- **L1（人の決定 sep-uid）**: private grader（`private.json`・pristine 試験・隠し受入 command・期待値）は評価器専用 uid
  （以下 `celeris-eval`）の 0700 の dir（以下 `EVAL_ROOT`）にだけ置く。人の uid からは開けない。
  評価器はこの uid で動き、試行の主体とは別の信頼境界とする。
- **L2（runtime の mount 許可表）**: 試行の実行主体は、許可した path だけを mount した bwrap の中で動かす
  （`--unshare-pid` と新しい `/proc` を必須にする。host の `/proc/<pid>/root` 経由で外の木に戻れないようにするため）。
  旧解答・他試行の出力・repo の clone（git に残った private を含む）は人の uid の持ち物のまま残るので、L1 では塞げず L2 で塞ぐ。

同じ uid の chmod・mode 000・プロンプトでの禁止は、どちらの層にも数えない。

| 主体 | 届くもの | 今の経路（K4） | 塞ぎ方 |
|---|---|---|---|
| A の直接 agent（`claude -p`） | private grader | bwrap の許可表に無いので届かない。ただし検査は host から見た mode で `deny_readable` を出した | L1 で `EVAL_ROOT` に移す。L2 は今の `Sandbox::args` のまま。runtime から開けないことを D3 の probe で確かめる |
| 同上 | 旧解答・他試行の出力 | 許可表に無い。試行 state dir は試行ごと（ADR 2026-10-18-a-trial-scoped-state） | 変更なし。D3 の probe に含める |
| B の案件 CoS（chat run） | private grader | db_guard の namespace は host 全体を見せる。repo の clone・`crates/campaign/fixtures/grader-private` を読める | L1 ＋ **測定 daemon 全体を L2 の bwrap で起動する**（D2）。CoS run・worker run は daemon の子なので同じ mount namespace を継ぐ。コードの経路は変えない |
| 同上 | 旧解答 | 本番 KB・本番 DB・過去の試行の artifacts（`/local/celeris/data/workspaces/<task>/artifacts/ops-r1/`）・account dir の `projects/` | daemon の許可表に入れない。account dir は測定専用（D2）。KB は `--state` の下の試行用 KB だけ |
| 同上 | 他試行の出力 | 同じ測定 state を複数試行で共有すると `ws/` に他試行の作業場所が残る | **B の試行 1 件ごとに新しい `--state`** で daemon を作る（D2） |
| B の worker（planner・worker・reviewer） | 上の 3 つ | CoS と同じ | CoS と同じ（同じ daemon の namespace） |
| 外部 operator（Claude Code） | private grader・旧解答・他試行の出力 | 人の普段の session。cwd が repo、`~/.claude` も host 全体も見える | 測定中の operator 作業は **実験専用 session**（D6）だけが行う。その session は L2 の bwrap で起動し、cwd は実験の証跡 dir。普段の operator（人の session）は試行区間に測定 daemon を操作しない。違反は D5 の照合で検出し、その試行を正式な試行にしない |
| 評価器（grader・採点） | private grader（正当） | 人の uid で `crates/campaign/fixtures/grader-private` を読んでいた | `celeris-eval` uid で `EVAL_ROOT` から読む。試行の出力は人の uid 側が drop box に置き、評価器が取り込む。評価器が人の uid 側へ返すのは判定 JSON だけで、返す前に評価器の uid で漏洩検査（`--private` に `EVAL_ROOT` の private）を通す（D4） |

公開入力（公開 tgz・材料・試行用 KB・固定依頼文）は試行の開始前に許可領域へ置く。
A は `prepare_trial` の workspace と `--inputs-src` の照合、B は `--state` の下の入力 dir と試行用 KB に置き、sha256 を記録する。
試行中に許可領域の外から入力を取りに行く経路は作らない。

### D2. B の測定 daemon は L2 の bwrap の中で起動する

`scripts/campaign/ops-r2-sandbox.sh`（新規）が bwrap の引数を固定の許可表から組み、その中で測定 daemon を起動する。
許可表は次だけ（他の host の path は存在しない）。

- ro: `/usr`・`/bin` 等・`/etc`、release dir（binary）、toolchain の固定 ro mount（A の `Toolchain` と同じ宣言）
- rw: 試行の `--state`（DB・token・KB・ws・scratch）、新しい `HOME`（`--state/home`）、`/tmp` は tmpfs
- 認証: 測定専用の Claude account dir（人が測定用に 1 回 login した dir。過去の試行の `projects/` を持たない）
- `--unshare-pid --unshare-ipc --new-session --die-with-parent`、`--proc /proc`、`--dev /dev`。
  network namespace は共有する（API の loopback port と LLM の外部接続のため）。本番 daemon の API は token が無いので使えない（下の「残る経路」）

daemon の中の db_guard はそのまま動く（bwrap の中の user namespace）。通ることは probe unit が確かめる。
config は `ops-r1-measure-config.sh` の生成物を使い、`--state` を試行ごとに分ける。本番の `~/.config/celeris`・本番 DB・本番 daemon には触れない。

### D3. deny_readable は runtime の中から確かめ、残れば正式試行の preflight を止める

- `celeris-campaign probe-read`（新規の subcommand、`crates/campaign/src/runtime_probe.rs`）は渡された path を
  `open` して 1 byte 読む試みと、親 dir の `readdir` を行い、結果を JSON で返す。加えて `/proc/self/mountinfo` を読み、
  mount の source が許可表の外にあれば記録する。LLM・network は使わない。
- preflight は主体ごとに、**その主体を起動するのと同じ包み方**で probe を走らせる。
  A は `Sandbox::args` と同じ引数（同じ関数から作る）、B は `ops-r2-sandbox.sh` と同じ許可表、外部 operator は D6 の起動 script と同じ許可表。
  人の uid の process 自身からも `EVAL_ROOT` の private を開く（L1 の確認。開けたら失敗）。開けなくても持ち主の uid で判定する（付記 2026-10-30）。
- 拒否すべき path（deny list）は承認票と同じ固定の入力にする: `EVAL_ROOT` の private、repo の clone の
  `crates/campaign/fixtures/grader-private`（`/home/rmaeda/workspace/agent-platform` と `/local/celeris/data/workspaces/*/repos/*`）、
  過去の campaign の out と試行の artifacts、本番 KB の root、`~/.claude`、`~/.config/celeris`、測定用以外の account dir。
- 1 件でも読めれば、`preflight::check` の `missing` に `deny_readable:<主体>:<path>` を入れる。`run_gated` は既存どおり
  有料実行・task 作成・relay を 0 件で止める。mountinfo に許可表の外の source があっても同じ扱いにする。
- `check-ops-r1-leak.py` は `--runtime-probe <probe の JSON>` を受け、その主体の可読判定を probe の結果で置き換える。
  runtime の拒否と L1 の確認が揃い、かつ `owned_by_run_uid` が空の path には `same_uid` の limit を書かない（付記 2026-10-30）。probe が無い path は今までどおり host から判定する。
  canary の出現検査は残す（読んだ後の書き出しの痕跡を見る副次の証拠）。

### D4. 評価器の境界

- `EVAL_ROOT`（例 `/local/celeris/eval`。付記 2026-10-30 で `/var/lib/celeris-eval` に改めた。持ち主 `celeris-eval`、0700）の下に `private/<case>/`（private.json・pristine 試験）、
  `work/`（採点の作業）、`out/` を置く。
- 人の uid → 評価器: `EVAL_ROOT` とは別の drop box（例 `/local/celeris/eval-inbox`、持ち主 `celeris-eval`、mode 1733）に
  試行の出力の tgz と sha256 を置く。人の uid は書けるが一覧・読み出しはできない。
- 評価器 → 人の uid: `/local/celeris/eval-outbox`（持ち主 `celeris-eval`、0755、file 0644）に判定 JSON だけを置く。
  置く前に評価器の uid で `check-ops-r1-leak.py --private <EVAL_ROOT の private> --scan <判定 JSON>` を通す。
- 評価器の起動は、人が作る狭い sudo 規則（`<人の uid> ALL=(celeris-eval) NOPASSWD: <固定した採点 wrapper の絶対 path>`）か、
  人が `sudo -iu celeris-eval` で入った shell で行う。どちらにするかは人の準備で決める（close 段の手順に両方を書く）。
  採点 wrapper と採点 binary は `celeris-eval` が持ち主の ro の場所に置き、人の uid からは書き換えられないようにする。
- repo の `crates/campaign/fixtures/grader-private/final/` は、正式な試行に使う case について HEAD から消し、`EVAL_ROOT` にだけ置く。
  dev の fixture（試験用）は repo に残す。git の履歴に残った写しは L2 で塞ぐ（D1）。K2 の扱いは「人に決めてもらう点」の 1。

### D5. 依頼の受付は operator principal の relay で行い、人の token を使わない

- 測定 daemon の token は既存の `operator:ops-r1`（`ops-r1-measure-config.sh` が作る）だけにする。人の token を config に足さない
  （measure-e2e の試験が末尾に足していた `human:ops-r1-e2e` の token も、試験を relay に替えて消す）。新しい principal・権限・恒久承認は作らない。
- `ops-r1-intake` の `--send`（人の token で送る）を消し、`--relay` に置き換える。`--relay` は operator principal で
  `POST /api/v1/chat/threads/{t}/relays` に `{client_message_id, text, said_at, task_ids: [], expires_at}` を送る。
  `client_message_id` は今と同じく trial から決める（再実行しても 1 件）。
- `--bind-message` は `relay` を持つ `role=user` の message だけを受ける（`author` が `operator:` か `mcp:`、
  `relay.relayed_by == author`）。`author` が `human:` の message は、人が自分の画面から送った場合に限られるが、
  測定 daemon に人の経路を用意しないので ops-r2 では受けない（受けると「人の token を誰が使ったか」を記録で区別できないため）。
- 取り次ぐ本文は、人が承認票で承認した固定依頼文の bytes（sha256 は承認票に記録済み）。`said_at` は人が承認した時刻
  （本番 daemon の decision・承認 message の時刻）。D1 の relay 規則は「利用者の発言の原文」を求めるが、ここで運ぶのは
  人が発言したのではなく承認した文である。これを記録で区別するため、intake 記録に `source` を持たせる（「人に決めてもらう点」の 2）。
- intake 記録の schema を `ops-r1-intake/2` にし、次の欄を持つ（`/1` の欄は残す。`intake_mode` は `bind`/`relay`）。

| 欄 | 値 |
|---|---|
| `source.kind` | `approved_request`（承認票で承認された固定依頼文）か `chat_message`（人の発言の原文） |
| `source.daemon` | `production`（人の指示がある本番 daemon） |
| `source.thread_id`・`source.message_id`・`source.decision_id` | 人の指示・承認の出典（どれか 1 つ以上が必須） |
| `source.said_at` | relay の `said_at` と同じ値 |
| `source.request_sha256` | 承認票に書かれた依頼文の sha256。`request_sha256` と一致しなければ記録しない |
| `decided_by` | `{kind: human, id: human:<name>}`（出典の主体） |
| `executed_by` | relay の `author`（`operator:ops-r1` 等。daemon が認証した値だけ） |
| `approved_by` | 有料試行の承認の主体（`{kind: human, id}`）。承認の出典は `source.decision_id` |
| `relayed_by` | relay の `relay.relayed_by` |

- human-ops /2 の行は今の schema のまま使う。relay の op は `actor`=`executed_by`=external_operator（実験専用 session の id）、
  `decided_by`=human、`approved_by`=human、`credential`=`operator:ops-r1`、`request_ref`=`chat-message:<relay の message id>`、
  `policy_ref`=出典（`chat-message:<本番の message id>` か `decision:<id>`）、`celeris_event_ref`=relay の message id。
  包括の委任（「runbook を実行して」）を個々の判断の `decided_by=human` にしない。operator が自分で決めた停止・回復は
  `decided_by=external_operator` とし、`note` に委任元の出典を書く（actors.md の提案を採る）。
- 照合（probe unit の check）: 試行区間に測定 daemon の events・CoS operation の監査に現れた operator principal の変更操作は、
  すべて human-ops の op 行（実験専用 session の id）と 1 対 1 で対応すること。対応しない操作があれば、その試行を正式にしない。

### D6. 人の能動時間は記録した区間だけで測る

- 人の能動時間は `actor.kind=human` の op 行のうち、`start`・`end` の両方がある区間の和集合だけにする。
  CoS の実行時間・回答待ち・operator の区間から推定しない。
- 人の行為の区間は、人が自分の shell で `celeris-campaign human-op record --actor-kind human`（session unit の
  `ops-r2-human-span.sh begin|end` が包む）を打って残す（`time_basis=evaluator_live`）。web での承認・回答のように
  点でしか残らないものは `occurred_at` の点として残し、長さ 0 として足さない。
- 人の op 行が 0 件の試行は、人の能動時間を 0 ではなく null（`missing` に `not_recorded`）とする
  （K4 証拠確定 task 01M669QAZE の cov-not-recorded と同じ規則。その task の統合後にこの ADR の本文と揃っていることを probe unit が確かめる）。

### D7. 実験専用の operator session と session-map は開始前に用意する

- `scripts/campaign/ops-r2-session-prep.sh`（新規）が試行前に次を作る（LLM を呼ばない）。
  - session id（UUID）を決め、Claude Code を `--session-id <uuid>` で起動する起動票（Claude Code の版・model・account・方針版・初期 context の sha256）
  - 新しい `CLAUDE_CONFIG_DIR`（測定専用）。`settings.json` に `cleanupPeriodDays: 3650` と、subagent の道具（`Agent`）の拒否を書く。
    `CLAUDE.md`・memory・skills は置かない（人の `~/.claude` を読まない）
  - 初期 context（固定 file）: 試行の識別（trial・condition・request）、使ってよい script、停止条件、記録の規則だけ。repo・過去の報告を含めない
  - `session-map.base.json`: その session id の `prep`・`observe` の区間を試行ごとに書いた `operator_usage::Mapping` の配列（schema は今のまま）
  - L2 の起動 script（D2 と同じ組み方。許可表は証跡 dir（rw）、`CLAUDE_CONFIG_DIR`（rw）、`.credentials.json`（ro）、
    固定した `celeris-campaign`・`celerisctl`（ro）、operator token file（ro））
- 単純な状態の poll と記録は `scripts/campaign/ops-r2-poll.sh`（新規）が行う。測定 daemon の `GET` だけを一定間隔で呼び、
  human-op の `job_observation`/`status_check` 行を `actor.kind=script`・`mode=automatic` で書く。止まる条件（停止条件・
  受信箱の件・error・cutoff）で exit し、LLM の session はそのときだけ判断する。
- operator の LLM session の介入（relay・停止・回復・回答）は 1 件ずつ human-op の op 行（`op_start`/`op_end`）にし、
  `trial_key`・`condition_id`・`request_ref` を付ける。`scripts/campaign/ops-r2-session-map.py`（新規）は
  `session-map.base.json` と human-op の op 区間から最終の `session-map.json` を決定的に作る（`observe` の区間を
  `intervene` の区間で分ける。同じ入力なら byte 一致）。
- transcript は `CLAUDE_CONFIG_DIR/projects/<dir>/<uuid>.jsonl` と `<uuid>/subagents/*.jsonl` を、今の
  `operator-usage.md` の手順で snapshot して `celeris-campaign operator-usage` で取り込む。親と子の両方に同じ message が
  あっても dedup_key で 1 回だけ数える（probe unit の fixture で確かめる）。
- 取れない消費: transcript が無い・壊れている session は、`operator-usage.jsonl` に行を作らず、報告の消費の欄を null にし、
  理由（`not_recorded`・`transcript_missing` 等）と session id を書く。0 で埋めない。普段の operator の session は
  この対応表に入れない（区間外の作業は試行に帰属しない）。
- 汎用の監視基盤は作らない。script は ops-r2 の証跡 dir の中だけで動き、daemon に常駐の仕組みを足さない。

## 後続 unit の作業範囲

| unit | 変えてよい file | 要る OS 権限・本番設定 |
|---|---|---|
| isolate | `crates/campaign/src/runtime_probe.rs`（新規）、`crates/campaign/src/preflight.rs`、`crates/campaign/src/harness_exec.rs`（probe と起動で同じ引数を作る関数の公開だけ）、`crates/campaign/src/main.rs`（`probe-read`）、`crates/campaign/src/lib.rs`、`crates/campaign/tests/runtime_probe.rs`（新規）、`scripts/campaign/ops-r2-sandbox.sh`（新規）、`scripts/campaign/check-ops-r1-leak.py`・`test_check_ops_r1_leak.py`、`scripts/campaign/ops-r1-measure-config.sh`・`test_ops_r1_measure_config.py`（試行ごとの `--state` と `HOME`）、`docs/ops/ops-r1-leak-check.md`、`docs/ops/ops-r2-isolation.md`（新規） | 試験は一時 dir と bwrap だけ。`celeris-eval` uid は使わない（L1 の確認は「開けない file」を試験用の別の仕組みで作れないので、fixture では注入した probe 結果で判定を試験し、実 uid の確認は close 段の人の手順に置く） |
| principal | `crates/campaign/src/ops_r1_intake.rs`・`ops_r1_intake_tests.rs`、`crates/campaign/src/main.rs`（引数）、`crates/celeris/tests/ops_r1_measure.rs`（人の token の追加を relay に替える）、`docs/ops/ops-r1-intake.md`、`docs/ops/eval-raw-record-definitions.md`（D5・D6 の記録規則） | なし（測定 daemon は試験の一時 config） |
| session | `scripts/campaign/ops-r2-session-prep.sh`・`ops-r2-poll.sh`・`ops-r2-human-span.sh`・`ops-r2-session-map.py`（新規）とその試験 `scripts/campaign/test_ops_r2_session.py`、`crates/campaign/src/operator_usage.rs`（試験の追加だけ。schema は変えない）、`docs/ops/operator-usage.md`、`docs/ops/ops-r2-operator-session.md`（新規） | Claude Code は起動しない（起動票と config dir の生成まで） |
| probe | `crates/campaign/tests/ops_r2_probe.rs`（新規）、`crates/celeris/tests/ops_r2_isolation_probe.rs`（新規。stub harness の測定 daemon を D2 の bwrap で起動し、CoS が自分で済ませる経路と task（planner・worker）経由の経路の両方から deny list を probe）、`scripts/campaign/` の probe 用 fixture | bwrap と user namespace（この run では通った。通らない環境では `CELERIS_USERNS_TESTS=1` の opt-in にする。ADR-0126） |
| stall | `crates/task-dispatch/src/dispatcher/tests/cos_chat_triage_stagnation.rs` か `crates/celeris/tests/cos_chat_stagnation_notify.rs` への固定時計の試験の追加だけ。本体の CoS 方針・reviewer 経路・triage の規則は変えない | なし |
| close | `docs/ops/ops-r2-k2-preflight.md`（新規。K2 の開始前チェックと人の準備手順）、進捗 file | 人の準備（下）を手順として書くだけ。実行は人 |

## 人の準備が要る点（close 段で手順にする）

- OS: `celeris-eval` の uid・group の作成、`EVAL_ROOT`（0700）・drop box（1733）・outbox（0755）の作成、採点 wrapper と
  採点 binary の配置（`celeris-eval` 持ち、人の uid は書けない）、評価器の起動方法（狭い sudo 規則か `sudo -iu`）の設定。sudo は人が実行する。
- 評価材料の移動: 正式試行に使う case の private を `EVAL_ROOT/private/<case>/` に置き、repo の HEAD から消す（「人に決めてもらう点」の 1 の答えによる）。
- credential: 測定専用の Claude account dir を B の測定 daemon 用と実験専用 operator session 用に用意する（人が `CLAUDE_CONFIG_DIR=<新しい dir>` で 1 回 login）。
  人が普段使う account dir をそのまま渡さない（`projects/` に過去の解答が入るため）。既存の credential は消さない。
- 本番設定: 変更なし。本番 daemon・本番 DB・`~/.config/celeris` は触らない。測定 daemon の config は試行ごとに生成する。
- 承認票: 試行数・上限・model・account・停止条件・期限に加え、固定依頼文の sha256、deny list、実験専用 session の id、評価器の起動方法を人が埋める（K4 の逸脱「空欄を operator が埋めた」を繰り返さない）。

## 人に決めてもらう点

1. K2 の private.json は git の履歴に残っている。L2（runtime の mount 許可表）で試行の主体から塞いだうえで K2 をそのまま使うか、K2 の private を作り直すか。
2. 固定依頼文を operator が relay するとき、取り次ぐ本文を「人が承認票で承認した文」（`source.kind=approved_request`）として扱ってよいか。
   代わりの案は、人が自分の画面で測定 daemon の案件 CoS thread に依頼文を送る（人の token は人だけが持つ。web の gateway を測定 daemon に向ける準備が要る）。

## 残る経路と限界

- network namespace は共有するので、bwrap の中から loopback の他の service（本番 daemon の API 等）に接続できる。
  token が許可表に無いので認証は通らないが、認証の要らない endpoint（health 等）は読める。private・旧解答を返す endpoint は無い（未確認の service は残る）。
- L2 の許可表に入れた物（release dir・toolchain・`/etc`）に private の写しが無いことは、D3 の deny list と mountinfo の照合では
  確かめられない。release dir に repo の source（fixtures）が入っていないことを close 段で確かめる（未確認）。
- 普段の operator（人の session）は人の uid で host 全体を読める。試行区間にそれが測定 daemon を操作したことは D5 の照合で事後に見つかるが、
  読んだだけで何も操作しなかった場合は検出できない。その session の出力が試行に入らないこと（試行の入力は承認票の固定入力だけ）で扱う。
- 実 LLM（Claude Code・CoS の実モデル）での確認はしない。fixture・stub harness・bwrap の probe で確かめる範囲を probe unit の報告に明記する。

## 確認（この unit で行ったもの。2026-10-30、Celeris worker の中）

- `unshare -Ur true` → exit 0。`bwrap --ro-bind /usr /usr ... --unshare-pid /usr/bin/unshare -Ur /usr/bin/true` → exit 0（bwrap の中の user namespace が作れる）。
- 同じ bwrap で `cat /home/rmaeda/workspace/agent-platform/Cargo.toml` → `No such file or directory`（許可表に無い path は存在しない）。
- `ls -ld /home/rmaeda/workspace/agent-platform/crates/campaign/fixtures/grader-private/final` → 持ち主 rmaeda、mode `drwxrwxr-x+`（人の uid から読める。K2・K4 の final private がある）。

## 付記（2026-10-30）: L1 は open の拒否でなく持ち主の uid で判定する

final review（task 01M66JYQ249SZHNXK0AEVTZNEC の criterion 3）の差し戻しによる。前の実装の L1 は、人の uid の process から
deny path を `open`・`readdir` した成否だけで判定した。人の uid が持つ `EVAL_ROOT` の dir と file を chmod 000 にすると
open は EACCES になり、`deny_readable` 0 件で preflight が通った。持ち主は chmod で読めるように戻せるので、これは本文の
「同じ uid の chmod・mode 000 はどちらの層にも数えない」に反していた。漏洩検査も、runtime と L1 の probe が両方拒否なら
持ち主を見ずに `same_uid` の limit を外していた。

- **L1 の判定（D3 を改める）**: probe の拒否に加えて、preflight は `--eval-root` の下の deny path について lstat で次を確かめ、
  1 つでも満たさなければ `deny_readable:L1:<path>:<理由> <要素> (uid <n>, mode <o>)` で止める（`crates/campaign/src/l1_owner.rs`）。
  - `EVAL_ROOT` から path までの各要素: 持ち主の uid が run の uid でも 0 でもない（run の uid が chmod できない）、
    run の uid から書けない（sticky でも不可）、symlink でない。
  - `EVAL_ROOT` の祖先（`/` まで）: 持ち主が run の uid でなく、run の uid から書けない（run の uid が `EVAL_ROOT` を rename で
    差し替えられない。sticky の dir は他人の entry を rename できないので可）、symlink でない。
  - run の uid は preflight を走らせた process の実効 uid と所属 group。書けるかは持ち主・group・その他のうち当てはまる 1 組の bit で決める。
    run の uid が 0 なら常に書けるとみなす（root の preflight は L1 で必ず止まる）。`EVAL_ROOT` と祖先を stat できなければ止める。
    `EVAL_ROOT` より下で ENOENT・EACCES になった所では歩みを止めて通す（親が条件を満たすので run の uid は作ることも覗くこともできない）。
  - 迂回の env 変数・CLI flag は置かない。`celeris.runtime-probe/1` の JSON の形は変えない（持ち主の判定は probe の外で行う）。
- **漏洩検査（D3 を改める）**: `check-ops-r1-leak.py` は probe で判定した path も host の stat で、path と祖先のうち run の uid が
  持ち主のものを `owned_by_run_uid` に書く。`same_uid` の limit を外す（`same_uid_cleared_by: runtime_probe_and_l1`）のは、
  runtime と L1 の probe が両方 `readable=false` で、かつ `owned_by_run_uid` が空のときだけ。空でなければ limit を残し、
  `--canary` が無ければ `same_uid_without_canary` の hit にする。
- **`EVAL_ROOT` の置き場所（D4 の例を改める）**: `/local` と `/local/celeris` は人の uid が持つので、`/local/celeris/eval` は
  `ancestor_owned_by_run_uid` で止まる。例は `/var/lib/celeris-eval`（drop box・outbox・採点 wrapper の dir も `/var/lib` の下）に改める。
  手順は `docs/ops/ops-r2-isolation.md` §2.3・§4.2。
- **試験（「後続 unit の作業範囲」の isolate の行を補う）**: 試験は別の uid を作れない。同じ uid の chmod 000 の実 fixture で preflight
  （library と実 binary）が止まり、漏洩検査が `same_uid` を残すことを確かめる。通過の規則は run の uid を注入した単体試験で確かめ、
  実 binary の通過は host にある他 uid 持ちの dir（中の path は存在しない）で確かめる。`celeris-eval` が持つ実在の private で通る確認は、
  人の準備の後に人が行う（運用文書 §4.7）。

## 関連

- ADR 2026-10-27-relayed-instructions（relay の route と `instructed_by`）、2026-10-27-api-principals、2026-10-27-external-operator、
  2026-10-27-human-ops-actor-roles-request-ref、2026-10-27-operator-usage、2026-10-29-ops-r1-cos-intake-record、
  2026-10-17-mlsys-pilot2-fixes（A の bwrap）、2026-10-18-a-trial-scoped-state、0095（db_guard）、0126（userns 試験の opt-in）、
  2026-10-29-stagnation-triage-notify-wiring（stall unit の対象）。
