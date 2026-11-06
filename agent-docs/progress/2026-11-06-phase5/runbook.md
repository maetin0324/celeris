---
title: 第5フェーズ N1〜N7 の runbook と受け入れ条件（数値の合否）
tasks: [01M6QW0GSCHZZGYRNQMNRKRDZA]
status: done
updated: 2026-11-06
completed: 2026-11-06
---
# 第5フェーズ N1〜N7 の runbook と受け入れ条件

N0（01M6QW0GSCHZZGYRNQMNRKRDZA）が 2026-11-06 に書いた。状態表と根拠は [n0.md](n0.md)。各 task の受け入れ条件（task の
acceptance）は変えない。この file は、その条件を数値の合否に落とし、使う道具と人の gate をそろえるためのもの。

## 共通の規則
1. 実 LLM・本番の操作・新しい soak は、記録の file がある人の承認の範囲内だけで行う。worker は実 LLM を起動しない
   （`long-horizon-pilot-llm.sh --harness config` と `compare.py run --tier llm` は `CELERIS_RUN_ID` があると断る）。
   承認が無いときは task の `question` で止め、表には「承認待ち」と書く。成功率・合格の数に入れない。
2. USD は ADR [2026-11-06-phase5-n0-usd-accounting-contract](../../adr/2026-11-06-phase5-n0-usd-accounting-contract.md) の定義で数える。
   - 予算判定（daemon）: `run_budget_cost`。cache creation は 1 時間 TTL の書き込み単価以上（D3）。
   - 実験の道具: 保存値 `usage.cost_usd` の和。cache creation は 5 分 TTL の単価で、予算判定より小さい。上限の判定に使ったときは、
     同じ試行の cache creation token と、1 時間 TTL の単価で数え直した USD を表に並べる。
   - 欠測は fail closed（D2）: `usage_json` が null、cache 欄（`cache_read_tokens`・`cache_creation_tokens`）が無い、USD が出ない run が
     1 件でもある試行は「不明」で、合格にしない。0 USD として足さない。
3. 二重の job・重複の課金をしない。同じ out の dir を上書きしない（既存の raw は `cp -a` で写して `diff -r` exit 0 を確かめる）。
4. K2/K4/IEEE Access の凍結は変えない（`PRICE_TABLE_VERSION` = `2026-09-approx-1` を変えない）。
5. 人の承認の file に要る欄: `approved_by`（`human:<名前>`）・範囲（試行・案件・環境）・上限（USD・時間）・`expires_at`。
   期限の欄が無い承認は、人が有効と答えるまで「範囲不明」として扱う。

## N1 G7 縮小 pilot（01M6QW0GSG1AVARWC334AZCES1）
手順:
1. stub で経路を確かめる（LLM なし。worker が実行してよい）:
   `bash scripts/dev/long-horizon-pilot-llm.sh --harness config-stub --out <絶対 path> --cases C06,C08 --arms new`
2. 欠測の再現試験を足す（`usage_json` null・cache 欄欠落・USD 未算出のそれぞれで、試行が不合格か不明になる）。
3. 実 LLM は operator が worker の外で、人の承認の後に:
   `LONG_HORIZON_PILOT_LLM_APPROVED=1 bash scripts/dev/long-horizon-pilot-llm.sh --harness config --llm-config <toml> --cos-route '<配列>' --cases C06,C08 --out <新しい dir>`。
   `--cos-route` は空にしない（空は driver が断る。01M6PYNMP4）。

| 合否の項目 | 合格の数値 |
|---|---|
| 欠測の再現試験 | 3 種とも、修正前の driver で「合格」になり得ることを示し、修正後に「不明」か「不合格」になる |
| stub の pilot | exit 0、2/2 試行が採点まで終わる |
| G7（実 LLM のときだけ） | 案件 CoS の chat run が 1 本以上あり、`cos_usage_null` = 0、cache 欄の欠落 0、USD 未算出 0 |
| 上限（実 LLM） | 保存値の USD の和 ≤ 承認の上限（既定 `--max-usd-per-day` 20）、wall ≤ 承認の時間 |
| oracle | C06・C08 とも `controller_achieved` と oracle の判定を併記。漏洩・拒否・二重作用は 0 件 |

実 LLM が無いときは G7 を `pending` のままにし、`cos_usage_null=0` を書かない。

## N2 P4-F 24h 非本番 soak（01M6QW0GSJR1X7797YF4Y1HWR8）
手順: 新しい soak を起こさない。`/local/celeris/data/scratch/p4f-soak/` の `run.log`・`out/main-*/samples.jsonl`・process
（`pgrep -af daemon-soak`）を読む。完了後に
`python3 -I scripts/dev/daemon-soak/daemon_soak.py summary <out の 4 dir> --json --markdown` で集計する。

| 合否の項目 | 合格の数値 |
|---|---|
| 完走 | 4 条件とも最終標本の t ≥ 86,000 秒、`run.log` の exit 0。t=43,200 の SIGKILL の後に再起動している |
| 記憶の増え方 | RssAnon で比べる（VmRSS は使わない）。ウォームアップ（最初の 2 時間）の後の線形の傾きが、4 条件とも 1 時間あたり 1 MB 以下。enabled・shadow の傾きが off の傾き + 0.5 MB/時 以下 |
| 再起動の後 | 再起動の 1 時間後の RssAnon が、再起動の前の同じ経過時間の値 ±10% |
| DB | `--max-db-mb 11963` に達しない |

未完なら「進行中」と待ち条件（終了見込み 2026-11-06 23:00Z ごろ）を書く。72 時間は今の承認（86,400 秒、期限 2026-11-07T00:00Z）の外。
24 時間で傾きが上の値を超えた条件があるときだけ、72 時間の承認を人に求める。

## N3 P4-E 統括 CoS 旧新 paired（01M6QW0GSM7ZHFNP3PWZJJ4SNF）
手順:
1. preflight（LLM なし）: `python3 -I scripts/dev/general-cos-compare/compare.py preflight --tier llm …`（実 LLM の前の検査）と `run --tier stub --only 7,8`。
2. 承認の照合: 今の `compare.py` は `approval.limits` に `max_tokens` があると断る（USD 予算の task で消した）。
   `/local/celeris/data/scratch/p4e/approval.json`（2026-11-05T22:56:19Z、`max_tokens` 1,000,000 を含む、期限の欄なし）は
   そのままでは使えない。人が USD だけの上限（`max_trials`・`max_wall_secs`・`max_usd`）と期限で書き直すまで「承認待ち」。
   N3 の task の objective にある「100 万 token（cache 込み）」の上限は、今の道具では強制できないので、記録だけにする。
   N3 で承認に `id`・`expires_at`・`scope.seqs` を必須にした（[n3.md](n3.md)）。旧腕の config は `compare.py config-check` で LLM なしに確かめる。
3. 承認があるときだけ operator が `compare.py run --tier llm --only 7,8 --approval <file> …` で 1 対を流す。

| 合否の項目 | 合格の数値 |
|---|---|
| preflight | 旧腕・新腕とも config の読み込みが exit 0、sandbox の検査が全部 ok |
| 1 対（実 LLM） | 2/2 試行が `error` でなく終わり、両方の `cost_usd` が出ている（`usd_unknown` 0）。USD ≤ 承認の `max_usd` |
| 決定的な採点 | 旧腕・新腕の得点を併記（stub 段は旧 0.8・新 1.0）。相談の採点は評価者が決まるまで「未実施」 |
| 残り 30 試行 | 計画だけ。実行は追加の承認で |

## N4 P4-C 残件: 10,000 件の dispatch（01M6QW0GSR42EYSKBME8VJ5TDF）
手順: `crates/celeris/tests/tick_scale_baseline.rs` を release build で、同じ build・同じ fixture で before/after を測る
（`TICK_SCALE_BLOCKED=600,2000,10000`、`TICK_SCALE_COND=off` と `shadow`）。before の値は P4-C の 10,000 件 3.3〜4.0 秒、
restart の最初の tick 18.8 秒（`agent-docs/progress/2026-11-05-phase4/p4c.md`）。

| 合否の項目 | 合格の数値 |
|---|---|
| 10,000 件の dispatch p50 | ≤ 1,000 ms で合格（≤ 500 ms なら目標達成と書く） |
| restart の最初の tick | before（18.8 秒）の半分以下 |
| 回帰 | 600 件・2,000 件の p50 が before（600 件 off 272 ms・shadow 299 ms）の 1.2 倍以下 |
| 正しさ | root の取りこぼし 0、double dispatch 0、lease・一時 DB の破損 0（試験で） |

## N5 証跡の整合の残件（01M6QW0GSWRGP85RE263TWTM01）
| 合否の項目 | 合格の数値 |
|---|---|
| checks だけの改版（人・CoS の root） | 再現試験が修正前に失敗し、修正後に通る。古い root では達成にならない |
| reached の evidence | 再達成の後に新しい root を指す試験が通る |
| fail closed | 旧版の root・root の reopen 中・repo の無い調査案件・権限の無い主体の各条件に試験が 1 件以上 |
| GC の最大 id | trigger か DB の制約で、最大 id の行を消す DELETE が失敗する試験。archive の hash・再起動・復元の結果を記録 |

## N6 本番 1 案件の限定導入の準備（01M6QW0GT1GBT4M4QKDDG79Z7X）
手順: `docs/ops/project-controller.md` の 4a 節。予行は `bash scripts/dev/pc-rollout.sh --out <絶対 path>`（本番に触れない、約 3 分）。

| 合否の項目 | 合格の数値 |
|---|---|
| G1〜G7 の表 | 7 行すべてに go・no-go・pending と証拠の path。G7 は N1 の実 LLM の facts が無ければ pending |
| 予行 | dry_run・shadow・undo が exit 0、監視の違反 0 |
| release | 本番の release が 9ed88afa4506 以降（schema 117・USD 予算を含む）であることを GET /api/v1/config の identity で確かめる |
| 人の承認 | observe（dry_run・shadow）と effects（canary・enabled、本番の shadow 86,400 秒以上・違反 0 の報告の sha256）を操作ごとに列挙 |

## N7 closeout の検証（01M6QW0GT56KDVBHCXX78GBFHF）
| 合否の項目 | 合格の数値 |
|---|---|
| 検査 | `bash scripts/dev/test-affected.sh`・`cargo clippy --workspace -- -D warnings`・`cargo fmt --all -- --check` が exit 0 |
| 再採点 | scripted と real（実 LLM）を別の表にする。real に入れるのは実 LLM の facts がある試行だけ |
| 費用対効果 | 試行ごとの USD（保存値と、1 時間 TTL で数え直した値の 2 列）、人の介入の回数、回復率。欠測の試行は「不明」の行に分ける |
| go/no-go | 条件付きで書く。本番運用の達成は宣言しない |

## 重複する task（閉じずにリンク）
- 第4フェーズ親 01M6PAK8E8RH3ZMQC0FAD8ZNAA の残り（G7・P4-E の実 LLM・P4-F 24h・P4-G 承認）は N1・N3・N2・N6 が引き継ぐ。
- USD 予算 01M6QDK2PMJTXF17Z8TNDKF85M の「本番への移行」は、人が 9ed88afa4506 を昇格したことで実施済み（n0.md）。cache の単価の
  修正は N0 の branch にあり、main と本番にはまだ無い。
