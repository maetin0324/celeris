---
tasks: [01M6RGZRNZQMF4PG39HRQ81NRV]
---
# ADR 2026-11-06: CoS run の起動境界・枠待ち・試行の終わりの待機を決め、起動前に確かに終わった run だけを費用不明から外す

- 状態: 採用（設計）。この ADR ではコードを変えない。実装は後続の task が行う。
- 決めた人: task 01M6RGZRNZQMF4PG39HRQ81NRV の worker。[ADR 2026-11-06-budget-usd-caps](2026-11-06-budget-usd-caps.md) D2（費用不明は fail closed）、
  [ADR 2026-11-06-phase5-n0-usd-accounting-contract](2026-11-06-phase5-n0-usd-accounting-contract.md)（欠測を 0 として足さない）、
  [ADR 2026-11-06-phase5-n1-pilot-measurement-contract](2026-11-06-phase5-n1-pilot-measurement-contract.md)（G7 は judge の出力だけを受ける）の範囲で決めた。
- 関係: [ADR-0089](0089-cos-runs-bypass-concurrency.md)（CoS は枠の外、実 account は +1）、
  [ADR 2026-10-08-account-concurrency-limits](2026-10-08-account-concurrency-limits.md)（account ごとの上限・暗黙 account は +1 しない）、
  [ADR 2026-11-05-phase4-cos-usage-empty-route](2026-11-05-phase4-cos-usage-empty-route.md)（D0 で訂正する）、
  [破壊的変更](2026-10-11-breaking-changes-by-default.md)。
- 観測の記録: [G7 1 回目の contract](../progress/2026-11-06-phase5/g7-run1/contract.md)（raw と manifest を同じ所に置いた）。

## 背景
G7 の 1 回目（approval `n1-g7-20261106-1`、C06・C08 の new 腕、`--cos-route` と CoS の skill あり）は exit 3、verdict unknown・g7 fail だった。
案件 CoS の chat run 5 本のうち 3 本の usage が null で、USD が分かったのは 12 本の起動の 0.9802 USD、wall は 234 秒。
C06（135 秒、採点 pass）は achieved の後の at 134 に始まった CoS run が null。C08（99 秒、採点 fail）は at 72・at 91 の CoS が null で、
`cos_run_failures` に「no declared route candidate for harness claude_code is eligible」が 1 件あり、案件は `pc/guard/budget`
（質問 `budget:usd_unknown`）で止まった。

コードを読むと、null は次の 2 つの経路から出る（根拠は contract の「原因の根拠」）。
1. CoS run は claim の時点で `started_at` を持つ。route を選べないと `fail_routes` が usage 無しで行を閉じる。
   `project_run_usage_between` は chat run を `finished_at` があれば全件返すので、起動しなかった run が費用不明になり、案件を止める。
2. driver は案件の phase が終わると mode を off にして daemon に SIGTERM を送る。最後の操作の後に始まった CoS run の終わりと
   usage の書き込みを待たない。

null 3 本のうち、どれがどちらの経路かは一部未確認（一時 DB が残っていない）。この ADR は 2 つの経路をどちらも閉じる。

## 決定

### D0 2026-11-05 の判断の訂正
[ADR 2026-11-05-phase4-cos-usage-empty-route](2026-11-05-phase4-cos-usage-empty-route.md) の D1「daemon は直さない」と付記の
「daemon 側の修正は不要と確定した」は、今回の観測で誤りと分かった。fake adapter の試験が固定したのは「起動した CoS run の usage が
案件の和に入る」ことだけで、起動前に失敗した行と、試行の終わりで切れた行は試験していなかった。route 候補が空という原因（D2・D3）は
正しく、driver の `--cos-route` の検査は残す。daemon は D1〜D2 と D4〜D5 のとおり直す。旧 ADR には訂正の付記を足す。

### D1 起動境界と費用不明からの除外（fail closed）
1. **起動の印**: `chat_runs` に `launched_at TEXT NULL` と `prelaunch_reason TEXT NULL` を足す（migration）。
   - `launched_at` は、route と account が決まり、skill の配置と run credential の発行が済んだ後、harness の process を spawn する
     **直前**に、spawn と同じ関数で書く。書けなければ spawn しない。spawn が失敗しても `launched_at` は残す（起動したかもしれない側に倒す）。
   - route の failover（`retry_routes`）で 2 本目以降の route を起動するときは `launched_at` を書き換えない（最初の起動の時刻）。
   - `prelaunch_reason` は、`launched_at` が NULL のまま run を閉じる経路だけが書く。値は閉じた列挙（`route_unavailable`・
     `skills_unavailable`・`wall_expired_before_launch`・`slot_wait_expired`（D2）・`daemon_restart_before_launch`（D4）・
     `stopped_before_launch`）。人向けの文は従来どおり `reason` に書く。
2. **除外の条件**: 予算の和（`project_run_usage_between`）は、次の 3 つを**すべて**満たす chat run だけを「起動前に終わった run」として返さない。
   - `finished_at` がある、`launched_at` が NULL、`prelaunch_reason` が列挙のどれか。
3. **費用不明に倒すもの**: 上の条件を満たさない終わった chat run で usage が無いものは、従来どおり費用不明 1 件。具体的には
   - 起動後（`launched_at` あり）で usage が無い・読めない行。harness が途中で殺された行（interrupted・stopped・failed）を含む。
   - 旧記録（migration より前の行。`launched_at` も `prelaunch_reason` も NULL）。
   - `launched_at` が NULL でも `prelaunch_reason` が無い・列挙に無い行（境界が不確か）。
4. **既存行の扱い**: migration は既存行の `launched_at`・`prelaunch_reason` を埋めない（推測で起動前と決めない）。
   既存行の `reason` の文面から起動前を推定する backfill もしない。よって旧記録の usage null は引き続き費用不明として数える。
5. task run の除外規則（budget-usd-caps D2-3、usage・model・adapter などが全部無い行）はこの ADR では変えない。

### D2 CoS の枠待ち
1. route の選択が失敗し、その理由が**枠（同時実行数）だけ**のとき、CoS run は失敗にしない。行は claim のまま `launched_at` NULL で残し、
   `chat_run_status` を Waiting（文面「CoS の枠の空き待ち」）にして、次の tick で同じ route の順に選び直す。新しい run・新しい入力は作らない。
2. 「枠だけ」の判定は `RouteTrace` の候補ごとの除外理由で決める（決定的、LLM なし）。除外された全候補が次のどれかなら枠待ち。
   - **実 account の枠**: 実 account（OAuth のプール）の `in_use`（CoS を含む全数）が `effective_max_runs + 1` 以上（ADR-0089 規則 2、
     ADR 2026-10-08 (c)）。
   - **暗黙 account の枠**: 暗黙 account（プールを持たない source）の `in_use` が `effective_max_runs` 以上。暗黙 account には +1 しない
     （ADR 2026-10-08 (c)・(e)）。
   - **CoS 全体の上限**: `[execution] max_cos_runs` に達している（従来どおり待つ）。
3. 次の理由が 1 つでもある候補は枠待ちにしない。全候補がこれらで除かれたら、従来どおり `route_unavailable` で閉じる。
   - source の上限・状態: source の cooldown（暗黙 account の cooldown を含む）、quota の枯渇、CoS の 5 時間枠の予約（Claude の予約）、
     未ログイン・auth 失敗、route 候補の未宣言・model の不一致。
   - 理由: これらは数秒〜数十秒で解けると限らず、待つと D3 の期限まで黙って止まる。人には `route_notice` の文面で理由が残る。
4. 枠待ちの期限は D3 の共有の期限と CoS run の `max_wall_secs`（`started_at` からの経過を引いた残り。現行の `routes.rs`）の早い方。
   期限に達したら `prelaunch_reason = slot_wait_expired` で閉じる。起動していないので D1-2 で費用不明に数えない。
5. 観測: 枠待ちの開始と終わりを `chat_run_status` の段（Waiting → Running）で残し、`GET /providers` の `in_use_cos` は起動した run だけを数える。

### D3 試行の終わりの待機（drain）と共有の期限
1. **共有の期限**: driver は承認の file から 1 本の期限 `deadline_at` を決め、すべての待ち（`wait_end`・枠待ち・drain）に使う。
   `deadline_at = min(試行の開始 + limits.max_wall_secs, 承認の開始時刻 + 試行数 × limits.max_wall_secs, expires_at)`。
   承認の開始時刻は、runner（`long-horizon-pilot-llm.sh`）が最初の試行を始めた時刻で、runner が各試行の driver に渡す。
   driver は daemon の config の `[cos] max_wall_secs` を `deadline_at` までの残り以下にする（CoS run の期限が試行の期限を超えない）。
2. **drain の範囲**: 最後の操作（`after_off`。mode off とその後の wake の確認）の後、daemon に SIGTERM を送る前に、driver は案件の
   全 chat run が次のどちらかになるまで待つ。
   - `finished_at` があり usage がある。
   - `finished_at` があり D1-2 の「起動前に終わった run」の条件を満たす。
   drain の間に新しく始まった CoS run（pending の wake から始まるもの）も同じ条件で待つ。待ちは DB を読む出来事待ち（1 秒間隔の読み取り、負荷を掛けない）。
3. **timeout → unknown**: `deadline_at` に達しても上の条件を満たさない run は、usage を 0 にせず `cos_drain_timeout` として数える。
   facts の `usage.missing` に `cos_drain_timeout:<件数>` を書き、tokens・cache・USD を null にする（N1 D1 の fail closed のまま）。
   judge は verdict unknown、g7 fail にする。drain で待った秒数は wall に含める（wall の上限の判定も同じ期限で行う）。
4. daemon の SIGTERM の扱いは変えない。SIGTERM で殺された CoS run の usage は harness が出した分しか残らず、D1-3 で費用不明になる。
   drain はこれを起こさないための driver 側の待ちである。

### D4 再試行と再起動の回収
1. **route の failover**（同じ run の中の 2 本目以降の route）: run は 1 本のまま。起動した attempt の usage は従来どおり
   `record_attempt` で run に足す。起動した attempt のうち 1 つでも usage が無ければ、run 全体を費用不明にする。
   どの attempt も起動していなければ D1 の起動前の run。
2. **枠待ち**: 再試行ではない（新しい run を作らない）。待った後に起動すれば普通の run。
3. **試行（trial）・承認の再実行**: 新しい trial id で新しい raw を書く。前の実行の raw は消さず、前の実行の USD 不明は後の実行で解けない。
   承認の累計の上限は「前の実行の分かった USD」と「人が承認の file に書いた不明分の余裕」を引いて決める（例: `approval-run2.json` は
   4 − 0.9802 − 0.32 = 2.7）。driver・judge は余裕を推定しない。
4. **daemon の再起動の回収**: 起動時の回収で、終わっていない chat run のうち
   - `launched_at` が NULL の行は `prelaunch_reason = daemon_restart_before_launch` で閉じる（`launched_at` は spawn の前に書くので、NULL なら spawn していない）。
   - `launched_at` がある行は従来どおり interrupted で閉じ、usage が無ければ費用不明。
   - 人の stop で起動前に閉じた行は `stopped_before_launch`。

### D5 API と driver・judge が伝える欄
1. **chat run の view**（`GET /api/v1/chat/threads/{id}/runs` などの run の表現）: `launched_at`（RFC 3339 か null）、`prelaunch_reason`
   （列挙か null）、`cost_state`（`known`・`unknown`・`not_launched`）を足す。`docs/api/v1/api.md`・schema・web の生成型を同じ変更で直す。
2. **案件の controller の view** の `budget_today`: 既存の `usd`・`usd_unknown_runs` に `not_launched_runs`（D1-2 で除いた chat run の数）を足す。
3. **driver の facts**（`celeris.long-horizon-facts/1` に加える欄。版は上げない。欄が無い旧 raw は下の規則で読む）:
   - `invocations[]` の CoS の行に `launch`（`launched`・`not_launched`・`unrecorded`）、`prelaunch_reason`、`slot_wait_secs`。
     `unrecorded` は daemon が `launched_at` を返さない旧記録で、起動したものとして扱う（fail closed）。
   - 最上位に `drain`: `{"deadline_at": …, "waited_secs": …, "timed_out": <件数>}`。
   - `usage.missing` の理由に `cos_drain_timeout` を足す。`cos_usage_null` は `launch` が `not_launched` の行を数えない。
4. **judge**（`pilot_gate.py`）: `cos_runs` を起動した CoS run（`launch` が `launched`・`unrecorded`）の数にし、`cos_not_launched`・
   `cos_drain_timeout` を足す。g7 が pass になるのは、全試行が実 LLM で、`cos_runs` ≥ 1、`cos_usage_null` = 0、`cos_drain_timeout` = 0、
   `cos_not_launched` = 0 のときだけ。起動前に終わった CoS run は費用不明ではない（`cost_cap` は unknown にしない）が、CoS が返事をしなかった
   ことなので g7 は fail にする。旧 raw（`launch` の欄が無い）は全 CoS 行を `unrecorded` として読むので、1 回目の raw の判定は変わらない
   （unknown・g7 fail・`cos_usage_null=3`）。

## 結果
- 起動前に確かに終わった CoS run は案件の予算を止めない。起動後・旧記録・境界の不確かな run は従来どおり費用不明で止める。
- 実 account・暗黙 account・CoS 全体の枠で選べない CoS run は失敗せずに待つ。source の状態で選べない run は従来どおり失敗する。
- pilot は最後の操作の後、CoS run の終わりと usage の書き込みを同じ期限の中で待つ。期限を過ぎた分は unknown になり、0 として足さない。
- 実装（後続の task）で試験に固定すること: 起動前の route 失敗が `not_launched` になり予算を止めない、`launched_at` のある usage null は
  止める、旧行（両欄 NULL）は止める、枠だけの失敗は待ち source の cooldown は待たない、暗黙 account に +1 しない、枠待ちの期限で
  `slot_wait_expired`、再起動の回収の 2 分岐、drain の timeout が `cos_drain_timeout` と verdict unknown になる、旧 raw の judge の結果が変わらない。
  時間依存の試験は時計の差し替えと出来事待ちで書く。
