---
title: 健全性v2 B0 dup-candidates — 同義の DTO・parser・policy・config 欄と未参照候補の棚卸し
tasks: [01M6TYVJSPEYA2P8B7G56HSQ73]
status: done
updated: 2026-11-07
---

# 健全性v2 B0 dup-candidates

## 結論
- 候補は 55 件。統合 5 件、削除 35 件、維持 15 件。各候補に file:line・判断・理由・一元化される正本・影響する試験を書いた。
- 統合 5 件: celerisctl の API 接続先の解決（6 subcommand の写し）、`AccountCheckResult`（task-api と task-worker の同じ 4 値と手書きの写像）、task features の内容 hash（route_record と eval_trials）、scratch status の schema 名、browser sandbox の内側 UID の定数。
- 旧い仕組みの残り 5 件は削除: 読まれない config 欄 `[execution] cheap_local_first`・`[scratch] l1_max_gb`、常に null の API 欄 `ScratchCacheView.stats`、使われない `route_record::RouteDecision`・`DecisionSubject`、使い捨ての `scripts/scratch/u5/stats.py`。
- 未参照の pub item は 36 件。30 件を削除、schema 名・版の定数 3 件を維持（owner が決める）、3 件は上の統合・削除に含めた。
- 名前が似ているだけのもの、security 境界の型、provider の wire、旧 event の読み方、Rust/TS の移行中の二重の判断は維持とし、理由を書いた。

## 成果物
- `agent-docs/progress/2026-11-07-health-v2-b0/dup-candidates.json`（schema `celeris.health-v2-b0.dup-candidates/2`。判断は `decision`（`delete`/`merge`/`keep`）、日本語は `decision_ja`、`base_sha` に着手時の `git merge-base HEAD main`）。
- 走査の raw: `dup-candidates-scan.json`（同名の pub struct/enum 132 組、未参照の pub item 36 件、未参照の script 6 本〈うち 5 本は unittest discover か手で走らせる試験で、候補にしていない〉）。
- 生成 script: `dup-candidates-scan.py`（参照検索の手順を docstring に書いた）、`dup-candidates-build.py`（判断を持ち、path:line の実在を確かめる）。
- 人が読む判断材料: 親 task の成果物 `artifacts/b0/dup-candidates.md`。
- 基準: 件名「repair: main 取り込みの衝突解消の import を整形する」（`git merge-base HEAD main`。capability-map.json と同じ）。

## 証拠（提出時の HEAD で実行）
- `python3 agent-docs/progress/2026-11-07-health-v2-b0/dup-candidates-scan.py > …/dup-candidates-scan.json` → exit 0、約 2.6 秒。build・試験は走らせない。
- `python3 agent-docs/progress/2026-11-07-health-v2-b0/dup-candidates-build.py` → exit 0、`{"candidates": 55, "by_decision": {"merge": 5, "delete": 35, "keep": 15}}`。
- WU check（`decision` が delete/merge/keep のどれかで `evidence` が空でない）→ exit 0、55。
- 未参照の 36 件は `git grep -n -w <名前> -- crates web control-ts scripts` がどれも定義の 1 件だけだった。
- 必須検査の結果は報告本文に書く。

## 再提出（attempt 2）
- 前回の run の後の WU check は `KeyError: 'decision'` で落ちた。前回の JSON は判断を日本語の `judgment` 欄に入れていた。
- `dup-candidates-build.py` が `decision`（英語の 3 値）と `decision_ja` を書くように直し、JSON を作り直した（schema を /2 に上げた）。候補の中身・判断・基準は変えていない。

## 未解決事項
- 「最近の git log で同時に変わったか」は `git log -S` の走査が 2 分を超えて打ち切られ、ほとんどを未確認とした。
- 同名の 132 組のうち、欄と使い道を読んだのは候補にした組だけ。task-api の API 欄の未参照は `ScratchCacheView.stats` 以外を走査していない。
- 本番 config に `cheap_local_first`・`l1_max_gb` が書かれているかは worker から確認できない（未確認）。

## 提案
- 削除・統合は後続の工程（G1〜）の write-set に入れる。config 欄 2 つを消す差分には「本番への移行」節を付け、人が本番 config から欄を消す。
- `route_record.rs:436` の `.expect("features serialize")` は試験以外の `expect` で、統合 `task-features-content-hash` の差分で Result にする。
