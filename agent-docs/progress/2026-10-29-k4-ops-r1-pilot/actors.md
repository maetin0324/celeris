---
title: K4 ops-r1 pilot 主体と論理要求の突合（human-ops /2 と request ledger）
tasks: [01M62S8XWCZDCY2ZBVQ35176MH]
status: done
updated: 2026-10-30
completed: 2026-10-30
---
# K4 ops-r1 pilot 主体と論理要求の突合

状態: 完了。外部 operator が実行した A-t001・B-t001 の raw と測定 daemon の DB の写しを読み、human-ops /2・request ledger・B の DB（chat・CoS run）・ops-r1-intake・stop.json の主体を突合した。DB は読み取り専用で開き（`mode=ro&immutable=1`。ledger は複製に対して実行）、`db-copy.sha256` の照合は OK。LLM・daemon は起動していない。

成果物（repo の外）: `/local/celeris/data/workspaces/01M62S8XWCZDCY2ZBVQ35176MH/artifacts/ops-r1/post/`

- `actors.json`（ops-r1-actors/1）: requests 7 件・actor_kind_counts（出典別）・mismatches 7 件・human_active_secs・human-ops の区間
- `actors.py`: 生成 script（読み取り専用。同じ入力で 2 回生成し `cmp` で一致）
- `request-ledger.json`・`request-ledger.all-time.json`: `celerisctl eval requests` の出力（測定 daemon と同じ release の celerisctl。版は固定票）
- `actors.SHA256SUMS`

## 結論

- request ledger は 0 件（2026-10-30T06:00〜08:00Z も全期間も）。測定 DB の受信箱の件・decision・approval・CoS operation も 0 件。B の CoS が task を作らず自分で分析したので、planner・worker・reviewer・受信箱の主体は観測されていない。これは「支援が無かった」の証拠ではない。
- human-ops /2 は 26 行すべて actor・executed_by が external_operator（runbook を実行した Claude Code の session。id は raw/operator/README.txt）、decided_by が human:rmaeda。request_ref・revision は全行 null で、ledger 0 件と矛盾しない。
- 人の能動時間（計測した区間の和集合）は 0 秒（人が実行した区間が 0 件）。人の承認（approve-paid、2026-10-29T03:00:34Z）・実行の指示・trials-done の回答は区間を測っていないので、`complete: false` とした。外部 operator の区間の和集合は 114 秒（A 1・B 60・shared 53。開始と終了が同じ秒の点の記録 8 件を含む）。問合せ待ちの区間は 0 件。

## 主な不一致（actors.json の mismatches）

- intake-executor: B の依頼 message は DB と ops-r1-intake で author=human:rmaeda だが、送信を実行したのは外部 operator（人の principal の token、credential=unknown）。
- stop-decided-by: stop.json は decided_by=external_operator、human-ops の b-stop は decided_by=human。
- decided-by-blanket: decided_by=human は包括の指示（runbook を実行して）を表すだけで、個々の判断の主体ではない。
- cos-question-not-in-ledger: 案件 CoS は最終報告の本文で「入力 tgz が違うなら知らせて」と問うたが、受信箱の件を作らず ledger に現れない。未回答。
- missing-protocol-records: a-stop・A/B の保存・post-7.3/7.4 の human-ops 行が無い（not_recorded として扱い、能動時間に入れない）。
- actor-id-format・executor-subagent: operator の id の接頭辞の揺れ、実行は親 session の subagent（raw/operator/README.txt）。

## unknown と片側だけの要求

- trials-done の回答: 回答した主体と時刻は本番 daemon の events にあり、この WU の入力に無いので actor.kind=unknown。
- approve-paid・実行の指示: 本番 daemon の decision と operator の親 transcript にだけあり、raw には human-ops の note しかない。
- A の起動: A は Celeris を通らないので ledger に行が無い（設計どおり）。A の Claude Code の問い合わせは 0 件。

## 証拠

- `sha256sum -c db-copy.sha256`（raw/B で実行）: OK
- `celerisctl --db <複製> eval requests --since 2026-10-30T06:00:00Z --until 2026-10-30T08:00:00Z`: exit 0、0 requests
- `celeris-campaign human-op validate --file raw/human-ops.jsonl`: ok 26 records
- `python3 -I actors.py .. actors.json` を 2 回、`cmp`: 差なし
- `celeris-campaign human-op report` は `--events`（raw/pipe/out、grade-pipeline WU の出力）が要るので、この WU では実行していない。和集合は actors.py が計算した。
- 検査は提出時の HEAD で実行（結果は報告に書く）

## 未解決

- trials-done の回答主体（本番 daemon の events で確認する）。
- human-ops に人が実行した区間が無いので、人の能動時間は測っていない。次の試行では、人の承認・回答を `human-op record --actor-kind human` の区間で残す。

## 提案

- ops-r1-intake の `--send` を外部 operator が打つときは、intake 記録の actor に executor を別欄で残す（今は token の principal＝人だけが残る）。
- CoS が chat の本文で人に問うた場合も request ledger に載る経路（受信箱の件を作る、または ledger が chat の問いも拾う）を検討する。
- human-ops の decided_by は、包括の委任と個々の判断を区別できる書き方（例: decided_by=external_operator、note に委任元）を runbook §6 に足す。
