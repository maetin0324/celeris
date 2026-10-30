# 外部 operator の transcript 保全と消費取り込み

測定中に評価者が動かす Claude Code の消費を、内部 worker 消費とは別の
`operator-usage.jsonl` に残す。対象は operator の session だけとし、Celeris の管理 run や
比較腕 A の harness transcript は取り込まない（二重計上防止）。既に消えた transcript は復元・推定できない。
設計は [ADR](../../agent-docs/adr/2026-10-27-operator-usage.md)。

## 測定前に人が行う設定

1. 使用する Claude Code の版・設定 source・project dir・測定 ID を起動票へ記録する。
2. 人が有効な settings.json（通常 `~/.claude/settings.json`、別の設定 dir を指定している場合はその設定）を
   バックアップし、他の設定を保ったまま `"cleanupPeriodDays": 3650` を追加する。
   測定と監査の期間を覆う日数にする。0 は使わない。
   ローカル Claude Code 2.1.303 の設定定義で、既定 30 日・最小 1・長期保持に 3650 を使えることを確認済み。
   別の版ではその版の設定定義を確認する。設定 source の無効化や project 設定の上書きにも注意する。
3. 次に人が起動した session で設定エラーが無いことを確認する。保持期間延長だけに頼らず、以下のコピーを毎回行う。
   測定を追跡する session で transcript 書き込みを無効化しない。

この作業は本番設定を変更するため人が実施する。本コマンドは設定変更、Claude Code の起動、LLM/API 呼び出しを行わない。

## session ごとの証跡コピー

終了した session を対象に、評価者が実在する project dir を確認して指定する。
通常は `~/.claude/projects/<proj>/<session>.jsonl`。
subagent の `<session>/subagents/*.jsonl` も必要。以下は一度きりの snapshot を作り、再取得は別の snapshot 名にする。

```sh
umask 077
project_dir="$HOME/.claude/projects/<実在するproject-dir>"
session='<session UUID>'
evidence='/測定証跡の絶対パス/operator'
snapshot='終了時刻などの一意名'
dest="$evidence/$session/$snapshot"
mkdir -p "$evidence/$session"
mkdir "$dest" || exit 1
cp -- "$project_dir/$session.jsonl" "$dest/main.jsonl" || exit 1
if [ -d "$project_dir/$session/subagents" ]; then
  cp -R -- "$project_dir/$session/subagents" "$dest/subagents" || exit 1
fi
(cd "$dest" && find . -type f -name '*.jsonl' -exec sha256sum {} \; > SHA256SUMS)
(cd "$dest" && sha256sum -c SHA256SUMS)
```

起動票、Claude Code 版、コピー日時、元の絶対パス、session-map とその digest、取り込みコマンドと結果も同じ証跡に保存する。
長時間 session は途中にも別 snapshot を保存し、終了後の snapshot で取り込みを行う。
書き込み中のコピーは行末が切れる可能性があり、取り込みは不正 JSON を拒否する。元の証跡を編集して直さない。
transcript 本体は会話・コマンド・秘密を含み得るためアクセスを評価者に限定する。
公開用には消費行だけを使い、元 transcript をリポジトリへコミットしない。

## 評価者が書く session-map

JSON 配列。`trial_key` は対象の `ledger.jsonl` の `idempotency_key`（既存 TrialKey 由来）をそのまま写し、
`trials.jsonl` の campaign・config・problem・rep・trial_no と照合する。
区分は prep（準備）、observe（観測）、intervene（介入）、shared（試行に割れない共有作業）、
unknown（結果に影響したか断定できない支援。例: 試行の後の一般的な修復で、試行の結果に効いたかを根拠で決められないもの）。
shared の trial_key は null、prep・observe・intervene は空でない試行鍵が必須。unknown の trial_key は分かれば試行鍵、
分からなければ null。断定できない支援を shared や observe に寄せない（ADR 2026-10-30-ops-r1-human-time-coverage）。
区間そのものが分からない応答は対応行を書かない（shared_unattributable になる）。
以下の trial_key は説明用の値なので、取り込み前に実際の試行鍵に置き換える。

```json
[
  {"session_id":"session-1", "start":null, "end":"2026-10-27T10:00:00Z",
   "category":"prep", "trial_key":"<実際の試行鍵>"},
  {"session_id":"session-1", "start":"2026-10-27T10:00:00Z", "end":"2026-10-27T11:00:00Z",
   "category":"observe", "trial_key":"<実際の試行鍵>"},
  {"session_id":"session-1", "start":"2026-10-27T11:00:00Z", "end":"2026-10-27T12:00:00Z",
   "category":"intervene", "trial_key":"<実際の試行鍵>"},
  {"session_id":"session-1", "start":"2026-10-27T12:00:00Z", "end":null,
   "category":"shared", "trial_key":null}
]
```

start/end は RFC3339（時差付き可）の半開区間 `[start,end)`。null は無限。
同じ session の区間重複・逆転・未知の欄や category はエラーになる。
subagent も transcript 自身の sessionId で対応させる。親の sessionId と同じなら同じ対応表を使う。
異なるなら独立に対応を書く。model は応答から読み、起動票の申告値で補わない。

## 取り込みと再現

```sh
celeris-campaign operator-usage --transcript "$dest/main.jsonl" \
  --session-map "$evidence/session-map.json" --out "$evidence/operator-usage.jsonl"
for transcript in "$dest"/subagents/*.jsonl; do
  [ -f "$transcript" ] || continue
  celeris-campaign operator-usage --transcript "$transcript" \
    --session-map "$evidence/session-map.json" --out "$evidence/operator-usage.jsonl" || exit 1
done
```

`--out` はファイル名（省略時は session-map の隣の operator-usage.jsonl）。親 dir は先に作る。
manifest・daemon・認証は不要。成功時の stdout は `added`（追加行数）と `out`。
同じ証跡を再取り込みすると added=0、行数も内容も変わらない。エラーは非ゼロ終了。
出力隣の `.lock` は同時取り込みの排他用であり、取り込み中に消さない。

1 行は schema=`celeris.operator-usage/1`、dedup_key=`op:<sessionId>:<message.id>`、
session_id、message_id、model、is_subagent、agent_id、start/end、category、trial_key、
input_tokens、output_tokens、cache_read_tokens、cache_creation_tokens を持つ。
start/end は同じ message の最初/最後の transcript timestamp であり、生成時間や session 全体の経過時間ではない。
時刻が欠ける message は start/end とも null。usage または token 欄が無い応答は該当欄を null にし、0 と区別する。
同じ message の累積 snapshot は各 token 欄の最大値を一度だけ計上する。

応答区間全体が一つの対応区間に収まらないもの、未登録 session、欠測時刻は
`category=shared_unattributable, trial_key=null`。均等配分しない。
shared・shared_unattributable・unknown は個別試行の消費に混ぜず別表に残し、集計時も null を 0 に置き換えない。
これは外部 operator の評価記録で、内部 DecisionLogUsage や既存 final-accounting への自動加算は行わない。

同じ dedup_key で内容が違えば conflict としてファイル全体の更新を拒否する。
途中 snapshot の更新や対応表の訂正では、旧証跡・旧出力を残し、訂正理由を記録し、
確定した全 transcript と新版対応表から新しい出力ファイルへ再生成する。旧版と新版を加算しない。
