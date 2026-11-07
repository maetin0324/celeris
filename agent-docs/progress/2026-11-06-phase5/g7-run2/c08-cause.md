---
title: G7 2 回目の C08 で到達の後に CoS が 3 回起きた原因の切り分け
tasks: [01M6VMSFP1R7DSH9T8PB4GND0Q]
status: done
updated: 2026-11-07
completed: 2026-11-07
---
# C08 の `cos_invocations_after_achieved` = 3 の切り分け

完了日: 2026-11-07。基準は `$CELERIS_WU_BASE`。この WU はコードも仕様も変えていない。実 LLM・本番の操作もしていない。

## 結論
- 到達（`achieved_at` 147）の後の CoS run 3 本（t=156・164・173）は、**controller が起こしたものではない**。到達の検出も遅れていない。drain は DB を読むだけで入力を作らない。
- 残る経路は、controller の mode と無関係に動く**案件の event の経路**（root の完了の要約依頼 root_done と、通知・受信箱の一次対応 triage）だけで、3 本はこれと推定する。3 本とも drain の待ちの間（t=154.2〜194.3）に、daemon の通常の tick が積まれていた入力を順に拾って起動した。
- **未確認**: 3 本がそれぞれどの入力（o4 の root_done か、どの通知の triage か）で起きたか。試行の一時 DB と daemon.log は script の終わりに消え、facts の `invocations` に入力の出所の欄が無い。
- **N6 の判定への影響は無い**。G1〜G7 に C08 の oracle は入らず、3 本は usage があるので G7（`cos_usage_null` 0）は pass のまま。N6 は G2・G2s・G2b・G6 が fail で no-go のまま。直す場合の提案（ADR の要否つき）は [n6.md](../n6.md) の「提案」節に書いた。

## 使った raw
- facts: [raw/facts/](raw/facts/) の 2 file のうち C08 の方（[raw/arm-map/](raw/arm-map/) で case C08・arm new・seed 1 の file 名と同じ名前）
- ops: [raw/ops/](raw/ops/) の同じ名前の json と [raw/ops/C08-new.log](raw/ops/C08-new.log)
- 比較: 同じ dir の C06 の facts・ops と、[../g7-run1/raw/](../g7-run1/raw/) の facts・ops

時刻の読み方（`scripts/dev/long-horizon-pilot/driver.py` を読んで確かめた）:
- 時刻は試行の開始 t0（driver の `self.t0`）からの秒。facts の `invocations[].at` は `chat_runs.started_at`（`chat_run_claim_next` が入力を run に結んだ時刻）を整数に切り捨てた値で、起動の開始の側。`slot_wait_secs` は `launched_at − started_at`。
- `achieved_at` は `pc/achieved` の判断の `created_at`。ops の `at` は driver が観測した時刻（0.1 秒単位）。
- UTC の時刻は推定。drain の `deadline_at` 2026-11-07T18:39:54Z は t0 + 450（`--max-wall-secs 450`）で決まる（承認の側の期限 18:28:32Z + 900 秒や 2026-11-08T12:00Z より早い）。ここから t0 ≈ 18:32:24Z と逆算した（±1 秒）。C06 の deadline 18:37:40Z から出る C06 の t0 18:30:10Z に C06 の wall 133 秒を足すと 18:32:23Z になり、矛盾しない。

## 時刻の並び（C08）
| t0 からの秒 | UTC（推定） | 出来事 | 出典 |
|---|---|---|---|
| 119 | 18:34:23 | o4 の root 作成・worker 起動 | facts `roots[3].created_at`・`invocations` |
| 129 | 18:34:33 | o4 の reviewer 起動 | facts `invocations` |
| 139 | 18:34:43 | CoS run（到達前の最後）。起動は 145（`slot_wait_secs` 6）、`session_resumed` true | facts `invocations` |
| 146.1 | 18:34:50 | driver が root 4 本とも done を観測（129.9 の観測では o4 が reviewing） | ops `state` |
| **147** | 18:34:51 | `pc/achieved` の判断（`achieved_at`） | facts `achieved_at`、ops `rules` の最後 |
| 148.1 | 18:34:52 | driver が phase `achieved` を観測し、`wait_end` を抜ける | ops `state` |
| 151.1 | 18:34:55 | 到達後の wake 1 件（`pilot-after-end`）の 3 秒後。新しい root 0 | ops `wake_after_end` |
| 151.2 | 18:34:55 | mode を `off` に（「従来の CoS に戻す」）。off の後の wake は `added` false | ops `set_mode`・`wake_after_off` |
| 154.2 | 18:34:58 | 判断の数 9 → 9（増えていない）、`pending_wakes` 1。drain を始める | ops `after_off` |
| **156** | 18:35:00 | 到達後の CoS run 1 本目。起動待ち 0 秒、`session_resumed` true、0.0435 USD、output 878 token | facts `invocations` |
| **164** | 18:35:08 | 2 本目。起動待ち 0 秒、0.0501 USD、output 931 token | 同上 |
| **173** | 18:35:17 | 3 本目。起動待ち 0 秒、0.1018 USD、output 1,738 token | 同上 |
| 194.3 | 18:35:38 | drain の終わり（`waited_secs` 40.1、`timed_out` 0）。未完了の run が 0 になってから 2 秒（`--drain-settle-secs` の既定）待って終わる | ops `drain`、facts `drain` |

3 本の費用の和は 0.1954 USD で、2 回目の合計 1.717436 USD に含まれる。

## 起動ごとの原因
3 本とも同じ判定になる。各候補を除いた根拠と、残る経路を下に書く。

| 起動 | 時刻 | controller | 到達の検出の遅れ | drain の待ちの間の tick | 案件の event |
|---|---|---|---|---|---|
| 1 本目 | t=156（18:35:00Z ごろ） | 違う | 違う | 起動の場（入力は作らない） | **推定**（どの入力かは未確認） |
| 2 本目 | t=164（18:35:08Z ごろ） | 違う | 違う | 同上 | **推定**（同上） |
| 3 本目 | t=173（18:35:17Z ごろ） | 違う | 違う | 同上 | **推定**（同上） |

### controller ではない（確認済み）
- controller が案件 CoS に入力を積むのは判断 `replan` のときだけ（`crates/task-dispatch/src/dispatcher/project_decider_ts.rs` の `ProjectAction::Replan` → `ProjectEffect::QueueCosInput`。積む関数 `project_controller_queue_cos_input` の呼び出しは `dispatcher/project_wake.rs` の 1 か所）。
- C08 の判断 9 件（ops `rules`）は `pc/outcome-open` 4・`pc/milestone-reached` 4・`pc/achieved` 1 で、`replan` は 0。よって C08 の CoS run 6 本は、到達の前の 3 本も含めて controller の入力ではない。
- 到達の後も判断は増えていない（`decisions_at_end` 9 = `decisions_after` 9）。off の後の wake は積まれていない（`added` false）。t=151.2 から mode は `off` で、controller の効果の門は閉じている。

### 到達の検出の遅れではない（確認済み）
- 最後の root（o4）の done を driver が観測したのが t=146.1、`pc/achieved` の判断が t=147、phase `achieved` の観測が t=148.1。検出の遅れは約 1 秒以内。
- 到達の後の起票は 0（`roots_after_achieved == 0` は pass）。3 本は到達の 9〜26 秒後に始まっており、「到達を検出する前に CoS を起こした」形ではない。

### drain は入力を作らない。ただし 3 本は drain の待ちの間に起動した
- drain（`pilot_drain.drain`・`read_chat_runs`）は DB を SELECT で読むだけで、入力も run も作らない。
- 3 本の開始（156・164・173）は全部 drain の窓（154.2〜194.3）の中にある。drain が daemon の停止を遅らせたので、daemon の通常の tick が待ち行列の入力を拾った。
- drain の無かった 1 回目では、同じ形の run が daemon の停止で終わらず usage null になっていた（g7-run1 の C06: 到達 129 の後の t=134 の CoS run）。drain は「到達後の CoS run を起こした原因」ではなく、「それを最後まで走らせて数えられるようにした場」である。

### 残る経路: 案件の event（推定）
controller の mode と phase に関係なく案件 CoS の thread に入力を積む経路は、code では次の 3 つだけだった（人の発言は new の arm に無い）。
1. root の完了の要約依頼（root_done。[ADR 2026-10-15-root-done-cos-summary](../../../adr/2026-10-15-root-done-cos-summary.md)、`dispatcher/cos_chat/root_done.rs`）: root が done になるたびに、案件の CoS thread に user の入力を 1 件積む。取り込み（delivery）が `Reviewing`・`MergeQueued`・`Merging`・`Preparing` の間は待つ。controller が `enabled` のときは「root を起票しない」という一文を足すだけで、積むこと自体は止めない。
2. 通知・受信箱の一次対応（triage。`dispatcher/cos_chat/triage.rs`、`task-core/src/chat/triage.rs`）: `task_done`・`delivery` などの通知と受信箱の項目を、task の root の案件の CoS thread の項目にし、1 run で最大 20 件を処理する。
3. 中断した run の続き（`cos-recover:`。`dispatcher/cos_chat/control.rs`）: daemon の再起動か持ち主の無い run の回収のときだけ。driver が daemon を起動したのは t0 の直前と t≈0.5（案件を許可リストに入れた config での起動）の 2 回だけで、どちらも最初の CoS run（t=57）より前。その後の再起動は無い。

o4 は t=146.1 までに done になり、その後に取り込みが進む。o4 の root_done の入力と、o3・o4 の通知の triage の項目は、到達（147）の前後に積まれる。案件の CoS thread は live な run を 1 本しか持てない（`chat_run_claim_next` は `active_run_id` があれば何もしない）ので、到達前の run（t=139 開始・145 起動）が終わるまで積まれた入力は待ち、終わると 1 件ずつ run になる。
- 確認済み（code の規則から）: 156 の run は 164 までに、164 の run は 173 までに終わっている。t=139 の run は 156 までに終わっている。173 の run は drain の終わり（194.3）の約 2 秒前、t≈192 に終わった。
- 推定: 3 本が起動待ち 0 秒で前の run の終わりの直後に続いているので、入力は前の run の間に既に積まれていた。各 run の `finished_at` は raw に無い。
- 未確認: 3 本のどれが root_done で、どれが triage か。C08 の CoS run 6 本のうち、root_done（root 4 本で 4 件）と triage が何本ずつかも未確認。

### 未確認の理由
- `scripts/dev/long-horizon-pilot-llm.sh` は試行の一時 home（`mktemp -d …/long-horizon-pilot.XXXXXX`）を終わりの trap で消す。DB（`celeris.sqlite3`）と `daemon.log` はそこにあり、残っていない。
- facts の `invocations` は CoS run ごとに `at`・`usage`・`launch`・`prelaunch_reason`・`slot_wait_secs` だけを持ち、入力の出所（入力の `client_message_id` の接頭辞 `root-done:`・`pc-input:`・`cos-recover:`、triage の項目の `source_kind`）と `finished_at` を持たない。

## 他の試行でも同じ形が出ている
| 試行 | 到達 | 到達後の CoS run | oracle の扱い |
|---|---|---|---|
| 2 回目 C06 | 112 | t=120 の 1 本（usage あり） | C06 の oracle に `no_cos_after_achieved` は無い。`cos_invocations([600,259200]) <= 2` は 0 で pass |
| 1 回目 C06 | 129 | t=134 の 1 本（usage null。drain が無く daemon の停止で終わらなかった） | 同上 |
| 2 回目 C08 | 147 | t=156・164・173 の 3 本 | `cos_invocations_after_achieved == 0` が fail |

到達した試行の 3 つとも、到達の後に CoS run が 1 本以上ある。到達は最後の root の done の後にしか判定されず、root_done はその root の done の後に入力を積む。よって今の製品と今の述語（到達後の CoS run を出所を問わず全部数える）の組み合わせでは、実 LLM の CoS が動く限り C08 の述語はほぼ通らない（推定。製品の規則から導いたもので、試行で確かめてはいない）。

ADR [2026-11-04-long-horizon-bench](../../../adr/2026-11-04-long-horizon-bench.md) の C08 の行は「達成の後に無関係の wake を 3 件送る」→「達成後の起票 0・CoS 起動 0」と書いており、述語の意図は「到達の後の wake で controller が CoS を起こさない」と読める。実 LLM の pilot の driver は到達の後に wake を enabled で 1 件、off の後に 1 件送るだけで、3 件ではない。到達後の CoS run が 3 本だったのは wake の数と偶然一致しただけで、wake が起こしたものではない（上の「controller ではない」）。

## N6 の判定への影響
- **影響なし**。N6 の G1〜G7 の定義に C08 の oracle は入っていない（[n6.md](../n6.md) の判定表）。G7 は `cos_usage_null` 0 で、3 本は usage（費用・token）が記録されているので null に数えられていない。G7 は pass のまま。
- N6 の判定は G2・G2s・G2b・G6 の fail で no-go のまま。この切り分けは go/no-go を変えない。
- 本番の 1 案件の導入（判定が go になった後）について: 到達後の CoS run は、controller が `off` の案件でも動く経路（root_done・triage）から出ている。ADR project-controller の I13（`off` の案件は機構が無い時と同じ動き）のとおりで、controller を入れたことで増える run ではない。
- ただし `[project_controller.budget]` の `max_cos_inputs_per_day` は `replan` の入力だけを数える（`project_decider_ts.rs` の `used("replan")`）。root_done・triage の CoS run はこの上限に入らない。本番の値を決める人は、案件の 1 日の CoS の費用にこれらが別に乗ることを前提にする必要がある。本番の config で root_done・triage が有効かどうかは未確認（本番の config を読んでいない）。

## 提案
[n6.md](../n6.md) の「提案」節に書いた（入力の出所を facts に残す計測の追加、述語の定義を決める ADR、製品側で抑える案を推さない理由、budget の数え方の注意）。

## 検査（提出時の HEAD で実行）
この WU は文書だけを変えた（この file と n6.md）。
- `sh scripts/dev/check-doc-links.sh` → exit 0（`check-doc-links: ok`）
- `sh scripts/dev/progress-index.sh --check` → exit 0（初回は試行 id の 16 桁を commit hash と判定されて落ちた。id を書かない形に直して再実行）
- `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` → exit 0
- `bash scripts/dev/test-affected.sh` → exit 0（「試験対象の変更なし（docs のみ等）」、repo-health verdict pass）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- 範囲: `sh "$CELERIS_WU_SCOPE_PATHS"` の変更 path はこの file と n6.md の 2 つだけ
