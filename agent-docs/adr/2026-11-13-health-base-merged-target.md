---
tasks: [01M79CHBRY3CBHKPNECPPM6G32]
---
# ADR 2026-11-13: WU の check と段の統合の health の base を、取り込んだ review target の分だけ進める
状態: 採用（実装は同じ task の unit impl-base）

- 日付: 2026-11-13
- 置き換えるもの: [ADR 2026-11-08 health を開発・統合ループに強制する](2026-11-08-health-enforcement-loop.md) の D1.2 付記（2026-11-10、task 01M72XBPY2PDZA1KFJ0FJWWY1N）の base の規則（段の基点 P が「統合の前の HEAD と target の merge-base M」の祖先なら base = M）と、同じ付記の「WU の check の health（同じ問題は無い。変えない）」。
- 関係: 同 ADR の D1.2（経路の表）・D1.3（実行の口は 1 つ）・D1.4（検査器は base の版）・D2（証跡・再利用）・D3.1（verdict の判定）・D3.4（Error は Blocked）・D4（上限と往復）、[ADR-0118](0118-review-target-sync-and-merge-candidate.md)（review target と review 前の同期）、[ADR 2026-11-07 repo health の契約](2026-11-07-repo-health-contract.md)（policy は base 側で評価する）、[ADR 2026-11-07 健全性を設計と review に組み込む](2026-11-07-health-review-integration.md)（G2 の I3、D6 の policy の branch）、[破壊的変更を既定にする](2026-10-11-breaking-changes-by-default.md)。

## 結論
- WU の check と段の統合の health は、評価する head が review target の commit を base より多く含むとき、base を「base と取り込んだ target の commit T の merge」にする。T は `git merge-base --all <head> <target>` の 1 件で、HEAD の親は辿らない。T が base の祖先（T = base を含む）なら取り込みは無く、base は今と同じ。
- base が T の祖先なら base = T。どちらも相手の祖先でなければ、`git merge-tree --write-tree` で base と T の tree を作り、`git commit-tree`（親は base と T、作者・日付・本文は固定）で作った commit を base にする。
- merge-tree が衝突を報告したときも、衝突の印を含む tree の commit を base にし、加えて head を T と比べた 2 回目の検査（base = T）も合格（loop D3.1 の Pass か PolicyOnly）であることを求める。衝突した path が検査器の file に当たるときは評価せず Unavailable にする。
- target を解決できない・git が失敗した・merge-base が複数あるときは Unavailable（verdict Error）にし、合格にしない。
- 段の統合の base は merge の後（統合の HEAD が決まった後）に決める。旧い規則（統合の前の HEAD で M を求める `phase_health_base`）と、WU の check の `base = wu.base_commit` の固定は消す。

## 背景
- 2026-11-13 の本番: R4（task 01M6TYVJT4S29ZMP6P0D4831JG）の WU sync-policy（01M74TGFAQM3QZKFXMQER7SMTD）は、計画どおり main 9091b3aa を WU の branch に merge した（親は WU の base f8a77917 と 9091b3aa。11 file の衝突を worker が解いた）。WU の check `celeris:repo-health/1` は base を `wu.base_commit`（f8a77917、main を含まない）にするので、取り込んだ main の変更を WU の変更として数え、fail 級 5 件で落ちた。修復の後も同じ集合が残り、loop D4.3 の往復で Blocked になった（受信箱 `health_repeat-01M6TYV9DTZESAJJMXRQVGZ56V-e0d14cc6eed8651b`・`authorization-01M79A6FVE7C9BRZ2JPVDF2RVN`）。
- 旧い規則の穴:
  1. WU の check には規則が無い。2026-11-10 の付記は「Celeris は WU の branch に target を取り込まない」ことから問題が無いとしたが、worker が計画に従って target を merge する場合（sync-policy のような同期の WU）を見ていなかった。
  2. 段の統合の規則は、Task の branch を target の merge commit まで ff-only で進めた場合（P が M の祖先）だけを扱う。M は統合の前の HEAD から求めるので、unit の branch が target を merge した場合や、target から分かれた branch に target を merge した場合（merge commit が target に無い）は base = P のままになる。R4 の sync-policy をこのまま統合すると、段の統合でも同じ 5 件で落ちる。
- この ADR を書く前に、R4 の WU の HEAD 2569a779 を手元の検査器（各 base の版を `git archive` で出した）で 3 通りの base で評価した（2026-11-13、作業場所の git の読み取りと `git merge-tree`・`git commit-tree` だけ。ref は動かしていない）。

  | base | 何か | exit | fail 級 |
  |---|---|---|---|
  | f8a77917 | 今の WU の check（`wu.base_commit`） | 1 | 5 件（`policy.changed:ffe502d50e6810f5`・`progress.index:92db900991dc031f`・`size.responsibility-growth:44b9ca4976609552`・`size.responsibility-growth:a28ce5c5f00898d9`・`source-size:bbf931d224b0274c`。受信箱と同じ） |
  | 371a0380 | この ADR の base。`git merge-tree --write-tree f8a77917 9091b3aa` は exit 1（11 file の衝突）で、その tree を `commit-tree -p f8a77917 -p 9091b3aa` した commit | 0 | 0 件 |
  | 9091b3aa | T（`merge-base --all 2569a779 refs/heads/main` の 1 件）。衝突時の 2 回目の検査 | 0 | 0 件 |

  `git diff --name-only 371a0380 2569a779` は 19 path で、衝突した 11 file をすべて含む（worker の解き方は差分に残る）。`git diff --name-only 9091b3aa 2569a779` は 114 path。同じ入力で `commit-tree` を 2 回実行し、同じ sha（371a0380）になることも確かめた。

## 用語
- head: 評価する commit。WU の check は WU の作業ツリーの HEAD、段の統合は統合の HEAD（unit を merge した後）。
- start: 取り込みが無いときの base。WU の check は `wu.base_commit`、段の統合は段の基点 P（`phase_base_commit`。無ければ統合の前の HEAD）。今の base と同じ。
- target: review 前の同期と同じ `Dispatcher::review_target_ref` の ref（root の task は repo の既定 branch、木の子は親の branch）。WU の check も、その WU の task の target を使う。
- T: head が含む target の commit のうち最も新しいもの（D1）。
- 番号: `loop D1.3` のように `loop` を付けた番号は [2026-11-08 ADR](2026-11-08-health-enforcement-loop.md) の番号。付けない D・K の番号はこの ADR の番号。

## 不変条件
- K1（I3）: base の tree は start と T（どちらも候補の外で決まり、T は target から辿れる承認済みの側）と git の決定的な merge だけで決まる。候補（worker の commit・worker の merge commit の tree）は base の内容に入らない。検査器と policy は今どおり base の版（loop D1.4）。
- K2: worker の merge の衝突の解き方と、merge commit に足した変更と、merge の後の commit は、すべて `base..head` の差分に残る。
- K3: head が target の commit を start より多く含まなければ、base は start のまま（今と同じ）。
- K4: target・git・merge を確かめられないときは評価せず Unavailable（verdict Error）にする。error・unknown を合格にしない（loop J4）。取り込みが無いと推測して start にするのは、下の D5 に挙げた「取り込みが無いと分かる」場合だけ。
- K5: 同じ (start, head, target の tip) からは同じ base の sha ができる（loop D2.5 の再利用の鍵と loop D4.3 の往復の判定が、同じ入力で揺れない）。

## 決定

### D1 T の特定と、取り込み済みの判定
- D1.1 T は `git merge-base --all <head> <target>` が返す commit。head が target を ff で取り込んだ、merge commit で取り込んだ、target の途中の commit を取り込んだ、target の上に rebase した、のどれでも、T は「head と target の両方から辿れる最も新しい commit」になり、必ず target から辿れる。
- D1.2 HEAD の親を辿って「target から来た親を持つ merge commit」を探す方法は採らない。merge の形（ff・merge・squash・rebase・親の順・複数回の merge）は worker が決めるので、形ごとの分岐が要り、ff と rebase では merge commit が無い。merge-base は commit の graph だけで決まる。
- D1.3 取り込み済みの判定: `merge-base --all` の結果がどれも start の祖先（`git merge-base --is-ancestor <T> <start>` が exit 0。T = start を含む）なら、head は start が含む以上の target の commit を含まない。取り込みは無く、base = start（K3）。target を取り込まない WU では、head = start + worker の commit なので、merge-base は start と target の merge-base になり、必ずこの場合に入る。
- D1.4 `merge-base --all` が 2 件以上を返し、そのうち start の祖先でないものがあるとき（criss-cross）は、T を 1 つに決められないので Unavailable（D5）。
- D1.5 merge-base の exit 1（共通の祖先が無い）は取り込みが無いので base = start（2026-11-10 の付記と同じ）。

### D2 base の決め方
T が start の祖先でないとき（取り込みがある）:
- D2.1 start が T の祖先（`merge-base --is-ancestor <start> <T>` が exit 0）なら base = T。start から T への ff と同じで、merge の commit を作らない。2026-11-10 の付記の場合（Task の branch を target の merge commit まで ff-only で進めた）はここに入る。unit が target を取り込んでいなければ T = M で、base は今と同じ commit になる。
- D2.2 どちらも相手の祖先でなければ、`git merge-tree --write-tree --name-only --no-messages <start> <T>` を実行する。
  - exit 0（衝突なし）: 1 行目の tree を D4 の `commit-tree` で commit にし、base にする。差分 `base..head` は、worker の merge commit が自動の merge と違う所（merge commit に足した変更）と、merge の後の commit だけになる。
  - exit 1（衝突あり）: 1 行目の tree（衝突した file は衝突の印を含む）を同じく commit にして base にし、D3 の 2 回目の検査を足す。2 行目以降の衝突した path を D3・D6 で使う。
  - ほかの exit・出力の読めない結果（tree の sha が無い等）: Unavailable（D5）。
- D2.3 段の統合の base は、統合の HEAD が決まった後（unit の merge の後）に D1・D2 で決める。unit の branch が target を merge した場合（R4 の sync-policy の統合）も T に入る。start（P、無ければ統合の前の HEAD）は今どおり merge の前に読む。統合の修復の後の再統合も同じ `start_integration` を通るので同じ規則になる。
- D2.4 task の review の health（base = review 前の同期の後の `target_sha`）は変えない。同期の後の head は `target_sha` を含むので、この規則に当てても T は start の祖先になり、base は `target_sha` のままになる。経路を足すだけの差し替えはしない。
- D2.5 範囲 check の基点（ADR-0074 の `CELERIS_WU_BASE`）は、この ADR では扱わない。

### D3 衝突したときの 2 回目の検査（base = T）
- D3.1 merge-tree が衝突を報告したら、health の項目は 2 回評価する。1 回目は base = D2.2 の commit、2 回目は base = T。どちらも head は同じ。どちらも loop D1.3 の実行の口（`health_review::gate`）と loop D1.4 の検査器の所在を通る（2 回目の検査器と policy は T の版）。
- D3.2 合否: 2 回の結果を loop D3.1 でそれぞれ判定し、悪い方を項目の verdict にする（Error > Fail > PolicyOnly > Pass）。fail 級の指摘が 0 件の Pass か、WU・段の統合で合格扱いの PolicyOnly のときだけ合格にする。PolicyOnly は今どおり task の review で G2 の `policy_review_gate` が人の承認を求める。PolicyOnly を合格扱いに含めるのは、loop D3.1 が WU・段の統合で PolicyOnly を合格扱いにして人の承認を task の review に置いており、衝突の有無で policy の branch の扱いを変えないため。PolicyOnly は変更 path が全部 policy の file のときだけなので、policy でない変更には fail 級 0 件を求めるのと同じになる。
- D3.3 2 回目が要る理由: 1 回目の base の衝突した file は両側の hunk と印を含むので、行数・大きさの指標が実際の file より大きい。worker の解き方が両側より file を大きくしても、1 回目では悪化に見えないことがある。2 回目は解き方を承認済みの target の版と比べる。1 回目だけでは K2 の「解き方を検査から外さない」を指標の面で守れない。
- D3.4 2 回目は base 側（start が T に無い分。Task の branch の前の段の変更）も差分に数える。前の段の変更はそれぞれの段で合格しているので、T と比べた指摘は解き方と WU の変更によるものが主になる。base 側に由来する fail 級の指摘が出たときも合格にせず、今の repair と loop D4.3 の往復（Blocked と受信箱）で人が決める。衝突のある merge は少ないので、誤って止める側（fail closed）を選ぶ。
- D3.5 検査器の file の衝突: 衝突した path が検査器の取り出しの対象（`task_core::required_checks::health` の `CHECKER_DIR`〈`scripts/dev/repo-health`〉と `CHECKER_SIDE_FILES`〈`source-size-report.py`・`check-architecture-map.py`〉。`with_base_checker` が `git archive` で出す file）に 1 つでも当たれば、1 回目の検査器が衝突の印を含むので評価しない。項目は Unavailable（`facts_unreadable:health_base: 検査器の file が衝突した: <path,…>`）にする。両側が検査器を変えた衝突は、G2 D6 の policy の変更と同じく人の判断が要る。profile の `policy_paths` に当たる file（検査器の dir の外）の衝突は評価を続ける。1 回目の base のその file は衝突の印を含むので、解き方が target の版と同じでも `policy.changed` が出る。項目は PolicyOnly（task の review で人の承認）か Fail（repair と往復で人が決める）になり、人の判断なしには合格しない。
- D3.6 時間: 2 回目の評価は検査器の起動ごとに `HEALTH_INPUT_TIMEOUT` を使う（loop D4.2 と同じ）。衝突した merge では検査器の時間が最大 2 倍になる。

### D4 base の commit の作り方（決定性）
- D4.1 `git commit-tree <tree> -p <start> -p <T> --no-gpg-sign`。親の順は start、T。
- D4.2 作者と committer は固定の `celeris-health-base <health-base@celeris.invalid>`。日付は start と T の committer date の新しい方（epoch 秒、`+0000`）を作者と committer の両方に使う。本文は `celeris health base` の 1 行と、`start <sha>`・`target <sha>` の 2 行。環境変数で渡し、host の git の設定（`user.*`・`commit.gpgsign`）に依らない。
- D4.3 日付を新しい方にする理由: 検査器は base の commit の committer date を例外の期限の基準日にする（契約の付記 2026-11-10）。D2.1 で base = T になる場合と同じ基準日になる。実行時の時計は使わない（K5）。
- D4.4 commit はどの ref にも指されず、作業ツリーの object の DB に置かれる（merge-tree の tree も同じ）。ref・index・作業ツリーは変えない。git gc が期限の後に消すことがあるが、同じ入力から同じ sha を作り直せる（K5）。証跡の `base_sha` はこの commit の sha で、親が start と T を示す。

### D5 Unavailable（合格にしない）
次のどれかなら評価せず、health の項目を `Unavailable(facts_unreadable:health_base: <理由>)`（verdict Error）にする。loop D3.4 のとおり repair に回さず Blocked と受信箱で止める。
- 作業ツリーの先頭の git の repo に案件の repo の参照があるのに、`review_target_ref` が target を返さない（`Ok(None)`）・読めない（`Err`）。
- head・start を commit に解決できない。
- `git merge-base --all`・`git merge-base --is-ancestor`・`git merge-tree`・`git commit-tree` が起動できない、または想定外の exit（merge-base は 0 か 1、is-ancestor は 0 か 1、merge-tree は 0 か 1、commit-tree は 0 以外が失敗）。merge-tree が `--write-tree` を受け付けない古い git も含む。
- `merge-base --all` が start の祖先でない commit を含む 2 件以上を返す（D1.4）。
- 衝突した path が検査器の file に当たる（D3.5）。
- 衝突時の 2 回目の検査が Error（D3.2 で項目も Error）。

取り込みが無いと分かる場合（base = start）は次の 3 つだけ: 案件の repo の参照が無い作業場所（review 前の同期の対象でない。2026-11-10 の付記と同じ）、merge-base の exit 1（D1.5）、T がすべて start の祖先（D1.3）。

### D6 証跡・再利用・上限
- D6.1 証跡は今どおり 1 項目の `HealthEvidence`（loop D2.1）で、`base_sha` は D2 の base（D4 の commit の sha か T か start）。任意欄 `intake: Option<HealthIntake>` を足す（serde default。旧い event は `None` として読める）。`HealthIntake { start, target, conflicted: Vec<String>, against_target: Option<TargetCheck> }`、`TargetCheck { verdict, report_sha256, blocking }`。`conflicted` は衝突した path（sort 済み、上限 50 件、超えた数は `reason`）、`against_target` は D3 の 2 回目の結果。base を start のまま決めたときは `None`。
- D6.2 欄を足す理由: D4 の commit はどの ref にも指されないので、証跡だけを読む人と reviewer が、base が何から作られたか（start・T・衝突）を知るには記録が要る。2 回目の結果も report の digest に結び付けないと、合格の根拠を後から照合できない。
- D6.3 `blocking` は 2 回の fail 級の fingerprint の和集合（sort・重複除き）。loop D4.3 の往復の判定と受信箱の `set_sha256` はこの集合で数える。`reason` は 1 回目の指摘の後に、2 回目の指摘を base が T であることを示して並べる。
- D6.4 再利用（loop D2.5）の鍵は変えない。D4 の commit の sha は親として start と T を含み、衝突の有無も start・T・git で決まるので、鍵の `base_sha` が同じなら 2 回目も同じ入力になる。Pass の項目だけを再利用する（今どおり）。
- D6.5 上限は今のまま数える（loop D4.1〜D4.4）。新しい欄・config は作らない。

### D7 I3（候補が検査器・policy を弱めて合格にできない）を保つ理由
- base の tree は start・T・git の merge だけで決まる（K1）。start は今の base と同じく候補の外で決まり、T は target から辿れる commit なので承認済みの側にある。worker の commit と worker の merge commit の tree は base に入らない。
- worker が選べるのは、どの target の commit を取り込むか（T）だけ。どの T でも target の承認済みの内容で、worker の内容は head にしか無い。head と base の違いはすべて差分に数える（K2）。
- worker が merge commit で検査器・policy を変えた場合（target の版を戻す、弱める）、base は target の版を含むので、その変更は `base..head` に残り `policy.changed` になる。merge commit の tree を base にしない（D8 の d）ので、worker の検査器が検査器になることも無い。
- 両側の検査器が衝突なく merge された場合、1 回目の検査器はどちらの側にも無い版（両側の承認済みの変更の機械的な merge）になる。壊れていれば exit 2 で Error になり合格にしない。最終の関門（task の review。review 前の同期の後に base = `target_sha`）は target の検査器で評価し直すので、merge された検査器が delivery の関門になることは無い。
- target の branch が書き換えられて（親の branch の rebase など）merge-base が古くなっても、T は start の祖先に戻るだけで、base は今の規則（start）になる。合格しやすくはならない。

### D8 退けた案
- (b) base を常に `merge-base(head, target)`（= T）にする: target を取り込まない WU・段では T は Task の branch が target から分かれた点になり、start より前になる。前の WU・段で統合済みの変更まで今の WU の変更として数え、K3（取り込まない WU・段の base は今と同じ）に反する。取り込んだ WU でも、Task の branch の変更（start が T に無い分）を常に数える。D3 では、衝突のときに限って補いの検査として使う。
- (c) base を start のまま、取り込んだ target の commit（`start..T`）が変えた path を差分から除く: target と worker が同じ file を変えたとき（R4 の衝突した 11 file。fail 級のうち `dispatcher.rs` の `size.responsibility-growth` はその 1 つ）、path を除くと worker の解き方も検査から外れ（K2 に反する）、除かないと target の変更を数える。path の単位では区別できない。差分の口を検査器と Celeris の 2 か所に持つことになり（loop D1.3 の口は 1 つ）、report/1 の入力も変わる。
- (d) base を worker の merge commit（HEAD の履歴の中で T を親に持つ commit）にする: merge commit の tree は worker が作るので、検査器と policy（base の版）を候補が決めることになり I3 に反する。衝突の解き方は merge commit の中にあるので差分から消える（K2 に反する）。ff・rebase・複数回の merge では merge commit が 1 つに決まらず、親を辿る分岐が要る（D1.2）。
- (e) 取り込んだ target と同じ内容の policy の file を `policy.changed` の例外にする: 症状の出た rule ごとに特例を足すことになり、`progress.index`・`size.*`・`source-size` の同じ問題が残る。
- 2026-11-10 の付記の規則（統合の前の HEAD で M を求め、P が M の祖先なら M）を残して WU の check にだけ新しい規則を足す案: base の決め方が経路ごとに 2 つになり、段の統合で unit が target を merge した場合が直らない。D2.1 が付記の場合を含むので、1 つにする。

### D9 撤去対象（impl-base が同じ変更で消す）
- `crates/task-dispatch/src/health_review/check_item.rs` の `phase_health_base`（統合の前の HEAD H と target の merge-base M で決める規則）。D1・D2 の 1 つの関数に置き換え、WU の check と段の統合の両方がそれを呼ぶ。
- `Dispatcher::work_unit_health` の `base: Ok(wu.base_commit)` の固定。start として渡し、target は段と同じ `phase_review_target`（名前は経路に依らないものにする）で引く。
- `phase_target` が merge の前に base を決める流れ。start と target は merge の前に読み、base は `HealthTarget::at_head`（head が決まった後）で決める。
- `HealthTarget.base` などの doc comment の「WU は `wu.base_commit`」「段の統合は `phase_health_base` が決めた base」。
- 旧い挙動を固定する試験: 取り込みのある head で base が start（`wu.base_commit`・P）になることを確かめる試験と、「WU の check は target を取り込まない」を前提にした試験。`check_item_phase_base_tests.rs` の `health_enforce_phase_base_*` 5 件の不変条件（ff の取り込みは数えない・worker の policy の変更は落ちる・worker の fail 級は落ちる・取り込みが無ければ start・target を解決できなければ合格にしない）は新しい関数の試験に移す。
- 2026-11-08 ADR の D1.2 の表の WU の check・段の統合の行の base の記述と、2026-11-10 の付記（この ADR と同じ変更で「置き換え済み」の注記を足した）。`docs/architecture-map.md` の health の行に旧い規則があれば close-out が直す。

### D10 実装の場所と試験
- 関数は `crates/task-dispatch/src/health_review/` の下の新しい module（例 `health_review/base.rs`）に置く。`check_item.rs` に足さない（既に大きい）。git は `repo_facts` と同じく closure か小さな port で渡し、試験で git の失敗を作れるようにする。結果は `HealthBase { Start(sha) | Target(sha) | Merged { sha, start, target, conflicted } }` のような型にし、`evaluate` は `Merged` で `conflicted` が空でないときだけ 2 回目を走らせる。
- 試験の prefix は `health_base_`。実際の git の repo と既存の stub の検査器（`health_review/stub_checker.rs`）で、WU の check と段の統合の両方について次を確かめる。
  - target から分かれた WU が target を衝突なく merge し、取り込んだ target の変更（policy の file を含む）だけなら合格。同じ head を base = start で評価すると落ちることも同じ試験で確かめる（規則を外すと落ちる）。
  - 衝突した merge で、worker の解き方が fail 級の指摘を足せば落ちる（1 回目か 2 回目）。merge の後の worker の commit の fail 級も落ちる。
  - worker が merge commit で policy の file を target の版から変えれば落ちる（I3）。
  - target を取り込まない WU・段は、証跡の `base_sha` が start と一致する（K3）。
  - target を解決できない・git が失敗する・merge-base が複数・検査器の file の衝突は Error で合格にしない（K4）。
  - 同じ入力で base の sha が同じ（K5）。
- 試験を足す理由: 今の試験（`health_enforce_phase_base_*`）は ff の取り込みと統合の前の HEAD だけを作り、WU の branch が target を merge する場合と、target から分かれた branch への merge を作らない。そのため R4 の形の不具合を検出できない。

## 本番への移行
- config・DB の表の変更は無い。`HealthEvidence` の任意欄 `intake` は serde default で、旧い event はそのまま読める。
- 実装を含む release を昇格した後、R4 の WU sync-policy（01M74TGFAQM3QZKFXMQER7SMTD）の check を新しい daemon でやり直す。再開の手順（受信箱 `health_repeat-01M6TYV9DTZESAJJMXRQVGZ56V-e0d14cc6eed8651b`・`authorization-01M79A6FVE7C9BRZ2JPVDF2RVN` への回答と確認の方法）は、この task の close-out の進捗ファイルの「本番への移行」節に書く。

## 状態の記録
- 2026-11-13: 採用。unit adr-base で書いた。2026-11-08 ADR の D1.2 付記（2026-11-10）に「置き換え済み」の注記を足した。実装と食い違いが出たら close-out で付記に書く。
