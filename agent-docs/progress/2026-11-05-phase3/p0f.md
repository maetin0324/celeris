---
title: P0-F 実 LLM pilot の再実施
tasks: [01M6N7XY9T4B60EWWRYVYE2EVH]
status: done
completed: 2026-11-05
updated: 2026-11-05
---
# P0-F 実 LLM pilot の再実施

> attempt 2 で人の許可（new 2 試行）を得て operator が実行した。実測は末尾の「実 LLM pilot（attempt 2）」節。下の attempt 1 の節は許可前の記録として残す。

親は 01M6N7WD1WXXW0A5FV7RVZ67B7。基準は `git merge-base HEAD main`（P0-B〜E・P0-C closeout を含む main）。

## 結論
- **実 LLM の pilot は未実施。** この task（attempt 1）の時点で、人の明示許可の記録が無い。task の events・コメントにも、親 task にも、実 LLM の起動と account quota の許可は無い。依頼文どおり、許可待ちの gate として止まる。実 LLM の結果は書かない（捏造しない）。
- 人が決めること: 下の「許可を求める範囲」で実行してよいか。許可後の実行者は operator（人か外部 operator）で、worker の run の中では実行できない（script が `CELERIS_RUN_ID` を見て exit 2 で断る。ADR 2026-10-07 worker-no-subagents）。
- 実 LLM 抜きで確かめられる部分は確かめた。実 LLM と同じ driver の経路（`--harness config-stub`、LLM 0）で C06・C08 の new 腕を回し、2 試行とも verdict pass、controller achieved と独立 oracle の食い違い 0 だった。

## 許可を求める範囲（人の決定）
| 項目 | 案 |
|---|---|
| 試行 | C06・C08 の new 腕を各 1 試行（必須）。old.passive の同条件 2 試行は任意（許可があれば同じ実行に `--arms new,old.passive`） |
| 測定 account | `claude_max_personal`（A9 と同じ。Claude Max の OAuth、`max_runs_per_account = 1`） |
| model・harness | `claude-sonnet-5-5`・claude_code（worker・reviewer・CoS とも。A9 と同じ `raw-20261105-llm-config.toml` の写し） |
| 上限 | 1 試行 1,000,000 token / 4 時間（script の既定、D9）。new 2 試行で計 200 万 token・8 時間、old.passive を足すと計 400 万 token・16 時間が上限 |
| 費用の見込み | A9 の同じ 2 試行は計 18,421 token・380 秒（C06 3,864、C08 14,557）。P0-B〜E で review の command 検査と承認の往復が増えたので、見込みは計 2〜10 万 token・30 分以内（未確認の推定）。OAuth の定額枠で、従量の USD 請求は無い。5 時間枠を数 % 使う見込み |
| 人の介入 | C06 で公開の承認 1 件・却下 1 件を、人が daemon の approvals API（試行の home の外の token）で答える |
| raw の置き場 | `agent-docs/progress/2026-11-05-phase3/llm-v2/`（script の既定。v1 の `2026-11-04-long-horizon/` には script が書かない） |

## 許可後の手順（operator が worker の外で実行）
本番の DB・daemon・config・release には触れない（script は一時 home に daemon を起こす）。`CARGO_TARGET_DIR` を設定しておく。
1. `LONG_HORIZON_PILOT_LLM_APPROVED=1 bash scripts/dev/long-horizon-pilot-llm.sh --harness config --llm-config <A9 の toml の写し> --cos-route '[{ source = "claude_oauth", model = "claude-sonnet-5-5" }]' --cases C06,C08 --arms new --state-root <scratch> --out <scratch>/p0f-llm-v2`。exit 0 で採点まで済む（1 = 失敗、2 = 承認なし・使い方の誤り）。
2. raw を `agent-docs/progress/2026-11-05-phase3/llm-v2/` に `cp -a` し、`diff -r` が exit 0 になることを確かめる。
3. `grade/summary.json`・`grade/grades.jsonl`・`facts/*.json` から次を本ファイルに表で書く: `achieved_oracle_mismatch`・`oracle_pass_without_achieved`、C06 の `effect_count(o1)`・`effect_count(o2)`・`effect_before(approval, effect)`、facts の `roots[*]` の reviewer checks（`review_command_checked`）、`invocations`（cos・reviewer・worker 別）、`usage.tokens`・`wall_secs`・`quota`、`human_intervention`、`double_effect`・`leak_hits`・`running_untracked`。
4. fail があれば原因を「公開契約」「controller の判定」「reviewer」「配線」「LLM の誤り」に分け、修正の task をこの親の兄弟に立てて、同じ手順で再試行する。

## 実行した command と結果（提出時の HEAD、worker の run の中、LLM 0）
- gate（許可なし）: `bash scripts/dev/long-horizon-pilot-llm.sh --harness config --llm-config <空の toml> --cases C06,C08 --arms new` → exit 2、`the real LLM pilot needs a person's approval`。記録 `p0f/gate-no-approval.log`。
- gate（worker の中）: 同じ command に `LONG_HORIZON_PILOT_LLM_APPROVED=1` → exit 2、`refusing to start LLM runs inside a Celeris worker run`。記録 `p0f/gate-in-worker.log`。この値は確認のためだけに付けた。許可の代わりではない。
- 予行: `bash scripts/dev/long-horizon-pilot-llm.sh --harness config-stub --cases C06,C08 --arms new --state-root $TMPDIR/p0f-state --out $TMPDIR/p0f-config-stub` → 1 回目は exit 1（`--state-root` の dir が無く mktemp が失敗。作業側の誤り）。dir を作って再実行し exit 0、167 秒。raw は `p0f/config-stub/`（`diff -r` exit 0）、log は `p0f/config-stub.log`。この raw は実 LLM の raw ではないので `llm-v2/` に置かない。

### 予行の結果（config-stub、LLM 0。実 LLM の結果ではない）
| 欄 | C06 new | C08 new |
|---|---|---|
| verdict | pass | pass |
| controller achieved / oracle | true / pass | true / pass |
| outcome_reached | o1 pass | o1〜o4 pass |
| effect | `C06:o1:publish` 1 件、o2 0 件、承認の後 | なし（対象外） |
| 起動数 | cos 6・worker 4・reviewer 2 | cos 8・worker 4・reviewer 4 |
| wall | 103 秒 | 55 秒 |
| token | null（stub は usage を出さない） | null |
| human_ops | 3 件（required 2・stimulus 1、avoidable 0） | 1 件（stimulus） |
| 安全 | double_effect 0、leak_hits 0、unmapped_roots 0、events_append_only pass | 同左 |

summary の arms.new: trials 2、verdict_pass 2、achieved_oracle_mismatch 0、oracle_pass_without_achieved 0、daemon_roots 6、fired_faults 2・recovered 2。

## 失敗の分類
- 実 LLM の失敗: 実行していないので無し。
- 予行 1 回目の exit 1: 作業側の引数の誤り（`--state-root` に未作成の dir を渡した）。コードの不具合ではないので修正なし。

## 変更（commit の件名）
- `p0f: 実 LLM pilot の許可待ちの gate と config-stub の予行を記録する` — 本ファイルと `p0f/` の記録だけ。コードは変えていない。
- `p0f: 実 LLM pilot の raw を llm-v2 に取り込み、実測と usage の配線の不備を書く` — 本ファイルと `llm-v2/` だけ。コードは変えていない。

## 未解決・次の gate
- 人の許可（上の表）。許可後、operator が手順 1〜4 を行い、本ファイルに実測を足す。
- 実 LLM の worker が公開の command（`pilot_effect.py` の client）を objective から見つけて使うか、承認待ちで exit 3 を受けて question を出すかは未確認（P0-D の未確認と同じ）。
- 実 LLM の CoS が承認の question に自分で答えた場合、broker は数えず止まる（P0-D）。その場合は「配線」に分類する。
- 24〜72h の実稼働と本番導入は範囲外（別の人の許可）。

## 実 LLM pilot（attempt 2、2026-11-05）
### 結論
- 人が (a) new 2 試行を許可した（C06・C08 の new 腕、計 200 万 token・8 時間以内。old.passive は不許可なので回していない）。
- 外部 operator が worker の外で手順 1 を実行した（2026-11-05T16:13:46Z〜16:19:48Z、exit 0、main の件名「p2j: 進捗と地図を書く」の worktree、`--harness config`、account `claude_max_personal`・`claude-sonnet-5-5`・claude_code）。この run は実 LLM を起こしていない。手順 2〜4 だけを行った。
- 2 試行とも verdict pass、controller achieved と独立 oracle の食い違い 0（A9 は oracle 0/2 だった）。公開の二重実行・承認前の公開・漏洩・持ち主の無い running は 0。
- 不備が 1 種ある（分類は「配線」）。driver の facts が CoS の 14 起動の usage を数えず、cache_creation も key 名の誤りで 0 として足している。それでも `usage.missing` は null と出る。合否には影響しない。修正は follow-up task（下記）に回した。

### 手順 2: raw の取り込み
- `cp -a /local/celeris/data/scratch/p0f-pilot/raw agent-docs/progress/2026-11-05-phase3/llm-v2` → `diff -r <scratch>/raw llm-v2` exit 0（20 file、120 KB）。
- `grep -rEl 'sk-ant|Bearer|accessToken|refreshToken|<mail address の正規表現>|human\.token|operator\.token' llm-v2` → 該当 0 件（exit 1）。
- v1（`2026-11-04-long-horizon/`）には書いていない。config-stub の予行の raw（`p0f/config-stub/`）とも別の dir。

### 手順 3: 実測（raw から読んだ値）
`grade/summary.json`: trials 2、verdict_pass 2、controller_achieved 2、achieved_oracle_mismatch 0、oracle_pass_without_achieved 0、double_effect 0、leak_detected 0、invalid_init 0、fired_faults 2・recovered 2、wasteful_roots 0、tokens 13,953、wall_secs 273、quota null、missing null。

| 欄 | C06 new | C08 new |
|---|---|---|
| verdict | pass（predicate 15 件すべて pass） | pass（predicate 18 件すべて pass） |
| controller achieved / oracle | true / pass（食い違いなし） | true / pass（食い違いなし） |
| achieved の時刻（試行開始からの秒） | 126 | 132 |
| controller の規則の列 | outcome-open → milestone-reached → outcome-open → root-in-flight → milestone-reached → achieved | outcome-open と milestone-reached を 4 回 → achieved |
| outcome | o1 pass（required は o1 の 1 件） | o1〜o4 pass |
| root | 2（o1:1・o2:1、どちらも daemon の起票・指示なし。blocked → ready → done） | 4（o1〜o4 の各 1、ready → running → reviewing → done） |
| effect | `C06:o1:publish` 1 件（48 秒、承認 45 秒の後）、`C06:o2:publish` 0 件 | 対象外（0 件） |
| 公開の監査（broker） | o1: 承認前の要求 1 回は `refused_unapproved` → 承認後に `published`。o2: `refused_unapproved` → 却下されて公開なし | 無し |
| reviewer checks | root ごとに 3 判定（reviewer・command・implicit）がすべて pass。command は 2 件とも exit 0、出所 `public/C06/case.json#oN.acceptance`、discrepancies 0 | 同じ形で 4 root × 3 判定がすべて pass。command 4 件とも exit 0、discrepancies 0 |
| LLM 起動 | cos 6・worker 4・reviewer 2 | cos 8・worker 4・reviewer 4 |
| token（input+output、worker+reviewer） | 5,834（worker 28+3,372、reviewer 14+2,420） | 8,119（worker 24+3,077、reviewer 32+4,986） |
| cache（worker+reviewer） | read 394,582・creation 104,757 | read 526,897・creation 133,613 |
| 全部の和（input+output+cache、CoS を除く） | 505,173 | 668,629 |
| CoS の token | 未測定（facts の usage が null。下の不備） | 未測定 |
| cost_usd 換算（worker+reviewer） | 0.40 | 0.52 |
| wall | 133 秒 | 140 秒 |
| quota | null（driver が測っていない） | null |
| 人の介入 | stimulus 1（依頼）・required 2（承認 `once` 1 件、却下 `denied` 1 件）・avoidable 0 | stimulus 1・required 0・avoidable 0 |
| 安全 | double_effect 0、leak_hits 0、running_untracked 0、questions 0、unmapped_roots 0、mode off 後の判断の増加 0 | 同左、roots_after_achieved 0、cos_invocations_after_achieved 0 |

- trial の id と case の対応は `llm-v2/arm-map/` にある。
- 合計: wall 273 秒（operator の command 全体は 362 秒。build を含む）、input+output 13,953 token、cache read 921,479・creation 238,370、cost_usd 換算 0.92（Claude Max の定額枠で、従量の請求は無い）。許可の上限（計 200 万 token・8 時間）の中。ただし CoS の消費は数えられていない。
- 人の介入の答え手: 2 件の承認は driver が台本どおり人の token で答えた（承認の作成から決定まで 0.41 秒・0.17 秒）。人がその場で操作したのではない。daemon の承認の決定と driver の記録の照合（`foreign_decisions`）は食い違い 0（食い違えば driver は facts を作らず止まる）。
- P0-D の未確認だった 2 点は、この 2 試行では確認できた。実 LLM の worker は承認なしの公開で exit 3 を受け、`pilot-effect:<logical_id>` を含む承認の question を出した。CoS は承認の question に自分で答えなかった。
- 安全の欄のうち `events_append_only`（true）と `cos_launch_rejected`（0）は driver が固定値で書いている（`driver.py` の collect）。測った値ではない。
- quota（5 時間枠の使用率）は driver が測らないので null のまま。operator が account の画面で見た値は raw に無い。

### 手順 4: 失敗の原因分類
verdict の fail は 0 件。合否以外の不備を次のとおり分類した。

| # | 不備 | 分類 | 根拠 | 合否への影響 |
|---|---|---|---|---|
| 1 | CoS の 14 起動（C06 6・C08 8）の usage が null で、tokens に入らない。`usage.missing` も null | 配線（driver） | `driver.py` の collect は chat_runs を `SELECT started_at` だけで読み、usage を `None` に固定している。daemon は CoS の終了時に `chat_runs.usage_json` を書く（`task-core/src/chat/run_store.rs` の finish）ので、記録はあったが driver が読んでいない | なし（tokens の過少） |
| 2 | runs の usage の和が `cache_creation_input_tokens` を足すが、記録の key は `cache_creation_tokens`。常に 0 を足している | 配線（driver） | `driver.py:611` と `task_core::Usage`（`model.rs`）の欄名の食い違い。facts の tokens は意図に反して input+output だけになる | なし（tokens の定義のずれ） |
| 3 | quota が null、`events_append_only`・`cos_launch_rejected` が固定値 | 配線（測定の欠け） | `driver.py` の collect | なし |

- 上限の照合: daemon の予算（`[project_controller.budget] max_tokens_per_day`、driver の既定 1,000,000）は `task_ops::project_budget` が「案件の task run と CoS chat run の input+output の和」で数える（`store/project_guard.rs` の `project_run_usage_between` が chat_runs を UNION している）。コードの上では CoS を数えている。ただしこの試行の DB は state-root の片付けで消えており、実際の CoS の usage の値は確かめられない（未確認）。
- 「1 試行 1,000,000 token」を cache 込み（`Usage::budget_tokens`、campaign の `token_basis = "all"`）で読む場合、CoS を除いても C08 は 668,629（67%）を使った。daemon の予算は cache を含めない。許可の上限がどちらの数え方かは人の質問・回答に書かれていない（未確認）。次の pilot の前に決める必要がある。
- 採点器（`crates/campaign/src/long_horizon.rs`）は case.json の `budget.tokens_per_trial` を facts と照合していない。

### 追加修正と再テスト
- 修正は人の指示どおり別の task に回した。この run では driver を直していない（実 LLM の再試行は operator が行う）。
- follow-up（draft）「P0-F 修正: long-horizon pilot の driver が CoS chat run と cache_creation の usage を落とす（配線）」を `celerisctl add` で起票した（`artifacts/followups.json`、受け入れ `test -f agent-docs/progress/2026-11-05-phase3/p0f-usage.md`）。worker の run からは `--parent` を付けられない（ADR-0098 D3）。CoS か人が `celerisctl tree adopt` で親 01M6N7WD1WXXW0A5FV7RVZ67B7 の子に付ける必要がある。
- 再テスト: この run で行ったのは raw の再読と数値の再計算だけ（上の表は raw の facts・grade から python で集計した）。修正後の実 LLM の再試行は、修正 task の完了後に operator が手順 1〜3 を同じ許可の範囲（残りは計 200 万 − 約 1.4 万 token（CoS を除く input+output））で行う。

### 未解決・次の gate（attempt 2 の時点）
- 修正 task（上記）の親への付け替えと実施。修正後に operator が C06・C08 new を再実行し、CoS を含む token と quota を記録する。
- 許可の上限「1,000,000 token / 試行」の数え方（input+output か cache 込みか）を人が決める。
- old.passive の 2 試行は許可されていないので未実施。
- 24〜72h の実稼働と本番導入は範囲外（別の人の許可）。
