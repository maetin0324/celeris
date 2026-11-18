---
title: アーキ v4 C metrics — 共通受入条件 (1)〜(6) の before/after と risk の規則の replay（設計 review の回数と費用）
tasks: [01M7MZTKB29VF5TX7FCQ15XEMW]
status: done
updated: 2026-11-18
completed: 2026-11-18
---

# metrics（WorkUnit）

完了日: 2026-11-18。計測は 2026-11-18T07:45Z〜08:01Z（host home-dev、TZ は UTC）。code・profile は変えていない（この WorkUnit の差分はこの file だけ）。
A の baseline（A の run の artifacts の `baseline.md`）§6・§7 のコマンドと規則をそのまま使った。道具は `$TMPDIR` と run の artifacts の `raw/` に置き、commit していない。

- before: main の commit `48b700ed`（件名「integrate wu/closeout (phase closeout)」。B の closeout）。件名「ADR: PathClass に任意の欄 invariants を先に足す決定
  （アーキv4 C の D2・D6 の一部）を記録する」の commit の第 1 親で、C の変更が main に入り始める直前の main。`git merge-base HEAD main` は task branch を
  main へ fast-forward した後の commit（件名「integrate 01M7MZTKB2 profile: …」）なので、profile の差分を落とす。使わない。
- after: 提出時の HEAD（件名「integrate wu/prompts (phase impl)」）。並行の WorkUnit `cases`（6 ケースの試験）の変更はまだ入っていない。
- 除いた commit: 0 件。before..HEAD の 30 commit は、C の WorkUnit（adr・profile・risk-rule・review-wire・review-gate・prompts）の commit と統合の merge、
  前提の task 01M7NZW0BS の 4 commit（件名「ADR: PathClass に任意の欄 invariants …」「PathClass に任意の欄 invariants を足し、往復試験と schema を更新する」
  「repair: PathClass.invariants の進捗に …」2 件）、operator の hand merge（件名「integrate 01M7MZTKB2 profile: …」）だけ。objective の定義どおり前提の task と
  hand merge は C の変更に含めた。ほかの task の commit は区間に無かった。

## 結論と人が決めること

- (1)(2)(3) は before と HEAD で同じ値。C は構造（import・循環・SQLite の参照・公開面）を変えていない。循環は 17 件・単位 198・組 756・行 1,827 で、B の後の値と同じ
  （A の値は 17 件・192・742・1,731。差は B の数え方の変化）。
- (4) C の差分の production は 17 file・8 module・3 crate（task-core・task-dispatch・task-worker）。
- (5) risk の replay: 2026-10-28T00:00Z 以降の main の first-parent の非 merge 499 commit のうち、class に合う commit は before 80 → after 94（+14）。
  web/・docs/ だけの 10 commit と agent-docs/ だけの 213 commit は before・after とも 0 件。増える設計 review は +14〜+94 回／20.9 日で、A の reviewer run の中央値で
  見積もると $28.66〜$192.42・4.4〜29.7 時間（wall）／20.9 日。
- (5) health の時間は B の後より S1 +2.21 s（+2.4%）・S2 +4.99 s（+5.6%）長かったが、C は deps・size の rule を変えておらず、同じ時刻の対照を取れなかった（理由は §5.4）。
  原因は未確認で「要判断」。
- (6) LoC: production 全言語 +662、production Rust +662、own_total_with_tests +2,102。**production Rust の純増 +662 行は ADR の目安「400 行以内」を超えた**
  （内訳は §6。review-gate +370、prompts +181、review-wire +94）。
- 未達/要判断: test-affected・cargo の build の時間、reviewer run の費用の after、final review の設計の節の費用、health の時間の差の原因（§5.5）。
- 人が決めること: (a) production Rust +662 行（目安 400 行の超過）を受け入れるか、closeout か H で削るか。(b) 設計 review の増加（上限の見積もり $192／20.9 日）を
  受け入れるか。決めるのは closeout（人への報告）で、この WorkUnit では直していない。

## 1. import・依存・循環の内部の辺（A の §7 C1・C2）

C1（`repo_health.py --base <rev> --head <rev> --policy-version 1`）は 3 通りとも verdict=pass・exit 0・findings 697（new 0・worsened 0）。rule ごとの件数は
deps.cycle 17・size.responsibility-growth 94・dup.contract 210・arch-map.wrong_owner 31 で、A の §2 と同じ。3 通り = before の検査器 × before、
HEAD の検査器 × before、HEAD の検査器 × HEAD（並列で 07:45:02Z〜07:46:49Z。値を見るだけで時間は比べない）。

| 循環 | A（旧い検査器 × `bc665168`） | B の後（B の検査器） | C の before（HEAD の検査器 × `48b700ed`） | C の after（HEAD の検査器 × HEAD） |
|---|---|---|---|---|
| 件数 | 17（Rust 14・web 3） | 17（Rust 14・web 3） | 17（Rust 14・web 3） | 17（Rust 14・web 3） |
| 単位・組・行の合計 | 192・742・1,731 | 198・756・1,827 | 198・756・1,827 | 198・756・1,827 |
| task-core | 55・210・617 | 55・210・617 | 55・210・617 | 55・210・617 |
| task-api | 49・261・628 | 49・261・632 | 49・261・632 | 49・261・632 |
| task-dispatch dispatcher | 3・4・22 | 4・7・51 | 4・7・51 | 4・7・51 |
| task-worker | adapter 3・3・5、browser 6・12・19 | adapter 12・24・85 | 12・24・85 | 12・24・85 |
| task-dispatch project_controller | 無し | 2・2・2 | 2・2・2 | 2・2・2 |

- before と HEAD で、行の単位の辺 13 本の行番号が変わった（`health_review/design_gate.rs`・`health_review/input.rs`・`task-core/src/execution_plan/design_brief.rs`）。
  (src の file, src の単位, dst の単位, kind) の多重集合は 1,827 本で一致した。辺は増えも減りもしていない。
- size.responsibility-growth の dispatcher の単位の本体の行は 32,347 → 32,338（−9）。件数は 94 のまま。
- Cargo の依存（`Cargo.toml`・`Cargo.lock`）は before..HEAD で変わっていない（`git diff --stat` に出ない）。

## 2. 共有状態・SQLite を直接参照する実装 file（A の §7 C3・C4）

before と HEAD で全部同じ値（`raw/c3-c6.txt`）。

| 指標 | A | before | HEAD |
|---|---|---|---|
| dispatcher/ の非試験 file・self.store（file/出現）・self.config | 69・44/633・35/218 | 69・44/633・35/218 | 69・44/633・35/218 |
| 論理 scope（A の §6 の 1） | 76・48/661・39/224 | 76・48/661・39/224 | 76・48/661・39/224 |
| `rusqlite` を書く production file（store/ を除く） | 74 | 74 | 74 |
| SQL 文字列を持つ production file（同） | 50 | 50 | 50 |

## 3. HTTP・CoS の相互 import と SQL の公開面（A の §7 C2・C5・C6）

| 指標 | A | before | HEAD |
|---|---|---|---|
| `git grep` handlers→cos の行 / cos→handlers の行 | 22 / 52（crate:: 48 ＋ super:: 4） | 22 / 52 | 22 / 52 |
| 単位の辺 handlers→cos / cos→handlers（C2 の task-api の SCC） | 22 本・13 file / 48 本・9 file（旧い検査器） | 22・13 / 52・9 | 22・13 / 52・9 |
| `&Connection`・`&Transaction` の引数（task-core / task-api / task-ops / celeris / llm-proxy） | 320（76 file、store/ 205）/ 14（9）/ 7（4）/ 1 / 4 | 同じ | 同じ |
| `Fn(&Connection)`（task-core / task-ops / task-api） | 13 / 5 / 4 | 同じ | 同じ |
| `impl SqliteStore`（path 付き含む）・trait の impl | 102（88 file）、105・6 | 同じ | 同じ |
| SqliteStore の method（pub / pub(crate) 等 / private） | 633 / 67 / 42 | 同じ | 同じ |

## 4. 変更の波及（C の差分。A の §7 C7b・C10 と同じ分類）

C の差分（before..HEAD、`--no-renames` で rename は両側）を base の source-size-report の分類で分けた（`raw/c4diff.py`、`raw/c4-diff.txt`）。

| class | file | 追加 | 削除 | 純増 |
|---|---|---|---|---|
| production | 17 | 937 | 264 | +673（うち inline test 11 行） |
| external_test | 16 | 1,393 | 67 | +1,326 |
| tooling（`health/profile.py`・`profiles/celeris.toml`） | 2 | 108 | 5 | +103 |
| schema | 5 | 67 | 11 | +56 |
| generated（`web/api/generated/schema.json`） | 1 | 2 | 2 | 0 |
| docs（ADR・進捗） | 11 | 993 | 7 | +986 |

- production の module（repo-health の単位）8 個: task-core の design_review・execution_plan、task-dispatch の dispatcher・health_review・review、
  task-worker の change_rules・claude_code・protocol。crate 3 個: task-core・task-dispatch・task-worker。
- C10 の snippet を before..HEAD に当てた（SINCE 2026-11-17T00:00:00Z）: 非 merge 4 commit（前提の task）のうち production を変えたもの 1、複数 region 0。
  merge を含む 7 commit では production を変えたもの 2、複数 region 1（件名「integrate wu/prompts (phase impl)」。risk-rule〜prompts の差分が 3 crate にまたがる）。
- C7b（変える単位を import する単位、HEAD の検査器）: before・HEAD とも S1 direct 93・transitive 365・packages 12、S2 direct 6・transitive 14・packages 4。
  B の後と同じ（A は S1 92・369・12、S2 6・14・4。差は B の数え方の変化）。
- 代表シナリオの risk の class（before → HEAD の profile）: S1 は `store/org.rs` が store で前後とも class に合う。S2 は `run_settlement.rs`・`run_settlement/work_unit.rs`
  が lease・budget で前後とも合い、HEAD では `dispatcher/worker_finish.rs` が worker-finish にも合う。旧い規則は `new_*` の宣言が無ければ Low、新しい規則では
  S1・S2 とも宣言によらず High になる。

## 5. review・health（時間と費用）

### 5.1 risk の replay（設計 review の対象の件数）

道具 `raw/replay_c.py`（使い捨て）は、main の first-parent の commit（`--since=2026-10-28T00:00:00Z`、`--no-renames` で rename の両側の path が出る）に、
before の `celeris.toml` の `[review].high_risk`（7 class）と HEAD の `celeris.toml` の `[review].high_risk`（13 class）を当てる。ADR の replay の道具は D2 の表を
手で書いていたので、実 profile から class を読むように替えた（値は ADR と同じになった）。REV は before の commit（C の commit を含まない区間）。

| 集合 | commit 数 | before の class に合う | HEAD の class に合う | HEAD だけで合う |
|---|---|---|---|---|
| first-parent の非 merge | 499 | 80（16.0%） | 94（18.8%） | 14 |
| first-parent 全部（merge は第 1 親との差） | 665 | 110 | 129 | 19 |
| web/・docs/ だけを変えた commit（非 merge・全部） | 10・11 | 0 | 0 | 0 |
| うち web/ だけ（非 merge・全部） | 8・9 | 0 | 0 | 0 |
| agent-docs/ だけを変えた commit（非 merge・全部） | 213・265 | 0 | 0 | 0 |

- class ごと（非 merge）: before は store 60・credential 12・sandbox 8・budget 8・events 7・selfdeploy 5・lease 4。HEAD で足した class は authz 13・review-gate 10・
  worker-finish 6・transaction 4・event-apply 2・browser 0。HEAD だけで合う 14 件の class は authz 6・review-gate 5・worker-finish 3（`raw/replay-c.txt` に件名）。
- before で合い HEAD で合わない commit は 0 件（class を狭めていない）。
- REV を main（C の前提の task と profile の hand merge を含む）にすると非 merge 503 件、before 80・HEAD 95。増えた 1 件は前提の task の commit
  （`design_review.rs` を変え、review-gate に合う）。

### 5.2 設計 review の回数と費用の見積もり

- 旧い規則の実績（本番 DB を `mode=ro` で読んだだけ）: agent-platform 案件（project 01M2WTS3DKNZBSZ2JMVB4CZMBW）で、2026-10-28 から before の commit 時刻
  （2026-11-17T21:45:07Z）までの `design_review_requested` は 39 件・9 task（最初は 2026-11-09T01:36Z。設計 review が入った日）。level は 39 件とも high、
  理由は `path_class` 131 個・`new_fallback` 2 個。`design_reviewed` も 39 件。計画の単位で、commit の単位に対応づけられない。
- 増える設計 review の回数（commit の単位、20.9 日）: 下限 +14（before の class に合う 80 件がすべて旧い規則でも High だった場合）、上限 +94（80 件のどれも
  `new_*` を宣言していなかった場合）。旧い規則で High になった commit の数は commit から分からない（未確認）。commit は task より細かいので、task の数ではこれより少ない。
- 費用（A の §5.4 の reviewer run の中央値 wall 1,137.5 s・$2.047 を 1 回に当てた）:

| | 回数／20.9 日 | 費用 | wall の合計 | 1 週あたりの費用 |
|---|---|---|---|---|
| 下限 | +14 | $28.66 | 15,925 s（4.4 時間） | $9.60 |
| 上限 | +94 | $192.42 | 106,925 s（29.7 時間） | $64.45 |

- D3（final review の差分の設計の節）は既存の final review の reviewer run の中に設計の節を足す。run の数は増えない。節が足す token と費用は測っていない（§5.5）。

### 5.3 reviewer prompt の health 節の大きさ

- 変わったのは finding の行だけ。`repo_block`（diff・owner 地図・policy・不変条件の行）の source は before と HEAD で同じ（関数の本文の md5 が一致）。
  HEAD は new・worsened の finding ごとに ` fingerprint=…`・` symbol=…`（最初の evidence の symbol）と ` - rationale: …` の行を足す。
- 道具 `raw/health_section_size.py` で、§5.4 の S1・S2 の report（HEAD の検査器、probe の const 入り）の finding を before と HEAD の書式で描いた:

| report | new・worsened（例外なし） | before の finding の行 | HEAD の finding の行 | 差 |
|---|---|---|---|---|
| S1 | 4 | 403 字 | 1,171 字（1,585 B） | +768 字（+1,182 B） |
| S2 | 7 | 769 字 | 2,134 字（2,706 B） | +1,365 字（+1,937 B） |

- この 2 つの report は reviewable=false なので、`summarize_report` は finding を材料にしない。S1・S2 の実際の health 節の差は 0。上の差は reviewable=true の
  report に同じ finding が出た場合の値で、finding 1 件あたり約 190〜195 字増える。上限は `MAX_FINDINGS` 50 件 × 約 195 字 ≒ 9,750 字（rationale の長さで変わる。推定）。

### 5.4 health の時間（A の §5.5・§7 C8 の手順、HEAD で各 3 回）

scenario の各 file の末尾に `SCENARIO_PROBE_<k>` の const を足し（毎回違う値）、HEAD の検査器で `repo_health.py --repo . --base <HEAD> --head worktree --policy-version 1`
の wall を `/usr/bin/time` で測り、測った後に `git checkout -- <files>` で戻した（`raw/health_time.sh`、log は `raw/health-timing.log`。最後の `git status --porcelain` は 0 行）。
各回の直前に 1 分 load average が 4 以下になるまで待った（待ち時間はどの回も 0 s）。

| | run 1 | run 2 | run 3 | 中央値 | user（中央値） | max RSS | 開始時の 1 分 load | verdict |
|---|---|---|---|---|---|---|---|---|
| A（旧い検査器）S1 | 87.82 s | 86.56 s | 81.48 s | 86.56 s | 69.45 s | 611 MB | 0.25〜1.28 | fail: new 3・worsened 1 |
| A（旧い検査器）S2 | 81.76 s | 81.92 s | 85.96 s | 81.92 s | 69.46 s | 612 MB | 同上 | fail: new 5・worsened 2 |
| B の後 S1 | 93.84 s | 88.88 s | 92.81 s | 92.81 s | 71.04 s | 639 MB | 1.97 / 1.59 / 2.50 | fail: new 3・worsened 1 |
| B の後 S2 | 88.68 s | 85.14 s | 88.74 s | 88.68 s | 71.23 s | 640 MB | 1.87 / 2.13 / 2.00 | fail: new 5・worsened 2 |
| C の after（HEAD）S1 | 93.40 s | 96.50 s | 95.02 s | 95.02 s | 71.97 s | 641 MB | 0.92 / 2.08 / 2.27 | fail: new 3・worsened 1 |
| C の after（HEAD）S2 | 91.08 s | 94.34 s | 93.67 s | 93.67 s | 71.62 s | 642 MB | 1.21 / 3.73 / 2.85 | fail: new 5・worsened 2 |

- 時刻: 2026-11-18T07:49:48Z〜07:59:13Z。B の後との差は S1 +2.21 s（+2.4%）・S2 +4.99 s（+5.6%）、user の CPU は +0.93 s・+0.39 s。A との差は S1 +8.46 s（+9.8%）・
  S2 +11.75 s（+14.3%）。判定（findings・new・worsened）は B の後と同じ。
- C は deps・size の rule を変えていない（検査器の差分は `health/profile.py` の `_validate_review` の検査と `celeris.toml` の `[review]` だけ）。wall の差は CPU の差で
  説明できず、並行の WorkUnit `cases` の負荷が混じった可能性がある（未確認）。
- 同じ時刻の対照（before の検査器で同じ手順）は取れなかった。before の検査器は HEAD の profile の `high_risk[]` の `invariants` を読めず、`profile_invalid`（exit 2）で
  止まる（`raw/health-time-ctl-ckbef-S1-1.out`）。この比較は「要判断」とし、悪化とも改善とも書かない。

### 5.5 前後比較していない項目（未達/要判断）

| 項目 | before（A） | 理由 | 次の候補 |
|---|---|---|---|
| test-affected の時間（S1 893.41 s・S2 655.59 s） | A の §5.5 | この WorkUnit の 60 分の予算では各 3 回（合計 70 分以上）を測れない。C は crate の依存を変えず、S1・S2 の file も変えていない | G（反復速度の計測）が同じ手順で測る |
| cargo check・test build の cold・warm | A の §5.3 | 同上。C は task-core（`design_review.rs`）を変えたが、build の範囲（逆依存の crate）は変わらない | G |
| reviewer run の時間と費用の after（C11） | wall 中央値 1,137.5 s・$2.047 | C は昇格していない（D1・D3 は昇格した release の daemon でだけ効く）。期間の実績を前後に分けられない | 昇格の後に同じ長さの期間で C11 を当てる（H） |
| final review の設計の節（diff_design）が足す token・費用 | 無し | 節は覆われない (class, path) があるときだけ付く。実 run の記録がまだ無い | 同上 |

## 6. LoC（A の §7 C12。分類規則は base に固定）

`scripts/dev/source-size-report.py` と `source-size-report.toml` は before の commit から `git show` で取り出した（`bc665168` の版と `cmp` で同じ）。
`python3 -I $TMPDIR/ssr.py --root . --config $TMPDIR/ssr.toml --exclusive-classes --rev <rev> --format json`（`raw/ssr-{bef,head}.json`）:

| | A（`bc665168`） | before | HEAD | 差（HEAD − before） |
|---|---|---|---|---|
| production 全言語（`views.product_only.lines`） | 375,099 | 375,099 | 375,761 | +662 |
| production Rust（`views.product_only.rust_lines`） | 330,408 | 330,408 | 331,070 | +662 |
| own_total_with_tests（`views.own_total_with_tests.lines`） | 1,025,677 | 1,026,070 | 1,028,172 | +2,102 |
| own_total_with_tests の Rust | 879,344 | 879,344 | 881,097 | +1,753 |
| 試験（external_test + inline_test） | 455,363 + 21,408 | 455,595 + 21,408 | 456,921 + 21,419 | +1,326 + 11 |
| tooling | 25,253 | 25,414 | 25,517 | +103 |

production の差（numstat の +673 から inline test の 11 行を引くと +662）の commit ごとの内訳（`raw/prod-by-commit.txt`）:

| commit（件名） | production の純増 |
|---|---|
| arch-v4 C review-gate: final review の差分に base 側の方針で risk を当て、… | +370（うち `health_review/input.rs` が大部分） |
| arch-v4 C prompts: reviewer・worker・planner の prompt に risk の根拠 … | +181（`change_rules/review.rs` ほか） |
| arch-v4 C review-wire: final review の設計の節を ReviewRequest.diff_design で渡し … | +94 |
| arch-v4 C risk-rule: 高 risk の class に合う write path を new_* の宣言によらず High … | +29（ほかに PathClass の欄を main に合わせた commit −1） |
| PathClass に任意の欄 invariants を足し、往復試験と schema を更新する（前提の task） | +3 |

- ADR の「規模の目安」は production の Rust の純増 400 行以内。実測 +662 行で 262 行超えた。leaf `review-gate`・`prompts` の進捗も超過を書き、
  「closeout か H が受け入れるか縮めるかを判断する」と提案している（replan は申告していない）。削るかどうかは人が決める（§「結論と人が決めること」の (a)）。
- 試験の純増 +1,337 行のうち、`health_review/input/tests/risk_tests.rs` +519、`change_rules/review/tests.rs` +353、`design_review/tests.rs` +111、
  `scripts/tests/test_repo_health_review_boundary.py` +246。

## 再現コマンド（repo の root で）

```sh
BEF=$(git rev-parse "$(git log --format=%H --grep='^ADR: PathClass に任意の欄 invariants を先に足す決定' -n 1 main HEAD)^1"); H=$(git rev-parse HEAD)
mkdir -p $TMPDIR/ckbef $TMPDIR/ckhead; git archive $BEF scripts/dev | tar -x -C $TMPDIR/ckbef; git archive $H scripts/dev | tar -x -C $TMPDIR/ckhead
CKH=$TMPDIR/ckhead/scripts/dev/repo-health
# (1) C1・C2（raw/c2.py・raw/c7b.py は B の metrics の道具の写し。第 1 引数が検査器の dir）
python3 -I $CKH/repo_health.py --repo . --base <$BEF|$H> --head <$BEF|$H> --policy-version 1 --out $TMPDIR/health.json
python3 -I raw/c2.py $CKH <$BEF|$H> $TMPDIR/cycle-edges.json
# (2)(3) A の §7 C3〜C6 をまとめた台本
bash raw/c3c6.sh <$BEF|HEAD>
# (4) C の差分の分類と C10・C7b
git show $BEF:scripts/dev/source-size-report.py > $TMPDIR/ssr.py; git show $BEF:scripts/dev/source-size-report.toml > $TMPDIR/ssr.toml
python3 -I raw/c4diff.py $TMPDIR/ssr.py $TMPDIR/ssr.toml $BEF HEAD
for m in no-merges with-merges; do python3 -I raw/c10.py $TMPDIR/ssr.py $TMPDIR/ssr.toml 2026-11-17T00:00:00Z $m "$BEF..HEAD"; done
python3 -I raw/c7b.py $CKH <$BEF|$H> S1:crates/task-api/src/handlers,crates/task-api/src/cos,crates/task-core/src/store S2:crates/task-dispatch/src/dispatcher,crates/task-dispatch/src/run_settlement
# (5) replay・health 節・health の時間
python3 -I raw/replay_c.py . $BEF HEAD 2026-10-28T00:00:00Z <$BEF|main>
python3 -I raw/health_section_size.py raw/health-time-head-S1-1.json raw/health-time-head-S2-1.json
bash raw/health_time.sh $CKH head 3
# (6) LoC
python3 -I $TMPDIR/ssr.py --root . --config $TMPDIR/ssr.toml --exclusive-classes --rev <$BEF|$H> --format json --out $TMPDIR/ssr.json
```

`raw/` は run の artifacts（`…/wu/metrics/artifacts/raw/`）。

## リリースゲート・回帰条件

- リリースゲート: 未実施（統合後の release の gate は人・operator が行う。本番への昇格はしていない）。
- 回帰条件: (1)〜(3) の値が before と同じであること（C は構造を変えない）。replay で web/・docs/・agent-docs/ だけの commit が class に合わないこと（0 件）。

## 証拠（提出時の HEAD で実行）

- 範囲: `git diff --name-only "$CELERIS_WU_BASE"` → `agent-docs/progress/2026-11-17-arch-v4-c/metrics.md` だけ。
- `sh scripts/dev/progress-index.sh --check` → exit 0。
- code を変えていないので test-affected・clippy・fmt は回していない（差分は進捗の file だけ）。
