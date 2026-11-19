---
title: アーキ v4 G cause — 悪化をコード由来と host 由来に分け、原因を絞り、crate 分離の利益を見積もる
tasks: [01M7MZTKBKXKEVBWS87PS5J9NK]
status: done
updated: 2026-11-19
completed: 2026-11-19
---
# cause（WorkUnit）

入力は [flags.tsv](flags.tsv)（`flag=control` は health:S2 の 1 組だけ）と [control-build.tsv](control-build.tsv)（health:S2 の before を同じ host で測り直した 3 回）。
control-s1・control-s2 は印が無く、行は header だけ（[control-s1.md](control-s1.md)・[control-s2.md](control-s2.md)）。結果の表は [cause.tsv](cause.tsv)
（列 `metric scenario after_median before_control_median verdict`）。測った host は home-dev（24 core）、時刻は 2026-11-19T08:53Z〜09:12Z。

## 0. 結論

- **health:S2 は規則どおり verdict=code**（after の中央値 96.09 s、before の測り直しの中央値 87.26 s。閾値 87.26 × 1.10 = 95.99 s を 0.10 s 超える）。
- ただし、コードが増やした分は **CPU 約 +2.0 s（user +2.9 %、before の wall の +2.3 %）だけ**で、10 % の大半は測った時間帯の差（host）による:
  - before の検査器 × bc665168 と after の検査器 × after の tree を**同時に並べて**走らせると、wall は 91.00 s と 91.00 s で同じ（§1）。
  - CPU の差は 21 の rule のうち **`deps.violation` の 1 つだけ**（自 process の CPU 17.36 → 19.38 s、+2.02 s）。ほかの 20 rule の差は ±0.4 s 以内。
  - `deps.violation` の中の差は `health/lang_deps.py` の Rust の解析で、約 6 割が Rust の file の増加（`rust_string_mask` の呼び出し 1,832 → 1,880、
    解析した file 916 → 940）、約 4 割が B が足した `#[path]` の論理 module の追跡（`_mod_decls`・`_inline_mods` の新しい走査）による（§2）。
- 直す owner は `scripts/dev/repo-health/health/lang_deps.py`（B の変更）。修正案（§3）で縮むのは最大で約 1 s（health の wall の約 1 %）。
  **fix の leaf には「コードを変えない」を推す**。理由は 2 つ。縮む量が測り直しの 3 回の幅（86.19〜107.83 s）より小さい。この file は policy の file
  （`profiles/celeris.toml` の `policy_paths` の `^scripts/dev/repo-health/`）なので、変えると policy だけの別 review と人の差分承認が要る。
- **crate 分離の pilot は推さない**（§4）。S1 の warm の test build で task-core の逆依存の再 build が占める時間は wall の 37.3 %（理想の上限）だが、
  S1 が変える `task-core/src/store/org.rs` は `SqliteStore` の inherent impl で、task-core の中からも呼ばれる。小範囲の分離ではこの file を task-core の外に
  出せないので、見積もりは S1・S2 とも 0 %。理想の上限を 2 シナリオで平均しても build 18.7 %・test-affected 11.6 % で、20 % に届かない。

## 1. health:S2 の verdict と、コード由来と host 由来の分け方

| 組 | after の中央値（compare） | before の測り直しの中央値（control-build の 3 回） | before × 1.10 | verdict |
|---|---|---|---|---|
| health:S2 | 96.09 s（98.73・96.09・90.65） | 87.26 s（87.26・107.83・86.19） | 95.99 s | **code** |

+8.83 s（96.09 − 87.26）の内訳。値はこの leaf と control-build の測定:

| 比べ方 | before | after | 差 | 読み取れること |
|---|---|---|---|---|
| 同時に並べて 1 回ずつ（この leaf、2026-11-19T09:08Z、開始時の 1 分 load 3.99。rule ごとの計時の wrapper の下） | 91.00 s（user 69.51 s） | 91.00 s（user 71.56 s） | wall 0.00 s、user +2.05 s（+2.9 %） | 同じ条件なら wall は変わらない。CPU は +2 s |
| 同じ時間帯に交互に 3 組（control-build、08:31Z〜08:40Z） | 87.26 s（中央値、user 68.81 s） | 88.89 s（中央値、user 70.90 s） | +1.63 s（+1.9 %）、user +2.09 s（+3.0 %） | コード由来の差は +2 % 前後 |
| 測った時間帯の差（compare の after は 05:20Z〜05:30Z、5 分 load 4.4〜4.7） | — | 96.09 s → 88.89 s | −7.20 s | 残りの差は時間帯（host の混み具合） |

- コード由来の差（+1.6〜+2.1 s）は before の +10 %（8.7 s）の 4 分の 1 以下。規則の上は code だが、10 % を超えた分は host 由来の待ち（wall − user − sys。
  compare の after で 10.6〜17.6 s、control-build の after で 9.2〜13.0 s）による。
- health:S1 は印が無い（+8.1 %）ので before を測っていない。S1 と S2 は同じ検査器・同じ rule を走らせ、違うのは probe を入れる file だけなので、
  コード由来の差は S2 と同じ `deps.violation` の +2 s と見込む（未測定）。

## 2. 原因を rule・関数まで絞る

### 2.1 rule ごとの時間（同時に並べた 1 回）

`health.engine.evaluate_rule` を包んで、rule ごとに wall・自 process の CPU・子 process の CPU を測った（wrapper は成果物の `raw/health-rule-timing.py`、
値は `raw/hr-before.tsv`・`raw/hr-after.tsv`）。before は bc665168 の worktree で bc665168 の検査器、after はこの WorkUnit の worktree で after の検査器。
どちらも S2 の 4 file に const の probe を入れ、`--head worktree`。

| rule | before の wall / 自 CPU / 子 CPU | after の wall / 自 CPU / 子 CPU | 自 CPU の差 |
|---|---|---|---|
| deps.violation | 23.61 / 17.36 / 0 | 24.38 / 19.38 / 0 | **+2.02 s** |
| source-size | 26.83 / 0 / 18.46 | 25.76 / 0.01 / 18.39 | 0（子 −0.07） |
| link.unlinked | 9.39 / 9.40 / 0 | 9.46 / 9.46 / 0 | +0.06 |
| doc.links | 6.93 / 0.01 / 7.31 | 7.24 / 0.01 / 7.63 | 0（子 +0.32） |
| size.responsibility-growth | 4.99 / 4.98 / 0 | 4.76 / 4.75 / 0 | −0.23 |
| ほか 16 rule の和 | 18.75 / 15.60 / 3.23 | 18.85 / 15.66 / 3.29 | +0.06 |
| rule の外（snapshot の読み込みなど） | 0.38 | 0.42 | — |
| 合計 | 90.88 / 47.51 / 30.49 | 90.87 / 49.43 / 30.78 | +1.92（子 +0.29） |

`deps.cycle` は 0.02〜0.03 s で、`deps.violation` が作った依存の graph（`tree._rh_dep_graph` の cache）を使う。B が変えた `rules/deps.py`・`rules/size_growth.py`
の rule の本体の差は数えられる大きさではなく、差は依存の graph を作る `health/lang_deps.py` に出た。

### 2.2 `deps.violation` の中（cProfile）

`--rules deps.violation` だけを cProfile の下で before と after を同時に走らせ、関数ごとに突き合わせた（`raw/prof-compare.txt`。cProfile の overhead で
秒は実時間の約 23 倍。比だけを読む）。

| 関数（file:関数） | 呼び出し before → after | 自分の時間 before → after | 累計 before → after |
|---|---|---|---|
| `deps.py:violation`（rule 全体） | 2 → 2 | — | 549.88 → 565.85（+2.9 %） |
| `source-size-report.py:rust_string_mask`（文字列と注釈を伏せる） | 1,832 → 1,880 | 115.43 → 118.05 | 454.95 → 464.18（+9.23） |
| `source-size-report.py:analyze_rust_file` | 916 → 940 | 1.44 → 1.48 | 230.48 → 235.14 |
| `lang_deps.py:_mod_decls`（B が足した `mod`・`#[path]` の走査） | 0 → 4,412 | 0 → 3.45 | 0 → 473.92（下に `rust_code_lines` の cache を含む） |
| `lang_deps.py:_inline_mods`（同上、inline の `mod {}`） | 0 → 1,826 | 0 → 1.45 | 0 → 3.07 |
| `lang_deps.py:_rust`（Rust の import の解析） | 2 → 2 | 3.01 → 4.84 | 479.34 → 501.97 |
| `re.Pattern.finditer` | 798,476 → 1,597,860 | 1.41 → 2.76 | — |
| `re.Pattern.match` | 22,791,530 → 25,908,116 | 40.67 → 46.38 | — |

- 伏せる処理（`rust_string_mask`）は `rust_code_lines` の `functools.lru_cache` で file ごとに 1 回で、二重には走っていない。B は呼び手を
  `rust_code_lines` から `rust_modules` → `_mod_decls` に移しただけで、呼び出しの数の増え方（+2.6 %）は解析した Rust の file の数の増え方（916 → 940）と同じ。
- cProfile の下の差 +15.99 のうち +9.23（約 58 %）が file の増加、残り約 6.8（約 42 %）が B の新しい走査（`_mod_decls`・`_inline_mods` の自分の時間、
  `_rust` の自分の時間の +1.83、`finditer` が倍になった分、`match` の +3.1M 回）。実時間に直すと file の増加 約 +1.2 s、B の走査 約 +0.85 s。

## 3. owner と修正案

| 原因 | owner | 修正案 | 縮む見込み | 推すか |
|---|---|---|---|---|
| B の `#[path]` の論理 module の追跡（約 +0.85 s） | `scripts/dev/repo-health/health/lang_deps.py`（`_mod_decls`・`_inline_mods`・`_rust`） | `_mod_decls` と `_inline_mods` の `mod` 宣言の走査を、`_rust` が import を拾う同じ行の走査（`rust_code_lines` の結果を 1 回なめる loop）にまとめ、`finditer` の 2 回目の走査をやめる。新しい状態・分岐は足さない | 最大 約 0.85 s（health の wall の約 1 %） | 推さない（下） |
| Rust の file の増加（約 +1.2 s） | 増えた file は B〜F の試験と実装（compare.md §2） | 無し（repo の大きさに比例する費用で、悪化ではない） | — | — |

推さない理由:
1. 縮む量（≤ 0.85 s）は before の測り直しの 3 回の幅（86.19〜107.83 s）と、同時に並べた before と after の wall の差（0.00 s）より小さく、測定で確かめられない。
2. `scripts/dev/repo-health/` は policy の file。変えると `policy.changed` が fail になり、policy だけの branch・人の差分承認・operator の merge が要る（共通の規則）。
   約 1 % のためにこの review を回す利益は小さい。
3. コード由来の差（+1.9〜+2.9 %）は 10 % の閾値の 4 分の 1 以下。

fix の leaf への提案: コードを変えず、「health:S2 は規則の上では code だが、コード由来の差は `deps.violation` の +2 s（+2.3 %）で、10 % を超えた分は
測った時間帯の差」と本体の進捗に書く。回帰条件は「同時に並べた before と after の health の wall の差が before の +10 % を超えたら、rule ごとの計時
（`raw/health-rule-timing.py`）で rule を絞る」。

## 4. crate 分離

### 4.1 S1 の warm の test build の --timings（after、1 回）

command: S1 の 4 file に const の probe を入れて `cargo nextest run --workspace --no-run --offline --timings`（その前に計らない test build を 1 回）。
2026-11-19T08:57:40Z 開始、開始時の 1 分 load 3.61（120 s 待った）。

| 測定 | wall | user | build した unit | unit の時間の和 | 最後の unit の終わり |
|---|---|---|---|---|---|
| S1（4 file に probe。build-test の after の中央値は 138.10 s） | 136.57 s | 765.23 s | 340 | 1,304.65 s | 134.08 s |
| 理想の分離の上限（task-api の 3 file だけに probe。task-core を変えない） | 85.61 s | 421.46 s | 269 | 1,025.58 s | 83.11 s |
| 差 | **−50.96 s（−37.3 %）** | −343.77 s（−44.9 %） | −71 | −279.07 s（−21.4 %） | — |

S1 の crate ごとの unit の時間の和（秒、`raw/timings-S1.tsv`）: celeris-campaign 379.6・task-api 312.5・celeris 266.8・task-worker 90.7・**task-core 78.3**・
task-dispatch 58.7・celerisctl 31.1・task-ops 26.9・celeris-script 18.3・celeris-credentiald 10.9・e2e 10.2・celeris-mcp 8.5・routing-eval 6.8・llm-proxy 5.5。

- 初めの 25 s は task-core の lib の frontend（24.49 s）で、ほかの 11 crate は task-core の rmeta を待つ。最後は `task_dispatch` の lib の試験 binary
  （43.90 s、90.18 s 開始 → 134.08 s 終了）。task-dispatch は task-api に依存しないので、S1 で再 build されるのは task-core の変更による。
- task-core の変更だけで再 build される unit（理想の分離で消える分）は task-core・task-ops・task-dispatch・celeris-script・celeris-credentiald・e2e・
  celeris-mcp・routing-eval・llm-proxy と task-worker の lib（task-worker の試験は task-api の dev 依存で残る）。

### 4.2 test-affected

理想の分離（S1 の変更が task-api の 3 file だけ）の `test-affected.sh --list` は 7 crate（celeris・celeris-campaign・celerisctl・llm-proxy・task-api・
task-dispatch・task-worker）＋ web で、ts-verify は外れる（`raw/ta-list-S1api.txt`）。外れる 7 crate（task-core・task-ops・celeris-script・
celeris-credentiald・celeris-mcp・routing-eval・e2e）の試験の属性は 2,773 件で、crates/ の 8,865 件の 31.3 %（`raw/test-attrs-per-crate.txt`）。
nextest の crate ごとの時間は test-affected の log に出ない（遅い試験だけが出る）ので、時間は試験の数の比で見積もった（未測定）。

### 4.3 見積もりの式と値

- 理想の上限（S1 の task-core の変更をすべて task-core の外に出せたとき）:
  - build: (T_S1 − T_api) / T_S1 = (136.57 − 85.61) / 136.57 = **37.3 %**
  - test-affected: (ΔT_build + N_外れる / N_全体 × T_nextest) / T_TA = (50.96 + 0.313 × 505.87) / 906.03 = **23.1 %**
    （T_nextest は ta-s1・ta-mix の S1 の nextest の中央値 505.865 s、T_TA は after の中央値 906.03 s。ts-verify の分は数えていない）
  - S2 は task-core を変えない（再 build は task-dispatch とその逆依存の 4 ＋ 1 crate）ので 0 %。2 シナリオの平均は build 18.7 %、test-affected 11.6 %。
- 小範囲の分離で実現できる割合 f: S1 が変える `crates/task-core/src/store/org.rs` は `impl SqliteStore` の inherent impl（338 行）で、
  task-core の中の `store/org_directory.rs:49`（`org_upsert_tx`）と `store/decision_log.rs:847`（`campaign_org_check_tx`）からも呼ばれる。
  - inherent impl は `SqliteStore` を定義した crate の外に置けない。外に出すには trait か自由関数に変え、`SqliteStore` の内部を公開する必要がある。
  - task-core の中の呼び手が新しい crate に依存すると、新しい crate（task-core に依存）との間で循環になる。避けるには `decision_log` の campaign の
    検査も移す必要があり、ドメイン実装を大きく動かすことになる。
  - したがって小範囲の分離では S1 の変更は task-core に残り、**f = 0、見積もりは S1・S2 とも 0 %**。
- 判定: **pilot は推さない**。理想の上限でも 2 シナリオの平均は 20 % に届かず、小範囲の分離の見積もりは 0 %。trait や Port を足すだけの分離は成果に数えない（共通の規則）。
- 次の候補（実施しない、記録だけ）: S1 の 25 s の待ちは task-core の lib の frontend の長さによる。task-core の frontend の時間を測り、どの module が
  長いかを `-Z self-profile` などで調べる案がある。nightly が要るので、この task の範囲では行わない。

## 5. 再現コマンド（repo の root で。`ART` はこの run の成果物 dir）

```sh
. agent-docs/progress/2026-11-19-arch-v4-g/scenarios.env
B0=bc665168c9caf1898adc4c611404bbee056daf9a
S2B="crates/task-dispatch/src/run_settlement/work_unit.rs crates/task-dispatch/src/run_settlement.rs crates/task-dispatch/src/dispatcher/worker_finish.rs crates/task-dispatch/src/dispatcher/work_units.rs"
API3="crates/task-api/src/org_write.rs crates/task-api/src/handlers/org.rs crates/task-api/src/cos/ops/admin.rs"
waitload() { s=0; while awk '{exit !($1>4)}' /proc/loadavg && [ $s -lt 600 ]; do sleep 10; s=$((s+10)); done; }
# verdict（flags.tsv の control の組と control-*.tsv の rev=before・valid=yes の中央値）
python3 -I $ART/raw/cause.py agent-docs/progress/2026-11-19-arch-v4-g > agent-docs/progress/2026-11-19-arch-v4-g/cause.tsv
# rule ごとの計時（before と after を同時に）
git worktree add --detach "$TMPDIR/wt-before" $B0
waitload
(cd "$TMPDIR/wt-before" && bash $ART/raw/probe-const.sh $S2B && python3 $ART/raw/health-rule-timing.py "$PWD/scripts/dev/repo-health" $ART/raw/hr-before.tsv -- \
   --repo . --base $B0 --head worktree --policy-version 1 --out $ART/raw/hr-before.json; git checkout -- $S2B) &
(bash $ART/raw/probe-const.sh $S2_FILES && python3 $ART/raw/health-rule-timing.py "$PWD/scripts/dev/repo-health" $ART/raw/hr-after.tsv -- \
   --repo . --base HEAD --head worktree --policy-version 1 --out $ART/raw/hr-after.json); wait; git checkout -- $S2_FILES
# deps.violation の cProfile（上と同じ形で、python3 -m cProfile -o $ART/raw/prof-{before,after}.prof ... --rules deps.violation）、突き合わせは raw/prof-compare.txt の script
git worktree remove --force "$TMPDIR/wt-before"; git worktree prune
# --timings（S1 と理想の上限）
cargo nextest run --workspace --no-run --offline                     # 計らない build
waitload; bash $ART/raw/probe-const.sh $S1_FILES
/usr/bin/time -f '%e %U %S %M' cargo nextest run --workspace --no-run --offline --timings; git checkout -- $S1_FILES
cargo nextest run --workspace --no-run --offline; waitload; bash $ART/raw/probe-const.sh $API3
/usr/bin/time -f '%e %U %S %M' cargo nextest run --workspace --no-run --offline --timings; git checkout -- $API3
python3 -I $ART/raw/timings-parse.py "$CARGO_TARGET_DIR/cargo-timings/<cargo-timing-*.html>" > $ART/raw/timings-S1.tsv
# 理想の分離の test-affected の対象
bash $ART/raw/probe-comment.sh $API3; bash scripts/dev/test-affected.sh --list; git checkout -- $API3
git status --porcelain -- crates                                     # 空
```

生の記録（この run の成果物 dir の `raw/`）: `cause.py`・`cause-summary.txt`・`health-rule-timing.py`・`hr-{before,after}.{tsv,time,json,out}`・
`prof-{before,after}.prof`・`prof-compare.txt`・`timings-S1{,api}.{html,tsv,log,time}`・`timings-parse.py`・`prep-build{,2}.log`・`ta-list-S1api.txt`・
`test-attrs-per-crate.txt`・`metadata.json`・`ws-deps.txt`。

## 6. 未達・要判断

| 項目 | 状態 |
|---|---|
| health:S2 の verdict | code（規則どおり）。コード由来は +2 s（+2.3 %）で、fix の leaf にはコードを変えないことを推す。fix の leaf が決める |
| health:S1 のコード由来の差 | 未測定（S2 と同じ rule・同じ検査器なので +2 s と見込む） |
| nextest の crate ごとの時間 | 未測定（log に出ない）。crate 分離の見積もりは試験の数の比で代えた |
| control-build の before の 2 回目（107.83 s）の原因 | 未確認（control-build.md §6 のまま） |
