# アーキv4 の歴史 replay（子 A・C・D・E・F・G、12 時点）

親 01M7MZT8KX547464YYS76QNP4G（アーキv4）は、子 A〜H がすべて done でも親の目的が未達だった。この replay は、
**途中の時点で未達を検知できたか**と**仮説を改められたか**を、その時点までのログだけで試すための入力を作る。
子 6（途中の親の評価）と子 8（対照 pilot）が使う。

## 再実行

repo の root で（stdlib の python3 と git だけ。本番 DB は読まない）:

```sh
python3 -I agent-docs/progress/2026-11-21-strategy-v1-0/replay/replay.py verify
python3 -I agent-docs/progress/2026-11-21-strategy-v1-0/replay/replay.py build --out "$TMPDIR/replay"   # slice を作って評価器に渡す
python3 -I agent-docs/progress/2026-11-21-strategy-v1-0/replay/replay.py check "$TMPDIR/replay"
```

`verify` は次の 4 つを確かめ、すべて期待どおりなら exit 0:

1. sentinel の初出時刻を fixture と git から測り直し、[sentinels.json](sentinels.json) の記録と一致する。
2. 12 の slice を作り直し、[manifest.sha256](manifest.sha256) の 365 file と byte 単位で一致する（同じ入力が再生できる）。
3. `check`: 各 slice の event の ts が cutoff 以下、snapshot の commit の committer date が cutoff 以下、初出が cutoff より後の sentinel が無い、
   answer key の印が無い、answer key の根拠の文字列が slice の中に実在する。
4. 否定対照 3 件で `check` が**落ちる**: N1（A-done に基準 commit の snapshot を入れる）、N2（D-done に G の完了の event を足す）、N3（C-done に answer key を入れる）。

fixture を作り直すのは operator だけ（本番 DB を読み取り専用で開く）:
`python3 -I …/replay.py export --db /local/celeris/data/db/celeris.sqlite3`。作り直したら `first-seen`・`manifest` を更新して commit する。

## 入力の作り方

| file | 中身 | 時点 T での切り方 |
|---|---|---|
| [goal.json](goal.json) | 親の作成時（2026-11-17T14:50:43Z の `created` event）の title と objective（人の依頼の全文を含む） | 全 slice に同じものを `goal.md` として入れる |
| [events.jsonl](events.jsonl) | 親・子 A〜H・その子 task（計 23 task）の events のうち 18 種、2,071 行。欄は白 list（`transitioned`・`review_verdict` の理由 400 字・`worker_finished` の outcome 200 字と cost など）。`worker_progress`・routing・quota と、人の名前が入る `decision_answered` の note は落とした | `ts <= T` の行だけ |
| git の main | 基準 commit の first-parent の commit の tree のうち、`agent-docs/progress/` の `arch-v4` を名に含む file と dir | committer date が T 以下の最新の first-parent の commit。rebase は committer date を後ろにしか動かさないので、T 以下の commit の tree は T の時点に実在した |

slice の dir には `goal.md`・`events.jsonl`・`snapshot/`・`slice.json`（cutoff・snapshot の commit・event 数・done の子・ここまでの費用）・`question.md`（評価器への問い）が入る。

## 時点と answer key

時点の規則（`checkpoints()`）: mid = 子の 2 つ目の `phase_integrated`（無ければ最初の running → reviewing）、done = `integration_merged`。
answer key（[answer_key.json](answer_key.json)、slice には入れない）は子 0 の案で、人が rubric で採点するときの参照。根拠はすべて slice の中の文字列で、`check` がその実在を確かめる。

| 順 | 時点 | cutoff（UTC） | done の子 | 費用（list price USD） | 理想の判定 | 理想の次の手 | slice の中の根拠 |
|---|---|---|---|---|---|---|---|
| 1 | A-mid | 2026-11-17T16:55:44 | – | 5.46 | unknown | continue | goal.md「既存モジュール循環17件」 |
| 2 | A-done | 2026-11-17T18:45:54 | A | 13.24 | at_risk | discriminating_experiment | A の進捗「既存の module 循環 17」 |
| 3 | C-mid | 2026-11-18T07:41:56 | A・B | 85.15 | at_risk | continue | event `design_review_failed:2` |
| 4 | C-done | 2026-11-18T10:48:51 | A〜C | 109.97 | at_risk | continue | C の進捗「+664」 |
| 5 | D-mid | 2026-11-18T18:40:55 | A〜C | 161.57 | at_risk | continue | C の進捗「17・198・756」 |
| 6 | E-mid | 2026-11-18T20:36:35 | A〜C | 171.06 | **off_track** | discriminating_experiment | event D#1446「未達/要判断として書いたもの: handlers ⇄ cos」 |
| 7 | D-done | 2026-11-18T20:39:40 | A〜D | 173.33 | off_track | pivot | event D#1566「組 261・単位 49 は不変」 |
| 8 | E-done | 2026-11-18T22:56:13 | A〜E | 183.25 | off_track | pivot | E の進捗「(1) 循環: 上のとおり差 0」 |
| 9 | F-mid | 2026-11-19T03:28:47 | A〜E | 210.24 | off_track | ask_human | D の進捗「単位の循環は解けていない」 |
| 10 | F-done | 2026-11-19T04:55:10 | A〜F | 218.32 | off_track | ask_human | F の進捗「rusqlite を書く production file 75 → 75」 |
| 11 | G-mid | 2026-11-19T09:26:52 | A〜F | 238.26 | off_track | ask_human | event の `verdict=code` |
| 12 | G-done | 2026-11-19T10:42:07 | A〜G | 245.87 | off_track | ask_human | G の進捗「S1 は file・単位 3 → 4」 |

## 測った結果（確認済み。fixture から計算）

- 最初に未達を検知できた時点: **E-mid（2026-11-18T20:36:35Z）**。D の worker の報告（D#1446、19:55:39Z）が handlers ⇄ cos を未達/要判断と書いた。
- 親が最初に評価した時刻: **2026-11-20T14:12:10Z**（親の最初の running。途中の checkpoint は計画に無く、親は ready のまま子の完了を待った）。差は約 41.6 時間。
- 検知できた時点の後に走った worker の run: 137 回（H の完了まで。前は 155 回）。費用は list price で 171.06 → 435.31 USD（+264.25、全体 450.35 の 59 %）。
  費用は `worker_finished.usage.cost_usd` の和で、subscription の実費ではない。
- 仮説を改めたか（史実）: 改めていない。E・F・G・H は同じ形の pilot・計測を続け、H の replan（R1〜R6）も同じ枠の中の修正だった。

## 限界（未確認・推定）

- answer key の「理想の判定・次の手」は子 0 の推定で、人の採点で上書きできる。とくに A-done の at_risk（実装の前の識別実験）は事前の判断で、反対の意見がありうる。
- snapshot は main に入った後の tree なので、task の branch にだけあった途中の進捗は入らない。その分は events で補った（D#1446 など）。
- worker の run の中の会話（`worker_progress`）は落とした。評価器が見られるのは、親が当時見られた記録（進捗・review・報告）に限る。
