---
tasks: [01M669QB069GND2ZKA1T97QPY9]
---
# ops-r2 K2 の開始前チェック: 隔離 preflight・漏洩検査・operator principal・session-map・停止規則

K2（`k2-bf-xattr-restart-campaign`）を ops-r2 の条件で正式に試行する前に、operator（実験専用 session か人）がこの順で 1 回ずつ確かめる。
1 項目でも期待と違えば試行を始めない。設計は [ADR 2026-10-30-ops-r2-isolation-principal](../../agent-docs/adr/2026-10-30-ops-r2-isolation-principal.md)。
各道具の詳しい使い方は [入力隔離](ops-r2-isolation.md)・[ops-r1 漏洩検査](ops-r1-leak-check.md)・[CoS 受付の記録](ops-r1-intake.md)・
[実験専用 operator session](ops-r2-operator-session.md)・[測定 daemon](ops-r1-measure-daemon.md)・[pilot runbook](mlsys-main-4-pilot-runbook.md) にある。

このチェックは LLM・実 HPC 負荷を使わない。本番 daemon・本番 DB・`~/.config/celeris` は変えない。
2026-10-30 の時点で、このチェックを K2 で通した試行は無い（人の準備 §0 が未実施のため、§3 の L1 で必ず止まる）。

以下の変数は例。`P` は試行の証跡 dir（試行ごとに新しい dir）、`S` は B の試行 state、`O` は operator session の state、
`R` は固定した release dir、`CC` は固定した `celeris-campaign`、`EVAL_ROOT=/var/lib/celeris-eval`、`case=k2-bf-xattr-restart-campaign`。

## 0. 人の準備が済んでいる

[入力隔離 §4](ops-r2-isolation.md#4-人が実行する準備手順) の 4.1〜4.7 を人が済ませている。加えて次が済んでいる。

| 項目 | 確かめ方 | 期待 |
|---|---|---|
| K2 の新しい private（人の決定 k2-private-in-git = reauthor。ADR 付記 2026-10-30「K2 の private の作り直し」） | 承認票の「K2 の新しい private の sha256」欄。作成は [入力隔離 §4.5](ops-r2-isolation.md#45-正式-case-の新しい-private-を-repo-の外で作り-eval_root-に置く) | 人（または評価器 uid の shell）が repo の外で作り、`$EVAL_ROOT/private/final/$case/private.json` に `celeris-eval` 持ち 0400 で置き、§4.5 の書式確認が `ok`、sha256 を人が承認票に書いている。欄が空なら始めない |
| K2 の `case.json` の `private_ref.sha256` | `python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["private_ref"]["sha256"])' crates/campaign/cases/final/$case/case.json` | 承認票の新しい private の sha256 と同じ。違えば grader の読込が `private file hash mismatch` で止まるので始めない（2026-10-30 の時点では承認票の値が無く、一致しない。値が出たら [入力隔離 §4.5](ops-r2-isolation.md) の `ops-r2-set-private-sha.py` で替えて commit する） |
| K2 の `case.json` の `grade_case_ref.sha256` | `python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["grade_case_ref"]["sha256"])' crates/campaign/cases/final/$case/case.json` | 承認票の grade-case の sha256（評価器が `$EVAL_ROOT/grade-cases` に `gen-grade-case` で作ったもの）と同じで、repo の旧 grade-case の値 `f84089bcf161…190aca593` ではない。旧値のままなら始めない（[入力隔離 §4.5](ops-r2-isolation.md)） |
| 採点 wrapper の install | `sha256sum /var/lib/celeris-eval-bin/grade scripts/campaign/ops-r2-grade.py` | 2 つの sha256 が同じ（install は [入力隔離 §4.3](ops-r2-isolation.md)） |
| 承認票の欄（試行数・上限・model・account・停止条件・期限・固定依頼文の sha256・K2 の新しい private の sha256・deny list・実験専用 session の id・評価器の起動方法） | 承認票を開く | すべて人が埋めている。operator が空欄を埋めていない（K4 の逸脱を繰り返さない） |
| 評価器の uid | `id celeris-eval`、`id -nG` | uid がある。人の uid の groups に `celeris-eval` が無い |
| 測定専用 account dir（B 用・operator 用の 2 つ） | `ls -A <dir>` | `.credentials.json` があり、`projects/`・`memory/` が無い |

## 1. release dir に private が入っていない

試行の主体は release dir を ro で見る（L2 の許可表の中）。ここに private の写しがあると deny list と mountinfo では見つからない。

```sh
sha256sum "$EVAL_ROOT/private/final/$case/private.json"   # 評価器の uid（celeris-eval の shell）で。承認票の「K2 の新しい private の sha256」と一致
grep -rlaF false_claim_traps "$R" | wc -l                  # 0
find "$R" -path '*grader-private*' | wc -l                 # 0
```

照合に使うのは承認票の新しい private の sha256 だけ。repo と git の履歴にある旧 private（漏洩済みの旧解答）の sha256 とは比べない。

`grep -rlaF grader-private "$R"` が `notes.md`・`notes.json`・`promote.log` だけを返すのは、release の変更記録に path 名が出るためで問題ない。
2026-10-30 に `/local/celeris/state/releases` の 4 件で確かめた結果: `false_claim_traps` 0 件、K2 の旧 private の hidden command の先頭 60 byte 0 件（新しい private は repo に入らないので release にも入らない）、
`grader-private` の文字列は notes と promote.log だけ（release に `celeris-campaign` の binary は入っていない）。

## 2. 測定 daemon の config に人の token が無い

```sh
sh scripts/campaign/ops-r1-measure-config.sh --state "$S" --release-dir "$R" --models <cos-opus|all-sonnet> \
  --claude-accounts <B 用の測定専用 account dir> --clusters-from ~/.config/celeris/config.toml --out "$S/../daemon.toml"
grep -c 'principal = "human:' "$S/../daemon.toml"   # 0
grep -c 'principal = "operator:ops-r1"' "$S/../daemon.toml"   # 1
```

期待: token は `operator:ops-r1` だけ。`--state` は試行ごとに新しい dir（他試行の `ws/` を見せない）。

## 3. 隔離 preflight（deny_readable が 0 件）

deny list（1 行 1 絶対 path）は承認票の deny list を実在の path に展開して書く。最低限、次を入れる。

- `$EVAL_ROOT/private/final/$case/private.json`
- 人の repo clone と Celeris の作業場所の clone の `crates/campaign/fixtures/grader-private/final/$case/private.json`（漏洩済みの旧 private。旧解答として塞ぐ）
  （`/home/rmaeda/workspace/agent-platform/...` と `/local/celeris/data/workspaces/*/repos/agent-platform/...` を展開した path）
- 同じ clone の `crates/campaign/fixtures/grade-cases/final/$case/` の中の file（`$case.json`・`hidden/.grader/` の試験。旧 private から作った漏洩済みの grade-case）
- 過去の campaign の out と試行の artifacts の中の file（K4 の `artifacts/ops-r1/` 等）
- 本番 KB の root の中の file、`~/.claude` と `~/.config/celeris` の中の file、測定用以外の account dir の中の file

dir ではなく中の file を書く（親を一覧できるだけで hit になる）。

```sh
"$CC" preflight <manifest・measure-config 等の通常の引数> \
  --deny-list "$P/deny.list" --eval-root "$EVAL_ROOT" \
  --sandbox-script scripts/campaign/ops-r2-sandbox.sh --probe-program "$CC" \
  --b-state "$S" --b-release "$R" --b-account <B 用 account dir> \
  --operator-state "$O" --operator-release "$R" --operator-account <operator 用 account dir>
echo "exit=$?"
```

期待: `exit=0` で、`missing` に `deny_readable:` で始まる行が 0 件。`measure-launcher.sh preflight` で走らせるときは env file の
`DENY_LIST`・`EVAL_ROOT`・`SANDBOX_SCRIPT`・`PROBE_PROGRAM`・`B_*`・`OPERATOR_*` に同じ値を書く。
出た行の読み方は [入力隔離 §2.3](ops-r2-isolation.md#23-celeris-campaign-preflight---deny-listdeny_readable-で止める)。
`deny_readable:L1:...:ancestor_owned_by_run_uid` は `EVAL_ROOT` を人の uid が持つ dir の下に置いたとき、`owned_by_run_uid` は人の準備が済んでいないときに出る。

正式試行（`--measure-config` 指定）では、この検査を外す env 変数・flag は無い。1 件でも残れば preflight は有料実行・task 作成を 0 件で止める。

## 4. 漏洩検査（runtime の probe と L1 の probe を渡す）

B と operator の包み方の中から probe を取り、L1 は人の uid から包まずに取る。

```sh
sh scripts/campaign/ops-r2-sandbox.sh --state "$S" --release "$R" --account <B 用 account dir> --ro "$CC" -- \
  "$CC" probe-read --principal B $(sed 's/^/--deny /' "$P/deny.list") --out "$S/probe-B.json"   # exit 0
"$CC" probe-read --principal l1 --deny "$EVAL_ROOT/private/final/$case/private.json" --out "$P/probe-l1.json"   # exit 0
python3 -I scripts/campaign/check-ops-r1-leak.py --private "$EVAL_ROOT/private/final/$case/private.json" \
  --scan <試行の出力の写し> --deny-readable $(cat "$P/deny.list") \
  --runtime-probe "$S/probe-B.json" --runtime-probe "$P/probe-l1.json" --out <outbox>/leak-pre.json; echo "exit=$?"
```

期待: probe 2 本が exit 0、漏洩検査が `exit=0`・`pass: true`。`same_uid_limit: false` は、持ち主の uid が人の uid でない path だけに付く。
漏洩検査は `--private` に `EVAL_ROOT` の private を読むので、評価器の uid で走らせる（[入力隔離 §4.4](ops-r2-isolation.md#44-評価器の起動方法どちらかを選ぶ) の案 2 の shell。
案 1 では採点 wrapper `grade` が判定 JSON について行う）。人の uid から private を開けないのが正しい状態で、人の uid で走らせると入力の誤りで止まる。
採点の判定 JSON は wrapper が drop box の tgz を sha256 で照合・展開して採点し、漏洩検査を通してから outbox に置く。
試行の出力そのものを `--scan` に渡す上の検査は wrapper に含めず、評価器の shell（`sudo -u celeris-eval /bin/bash`）で手で行う（残す）。
`check-ops-r1-leak.py` の probe の欄（`errno` は整数）は `celeris.runtime-probe/1` のまま。

bwrap の中から評価器の sudo が効かないことも確かめる（[入力隔離 §4.4](ops-r2-isolation.md#44-評価器の起動方法どちらかを選ぶ)、実機では未確認）。

```sh
sh scripts/campaign/ops-r2-sandbox.sh --state "$S" --release "$R" --account <B 用 account dir> -- \
  /usr/bin/sudo -n -u celeris-eval /var/lib/celeris-eval-bin/grade; echo "exit=$?"   # 0 以外
```

## 5. operator principal と出典

- `--source` の file（`approved_request`）に、承認の `decision_id`・承認の時刻 `said_at`・承認票の依頼文 sha256・`decided_by`・`approved_by` を書く。
  `sha256sum <固定依頼文>` が `request_sha256` と一致すること。
- `ops-r1-intake --relay` は operator の token（`$S/token`）で呼ぶ。人の token を環境変数に置かない。
- 試行区間に、人の普段の session が測定 daemon を操作しない（照合で見つかればその試行は正式にしない。ADR D5）。
- 停止・回復を operator が自分で決めたときは、human-op の `decided_by=external_operator` とし、`note` に委任元の出典を書く。

確かめ方: relay の後で `$P/intake.json` の `executed_by`・`relayed_by` が `operator:ops-r1`、`decided_by`・`approved_by` が `human:<name>`。
記録が書かれない（exit 1）なら、理由は [CoS 受付の記録](ops-r1-intake.md) の「拒否する場合」を見る。

## 6. 実験専用 operator session と session-map の準備

```sh
sh scripts/campaign/ops-r2-session-prep.sh --evidence "$P/operator-session" --trials "$P/trials.json" \
  --claude-version <版> --model <model> --account-dir <operator 用 account dir> --policy-version <方針版> \
  --claude-bin <claude> --campaign-bin "$CC" --celerisctl <固定した celerisctl> \
  --token-file "$S/token" --api-url http://127.0.0.1:<測定 daemon の port>
(cd "$P/operator-session" && sha256sum -c SHA256SUMS)   # すべて OK
```

期待: `ticket.json`（session id・版・model・account・方針版・sha256・`transcript_expected`）、`session-map.base.json`、`session-trials.json`、
`launch.sh`、`claude-config/settings.json`（`cleanupPeriodDays: 3650`、`Agent`・`Task` の拒否）がある。
`trials.json` の各試行に `trial_key`・`condition_id`・`request_ref` と prep・observe の区間がある。
承認票の実験専用 session の id が `ticket.json` の session id と一致する。

試行中は `bin/ops-r2-poll.sh` が状態を見て記録し、operator の LLM session は poll が 0 以外で返ったときだけ判断する。
operator の介入は 1 件ずつ開始と終了のある `human-op record --actor-kind external_operator` にし、人の能動時間は
`bin/ops-r2-human-span.sh begin|end` で人が記録した区間だけを数える（[実験専用 operator session §2](ops-r2-operator-session.md#2-試行の間-poll-と記録)）。
初回の起動で `transcript_expected` と実物の path を照合する（未確認）。

## 7. 停止規則の確認

[pilot runbook §3](mlsys-main-4-pilot-runbook.md#3-停止条件) の 9 項目に、ops-r2 では次を足す。どれかに当たったら、その試行の後に進まず、
`sh scripts/campaign/measure-launcher.sh stop --env <env>` で測定 daemon を止め、事実を進捗に書いて人に返す。

1. 隔離 preflight に `deny_readable:` が 1 件でも出た（§3）、または漏洩検査が exit 0 以外（§4）。
2. 試行区間の operator principal の変更操作に、human-ops の op 行（実験専用 session の id）と対応しないものがあった（ADR D5）。
3. 人の普段の session が試行区間に測定 daemon を操作した。
4. `ops-r2-session-map.py final` が exit 2（終了の無い介入、他試行にかかる介入、trial_key・condition_id の不一致）。
5. `ops-r2-poll.sh` が exit 2（取得か記録の失敗）で、記録を後から足せない。
6. 測定 daemon の `cos_policy_version`（`/api/v1/health`）が承認票の方針版と違う。

K2 は cluster job を使う。試行後も job が終端していなければ、pilot runbook §3 の 8 どおり人が `cluster-cancel.jsonl` の手順で消す（自動では消さない）。

## 8. 記録を残す

`$P` に `deny.list`・preflight の出力・`probe-*.json`・`leak-pre.json`・`intake.json`・`operator-session/` を残し、sha256 を進捗に書く。
取れなかった値（消費・人の時間）は null と理由（`not_recorded`・`transcript_missing`・`usage_missing:<件数>`）で残し、0 で埋めない。
