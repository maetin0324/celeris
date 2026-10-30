# ops-r2 の実験専用 operator session

ops-r2（K2 等の次の研究測定）で外部 operator（Claude Code）が試行に関わるときは、人の普段の session ではなく、
試行の前に用意した実験専用 session だけを使う。消費は `celeris.operator-usage/1`（[operator-usage](operator-usage.md)）の
schema のまま記録し、session-map は試行の前に作る。設計は
[ADR 2026-10-30-ops-r2-isolation-principal](../../agent-docs/adr/2026-10-30-ops-r2-isolation-principal.md) D6・D7。

ここに挙げる script は LLM・Claude Code・daemon を起動しない。Claude Code の起動（`launch.sh`）は人が行う。
汎用の監視基盤ではなく、証跡 dir の中だけで動く。
試行前の確認全体の順序は [K2 の開始前チェック](ops-r2-k2-preflight.md) の §6。

## 1. 試行の前: session を用意する

試行ごとの区間を書いた trials file を用意する（時刻は承認票の予定。RFC3339、`null` は無限）。

```json
{"trials": [
  {"trial_key": "<ledger の idempotency_key>", "condition_id": "B-direct", "request_ref": "chat-message:<relay の id>",
   "prep": {"start": "2026-11-01T00:00:00Z", "end": "2026-11-01T00:10:00Z"},
   "observe": {"start": "2026-11-01T00:10:00Z", "end": "2026-11-01T02:00:00Z"}}
]}
```

```sh
sh scripts/campaign/ops-r2-session-prep.sh --evidence /測定証跡の絶対パス/operator-session \
  --trials trials.json --claude-version <版> --model <model> --account-dir <測定専用の account dir> \
  --policy-version <方針版> --claude-bin <claude の path> --campaign-bin <固定した celeris-campaign> \
  --celerisctl <固定した celerisctl> --token-file <operator principal の token> --api-url <測定 daemon の URL>
```

- `--evidence` は無い dir を指定する（既にあれば止まる。作り直しは別の dir に作り、旧証跡を残す）。
- `--account-dir` は人が `CLAUDE_CONFIG_DIR=<新しい dir>` で 1 回 login した測定専用の dir。人の普段の `~/.claude` は拒否する。
- 作る物: `ticket.json`（起動票: session id、Claude Code の版・model・account・方針版、各 file と道具の sha256、
  transcript の予定 path）、`claude-config/settings.json`（`cleanupPeriodDays: 3650`、`Agent`・`Task` の拒否。
  CLAUDE.md・memory・skills は置かない）、`context.md`（`scripts/campaign/ops-r2-operator-context.md.tmpl` から作る初期 context）、
  `session-map.base.json`、`session-trials.json`、`bin/`（poll・human-span の写し）、`launch.sh`、`SHA256SUMS`。
- `launch.sh` は bwrap の中で `claude --session-id <id>` を起動する。見えるのは `/usr`・`/etc` 等の ro、証跡 dir（rw）、
  `.credentials.json`・3 つの道具・token（ro）だけ。repo・人の `~/.claude` は見えない。

## 2. 試行の間: poll と記録

状態の確認は LLM でなく script が行う。

```sh
bin/ops-r2-poll.sh --api <URL> --token-file <token> --task <task id> --human-ops human-ops.jsonl \
  --campaign-bin <celeris-campaign> --interval 60 [--cutoff <RFC3339>] \
  -- --campaign <id> --manifest <manifest> --condition <condition> --case <case> --rep <n> --credential operator:<name>
```

1 回の GET ごとに human-op の `status_check` 行（`actor.kind=script`、`mode=automatic`、`note=status=<状態>`）を書く。
exit 0 は終端（done・failed・cancelled）、10 は判断が要る状態（blocked・reviewing）、11 は cutoff、12 は poll 回数の上限、
2 は取得か記録の失敗。operator の session は 0 以外で返ったときだけ判断する。

operator の介入（relay・停止・回復・回答）は 1 件ずつ `human-op record --actor-kind external_operator --actor <session id>
--started-at <開始> --ended-at <終了> --request-ref <出典>` で記録する。終了時刻の無い介入は次の手順で session-map を作れない。

人の能動時間は人が自分の shell で区間を記録する。

```sh
bin/ops-r2-human-span.sh begin --state spans --logical-op approve-1
bin/ops-r2-human-span.sh end --state spans --logical-op approve-1 --campaign-bin <celeris-campaign> \
  --file human-ops.jsonl --actor human:<名前> -- --campaign <id> --manifest <manifest> --condition <c> --case <case> \
  --rep <n> --op-kind safety_approval --intent safety
```

`end` は開始と終了を 1 行の `human-op record`（`actor.kind=human`、`time_basis=evaluator_live`）に渡す。
begin の無い end は拒否する。web の承認のような点は長さ 0 として足さない。

## 3. 試行の後: session-map と消費

```sh
celeris-campaign human-op list --file human-ops.jsonl >human-ops-list.json
python3 scripts/campaign/ops-r2-session-map.py final --base session-map.base.json --trials session-trials.json \
  --human-ops-list human-ops-list.json --out session-map.json
```

この session の介入（`actor` が `external_operator` で id が session id の op）で、その試行の observe・prep の区間を分け、
`intervene` の区間を作る。同じ入力なら byte 一致。次のときは作らずに exit 2 で止まる: 介入の開始か終了の時刻が無い、
介入が自分の試行の prep・observe の区間 1 つに収まらない、trial_key が `session-trials.json` に無い、condition_id が違う。

transcript は `ticket.json` の `transcript_expected`・`subagents_expected` から [operator-usage](operator-usage.md) の
手順で snapshot し、親と subagent をすべて `celeris-campaign operator-usage --session-map session-map.json` で取り込む。
同じ message は dedup_key で 1 回だけ数える（同じ snapshot や途中の snapshot を再び取り込んでも added=0、内容は変わらない）。
区間外の応答は `shared_unattributable` になり、試行に配分しない。

```sh
python3 scripts/campaign/ops-r2-session-map.py summary --trials session-trials.json \
  --usage operator-usage.jsonl --snapshot <snapshot dir> --out operator-usage-summary.json
```

`(category, trial_key)` ごとの合計を、condition_id・request_ref と並べて書く。取れない消費は 0 にしない。
`main.jsonl` が無いときは全部 null で `reason=transcript_missing`、この session の行が無いときは `not_recorded`、
usage の欠けた message を含む組は token が null で `reason=usage_missing:<件数>`。

## 確かめていること

- `crates/campaign/tests/ops_r2_session.rs`: 開始前の準備物、親と subagent の再取り込みで合計が変わらないこと、
  区間外は `shared_unattributable`、欠けた消費は null と理由、終了の無い介入・他試行にかかる介入で session-map を作らないこと。
- `scripts/campaign/test_ops_r2_session.py`: poll の記録と止まる条件、人の区間の記録。
- 実 Claude Code での起動（`launch.sh`）と transcript の実 path は未確認。初回の起動で `transcript_expected` と実物を照合する。
