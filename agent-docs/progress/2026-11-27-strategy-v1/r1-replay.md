---
tasks: [01M7XQ9PS0AFKKMRJ7P5ZMKQQE]
wu: r1-replay
status: done
updated: 2026-11-27
completed: 2026-11-27
---
# 戦略層 v1 親 r1-replay: R1 の最小の識別実験（決定的な escalate の規則を replay 12 点と研究ケース 7 点に当てる）

## 結論

- K = 3 では、人の反証条件に当たらなかった（`falsified: false`）。A-done・C-mid・C-done のうち発火は C-done の 1 点だけで、A-mid〜F-done の 10 点のうち 7 点で発火した。
- 誤検知は K = 3 で replay 2 点（C-done・D-mid）、研究ケース 3 点（11.17・11.18・11.19）。最初の発火は replay が C-done（基準の E-mid の 2 点前）、研究ケースが 11.17（基準の 11.20 の 3 点前）。見逃しはどちらも 0 点。
- 発火は終端の子の数だけで決まった。19 点のどれにも Outcome の到達の記録が無く、`outcome_reached` は全点で偽、`no_progress_streak` は全点で `terminal_children` と同じ値だった。この実験では、規則の no-progress の部分を試せていない（「限界」の 1）。
- LLM の呼び出しは 0 回、worker の外の run は 0 回、本番 DB への書き込みは 0 件。1 回の実行は約 3 秒。
- 人が決めること: この結果を受けて R1 を続けるか、R3 に移るか（決定 after-r1。closeout の段が人に聞く）。この WorkUnit は決めていない。

## 規則（計画で事前に固定。結果を見て変えていない）

slice ごとに次の 3 つを出し、`escalate(k)` = `outcome_reached` が偽 かつ `terminal_children >= k` かつ `no_progress_streak >= k`。判定は K = 3（依頼の「2〜3 の子完了時」の上限）、K = 2・4 は感度で判定に使わない。

- `outcome_reached`（D6）: Outcome が 1 件以上あり、全部が到達のときだけ真。到達の記録が無ければ偽（子 8 の `goal_reached` と同じ定義）。
  - replay: Outcome は goal.md から作る 1 件（子 8 の seed と同じ数）。到達の記録は親 P の done への遷移か `reviewer_passed` の event。
  - 研究ケース: slice（c-results.md の 11.15〜11.k 節）に Outcome とその到達の記録は無い。Outcome 0 件なので偽。
- `terminal_children`:
  - replay: slice.json の `children_done` の数（親 P は除く。12 点のどれにも P は無かった）。
  - 研究ケース: 11.15〜11.k 節のうち、実験の結果を報告した節の数。節の本文に PBS の job の番号（5 桁の数）と測った数の比（`8/24` の形）の両方があれば報告した節とする。正規表現は節の本文だけから決め、sentinel と時点の表を使わない。
- `no_progress_streak`: 到達した Outcome の数が最後に増えてからの `terminal_children`。増えていなければ `terminal_children`。

誤検知の数え方（採点だけが answer key と時点の表を読む）:

- replay: 子 0 の answer key で `ideal_verdict` が `off_track` の点（E-mid〜G-done の 7 点。`first_detectable` = E-mid）を「検知してよい点」とし、それより前の 5 点での発火を誤検知とする。
- 研究ケース: 子 0 の research-case.md の時点の表で、「合理的な次の手」に「改め」（結論を改める）が出る最初の行の時点 11.20 を基準とし、それより前の点での発火を誤検知とする。表の行は 11.16・11.17・11.20 の 3 つで、表に無い 11.15・11.18・11.19・11.21 は基準の前か後かで分けた。
- 反証（人の決定。K = 3）: A-done・C-mid・C-done のうち 2 点以上で発火する、または A-mid〜F-done の 10 点で一度も発火しない。

## 結果（K = 3 が判定、K = 2・4 は感度）

replay（アーキv4。時刻は cutoff の UTC）:

| 順 | 時点 | cutoff | 終端の子 | streak | K=2 | K=3 | K=4 | answer key |
|---|---|---|---|---|---|---|---|---|
| 1 | A-mid | 11-17 16:55 | 0 | 0 | | | | 前 |
| 2 | A-done | 11-17 18:45 | 1（A） | 1 | | | | 前 |
| 3 | C-mid | 11-18 07:41 | 2（A・B） | 2 | 発火 | | | 前 |
| 4 | C-done | 11-18 10:48 | 3（A・B・C） | 3 | 発火 | 発火 | | 前 |
| 5 | D-mid | 11-18 18:40 | 3 | 3 | 発火 | 発火 | | 前 |
| 6 | E-mid | 11-18 20:36 | 3 | 3 | 発火 | 発火 | | 基準（first_detectable） |
| 7 | D-done | 11-18 20:39 | 4 | 4 | 発火 | 発火 | 発火 | 検知してよい |
| 8 | E-done | 11-18 22:56 | 5 | 5 | 発火 | 発火 | 発火 | 検知してよい |
| 9 | F-mid | 11-19 03:28 | 5 | 5 | 発火 | 発火 | 発火 | 検知してよい |
| 10 | F-done | 11-19 04:55 | 6 | 6 | 発火 | 発火 | 発火 | 検知してよい |
| 11 | G-mid | 11-19 09:26 | 6 | 6 | 発火 | 発火 | 発火 | 検知してよい |
| 12 | G-done | 11-19 10:42 | 7 | 7 | 発火 | 発火 | 発火 | 検知してよい |

研究ケース（BenchFS ISC27 task C。節の番号と数だけ）:

| 順 | 時点 | 報告した節 / slice の節 | streak | K=2 | K=3 | K=4 | 時点の表 |
|---|---|---|---|---|---|---|---|
| 1 | 11.15 | 1 / 1 | 1 | | | | 前 |
| 2 | 11.16 | 2 / 2 | 2 | 発火 | | | 前（表の行あり） |
| 3 | 11.17 | 3 / 3 | 3 | 発火 | 発火 | | 前（表の行あり） |
| 4 | 11.18 | 4 / 4 | 4 | 発火 | 発火 | 発火 | 前 |
| 5 | 11.19 | 5 / 5 | 5 | 発火 | 発火 | 発火 | 前 |
| 6 | 11.20 | 6 / 6 | 6 | 発火 | 発火 | 発火 | 基準（表の行あり） |
| 7 | 11.21 | 7 / 7 | 7 | 発火 | 発火 | 発火 | 後 |

19 点とも Outcome の到達は偽（replay は Outcome 1 件・到達 0 件、研究ケースは Outcome の記録 0 件）。

採点:

| 事例 | K | 最初の発火（基準からの点の差） | 誤検知 | 見逃し |
|---|---|---|---|---|
| replay | 2 | C-mid（−3） | 3 点（C-mid・C-done・D-mid） | 0 |
| replay | **3** | **C-done（−2）** | **2 点（C-done・D-mid）** | **0** |
| replay | 4 | D-done（+1） | 0 | 1 点（E-mid） |
| 研究 | 2 | 11.16（−4） | 4 点（11.16〜11.19） | 0 |
| 研究 | **3** | **11.17（−3）** | **3 点（11.17〜11.19）** | **0** |
| 研究 | 4 | 11.18（−2） | 2 点（11.18・11.19） | 0 |

反証条件（K = 3）: 早い 3 点で発火したのは C-done の 1 点（2 点以上ではない）。A-mid〜F-done の 10 点では C-done・D-mid・E-mid・D-done・E-done・F-mid・F-done の 7 点で発火した。どちらの条件にも当たらず、`falsified: false`。
感度の K = 2 なら C-mid・C-done の 2 点で発火し、前者の条件に当たる（判定には使わない）。

## 子 8 の方式 a・b・c との比較

子 8 の数は [2026-11-24-strategy-v1-8.md](../2026-11-24-strategy-v1-8.md) の「機械で付けた軸」と「指標」から。子 8 の検知は「verdict が at_risk・off_track で next が continue 以外」、R1 の検知は escalate の発火で、意味が違う。R1 は pivot も識別実験も出さず、人か評価器に上げるだけである。

| 方式 | replay の最初の検知（iter-1 / iter-2） | 研究の最初の検知（iter-1 / iter-2） | 方式の LLM run・USD（2 反復の計） | 反復の間の揺れ |
|---|---|---|---|---|
| a 旧の固定 Task tree | 無し / 無し | 無し / 無し | 0 run・0 USD | 無し（検知しない） |
| b 途中の親 Evaluator | 無し / D-done（+1） | 11.20（0）/ 11.20（0） | replay 6 run・0.753976 USD、研究 6 run・1.612363 USD | replay で揺れた |
| c Evidence ＋ 複数戦略 | C-mid（−3）/ 無し | 11.20（0）/ 11.16（−4） | replay 10 run・2.832197 USD、研究 10 run・3.101796 USD | 両事例で揺れた |
| R1 K=3（全 12・7 点） | C-done（−2） | 11.17（−3） | 0 run・0 USD | 無し（再計算で byte 一致） |
| R1 K=3（子 8 と同じ 3 点だけ） | D-done（+1） | 11.17（−3） | 0 run・0 USD | 無し |

- 子 8 と同じ replay の 3 点（C-mid・D-done・F-done）では、R1 は C-mid で発火せず、D-done・F-done で発火した。answer key に照らすと誤検知 0・見逃し 0 で、子 8 の見逃し（a 2 / 2、b 2 / 0、c 1 / 0）より少ない。
- 子 8 と同じ研究の 3 点（11.16・11.17・11.20）では、R1 は 11.17 で誤検知し、11.20 で発火した。方式 b は 2 反復とも 11.20 で、R1 より基準に近い。
- R1 は毎回同じ点で発火する。子 8 の方式 b・c は 2 反復のうち片方でしか replay の兆しを出さず、機械の pivot の時点の点は 0 になった。R1 にはこの揺れが無い。
- 子 8 の方式 b・c は、親の目的に照らして何が未達かを文で返した。R1 は数だけで発火し、理由を返さない。

## 限界

1. 19 点のどれにも Outcome の到達の記録が無い。`outcome_reached` は全点で偽、`no_progress_streak` は全点で `terminal_children` と同じで、規則は「K 個目の子が終端になったら発火する」数え上げと区別できなかった。途中で Outcome が到達する事例が無いので、no-progress の部分（到達で streak が 0 に戻ること）は合成の slice の試験でしか確かめていない。
2. replay の D-mid と E-mid は入力の値が同じ（終端の子 3・streak 3）で、規則は 2 点を区別できない。answer key は E-mid の中身（D の worker の報告の「未達/要判断」）で 2 点を分けたが、R1 はその中身を読まない。D-mid の誤検知はこの差から出た。E-mid の発火は、C-done から子の数が 3 のまま続いた結果である。
3. 研究ケースは 1 節を終端の子 1 件とみなした（子 8 の seed と同じ）。7 節すべてが報告の節で、正規表現は数を減らさなかった。研究の Outcome は slice に記録が無く 0 件とした。子 8 の seed は Outcome を 1 件置いたが、到達の event は無いので、どちらでも偽になる。
4. 誤検知の基準は子 0 の answer key と research-case.md の表である。どちらも子 0 の案で、人の独立の採点ではない。研究の表は 3 行だけで、表に無い 4 点は基準の前か後かで分けた。
5. 点の数は replay 12・研究 7 と少ない。K = 3 がほかの案件でも同じ誤検知の率になるかは未確認。
6. 本番の controller への組み込みは試していない（人の決定で shadow のまま）。HPC・canary・外部公開はしていない。

## 再現

repo の root で実行する（stdlib と git だけ。研究ケースの c-results.md は子 0 の research_case.py の `SRC` から読み、sha256 を確かめる）。

```sh
python3 -I agent-docs/progress/2026-11-27-strategy-v1/r1_rule.py run      # r1-result.json を書く
python3 -I agent-docs/progress/2026-11-27-strategy-v1/r1_rule.py verify   # 再計算と byte 一致・欄・k=3（exit 0/1）
python3 -I agent-docs/progress/2026-11-27-strategy-v1/test_r1_rule.py     # 12 件
```

- `r1_rule.py` は子 0 の `replay/replay.py`（`cmd_build`・`manifest_lines`・`norm`）と `research/research_case.py`（`sections`・`slices`・`leaks`・`SRC`・`SHA`・`POINTS`）を importlib で path から読む。写していない。replay の slice は子 0 の `manifest.sha256` と一致することを確かめてから使う。
- slice は `$TMPDIR` の下の tempfile の dir に作り、終わったら消す。研究ケースの本文は repo に写していない（節の番号と数だけを書いた）。
- 試験 `test_r1_rule.py` の守る不変条件:
  - 規則の関数 `evaluate` は slice の dir の中の file だけを開く。19 点の実の slice で、`sys.addaudithook` の open の event を記録して確かめる。
  - answer key を読む偽の規則を同じ記録器に当てると、読みを検出する（否定対照）。
  - `compute` は 19 点の発火をすべて出した後にだけ answer key と時点の表を開く。
  - 規則の定義（親 P を除く・到達で streak が戻る・Outcome の記録が無ければ偽・job と比の両方が要る）を合成の slice で確かめる。
  - `verify` は欄の欠けと k の書き換えで落ちる。
  - 既存の試験では検出できない理由: 子 0 の `replay.py verify`・`research_case.py verify` は slice の漏れを見るだけで、規則の関数が何を読むかを見ない。

## 検査（提出時の HEAD で実行）

- `python3 -I agent-docs/progress/2026-11-27-strategy-v1/r1_rule.py run` → exit 0。
- `python3 -I agent-docs/progress/2026-11-27-strategy-v1/r1_rule.py verify` → exit 0（再計算が r1-result.json と byte で一致、falsified=False）。
- `python3 -I agent-docs/progress/2026-11-27-strategy-v1/test_r1_rule.py` → exit 0、12 件 OK。
- `python3 -I agent-docs/progress/2026-11-21-strategy-v1-0/replay/replay.py verify` → exit 0。`python3 -I agent-docs/progress/2026-11-21-strategy-v1-0/research/research_case.py verify` → exit 0。
- production code（`crates/`・`control-ts/`・`web/`・`scripts/`）と子 0 の dir の差分: `git diff --stat "$CELERIS_WU_BASE" -- crates control-ts web scripts agent-docs/progress/2026-11-21-strategy-v1-0` → 0 行。
