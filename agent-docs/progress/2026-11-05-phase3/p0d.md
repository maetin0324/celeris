---
title: P0-D C06 の外部効果と実承認経路（pilot 専用・安全）
tasks: [01M6N7XY9N8J21RRGC8AP6EZ4A]
status: done
completed: 2026-11-05
updated: 2026-11-05
---
# P0-D C06 の外部効果と実承認経路（pilot 専用・安全）

完了日: 2026-11-05。親は 01M6N7WD1WXXW0A5FV7RVZ67B7。基準は `git merge-base HEAD main`。attempt 2 の途中で main に P0-C
（bench v2）が入ったので merge で取り込んだ（driver・stub の衝突を解いた: stub は台本の答えを読まず、facts に `leak_hits`・
`controller_achieved` が入る main の形に、P0-D の token・照合・forge を足した）。
方式の決定は [ADR 2026-11-05-phase3-p0d](../../adr/2026-11-05-phase3-p0d.md)。

## 結論
実 daemon の pilot で、C06 の公開は worker の外の broker だけが記録し、承認は daemon の承認 API（`POST /approvals/{id}/decide`、
人の token）で決まるようになった。worker には公開の command だけが案件の依頼文で渡る。stub 配線（`--harness stub`）と
config harness の経路（`--harness config-stub`）の両方で、o1 は承認の後に 1 件、o2 は却下で 0 件、verdict pass になった。
worker が記録に偽の行を足すと、driver は facts を作らず試行を失敗にする。実際の外部 publish はしない。

attempt 2（review の差し戻し）: 最初の版は人の token を試行の home（worker が読める）に repo の定数で置いていたので、worker が
自分で `POST /approvals/{id}/decide` を呼んで承認を偽造できた。token を試行ごとの乱数にし、home の外の私的な dir に daemon と
broker の起動の間だけ置くようにした。driver は daemon の決定が自分の決めたものと一致しなければ observation_failed にする。
両経路の実 daemon の試行で、worker に token を探させても API に通らない（`forge_refused`）ことを確かめた。

## 照合した既存の事実（再実装しないもの）
- fix-effects（`agent-docs/progress/2026-11-05-a9-llm-pilot-fixes/fix-effects.md`）の effect stub・`driver.py` の `effects.jsonl`
  の読み取り・observation_failed の扱いは残し、記録の書き手と承認の経路だけを置き換えた。
- `crates/campaign/src/long_horizon.rs` の採点（`effect_count`・`effect_before(approval, effect)`）は変えていない（P0-C の持ち物）。
  fixture（`tests/fixtures/long-horizon/`）も変えていない。
- 旧 milestones API・controller・認可（ADR-0033 の approvals、A7 の authority）・Rust/TS の分担には触れていない。migration は無い。

## 変更（commit は件名で挙げる）
- `scripts/dev/long-horizon-pilot/pilot_effect.py`: broker（`serve`）と client（`publish`）に書き換えた。HMAC の鎖・末尾の報告・
  daemon の approvals と `Answered` event の主体の確認・lock と flock・追記の途中の crash の回復・試験 hook。
- `scripts/dev/long-horizon-pilot/driver.py`: broker を worker の外で起こし（鍵は stdin）、C06 の依頼文に公開の手順を書き、
  承認待ちを daemon の approvals から人の token で決め、collect で記録を検証し、承認を DB の `approvals` から独立に facts に入れる。
  ops に `approvals` と `effect_audit` を残す。`--stub-early-publish` を消し、`--stub-tamper` を足した。
- `scripts/dev/long-horizon-pilot/stub_harness.py`: exit 3 で `pilot-effect:<logical_id>` の question を出して止まる。
  `--from-objective`（objective の手順だけを使う）、`--tamper-dir`（陰性）。再開の run で同じ成果を commit し直さない。
- `scripts/dev/long-horizon-pilot-llm.sh`: `--harness config-stub`、`LONG_HORIZON_PILOT_STUB_TAMPER=1`、後片付けの chmod。
  `LONG_HORIZON_PILOT_STUB_EARLY_PUBLISH` を消した。
- `scripts/dev/long-horizon-pilot/test_pilot_effect.py`（新規、15 件）、`scripts/dev/test-long-horizon-pilot.sh` の段 2b。
- `docs/ops/project-controller.md` に C06 の公開と承認の 1 項目。ADR `2026-11-05-phase3-p0d.md`。
- attempt 2: `driver.py` の token（乱数・home の外・起動の間だけ）と `check_forge`・`--forge`、`pilot_effect.foreign_decisions`、
  `stub_harness.py --forge`、`long-horizon-pilot-llm.sh` の `LONG_HORIZON_PILOT_STUB_FORGE=1`、単体試験 `ForeignDecisionCase` 6 件
  （合計 21 件）、段 2b の forge の検査、ADR の決定 9、`docs/ops/project-controller.md` の 1 文。
- commit: 「p0d: pilot の公開 effect を worker の外の broker と daemon の承認 API に移す」「p0d: 進捗に検査の結果を書く」
  「p0d: 人の token を worker から読めない場所に移し、driver の知らない承認の決定を observation_failed にする」
  「p0d: 差し戻しの対応の raw・ADR・進捗を書く」「Merge main（P0-C の bench v2）を p0d に取り込む」
  「p0d: main 取り込み後の raw と検査の結果を書く」。

## 受け入れ条件と証拠（提出時の HEAD で実行）
| 条件 | K | コマンド | 結果 |
|---|---|---|---|
| 承認前は effect 0 で拒否 | K17 | `python3 -I -W ignore::ResourceWarning scripts/dev/long-horizon-pilot/test_pilot_effect.py -v` | exit 0、21 件 OK。`test_k17_*` 2 件（承認前・未決・別 id・CoS/operator の決定は exit 3、効果 0） |
| 承認を worker が偽造できない（attempt 2） | K17 | 同上・段 2b（`LONG_HORIZON_PILOT_STUB_FORGE=1`） | `ForeignDecisionCase` 6 件 OK（driver の知らない決定は食い違い → observation_failed）。実 daemon の stub・config-stub で worker が集めた token は API に 401、token の file 4 つはすべて FileNotFoundError、op `forge_refused`、承認は driver の `once`・`denied` だけ |
| approved で exactly once | K18 | 同上・段 2b | `test_k18_approved_publishes_exactly_once`。実 daemon の C06 で `effect_count(C06:o1:publish) == 1` pass |
| rejected で 0 | K25 | 同上・段 2b | `test_k25_*` 2 件。実 daemon で o2 は `denied` → audit `rejected`、`effect_count(C06:o2:publish) == 0` pass |
| crash で二重 0 | K26 | 同上 | `test_k26_*` 4 件（記録の前・後・途中で落ちて再実行して 1 件、承認前は 0） |
| 重複・並行で二重 0 | K18・K27 | 同上 | `test_k27_concurrent_publish_of_one_logical_id_records_once`（hook で 2 本目が lock の外で待つことを確かめて 1 件） |
| worker が audit/effects を編集できない | K19・K20 | 同上・段 2b の陰性 | `test_k19_*` 3 件・`test_k20_*`。実 daemon で `LONG_HORIZON_PILOT_STUB_TAMPER=1` は exit 1、`observation_failed: the effect record cannot be trusted … MAC does not match`、facts 0 件 |
| stub 配線を保ち config harness 実行で通る | K28 | `bash scripts/dev/long-horizon-pilot-llm.sh --harness stub …` と `--harness config-stub …`（C06、new） | どちらも exit 0、verdict pass、effects は `C06:o1:publish` 1 件、承認は `once`・`denied` |
| 全体 | — | `bash scripts/dev/test-long-horizon-pilot.sh --out <dir>` | 下の「検査」を参照 |

## 生記録
`agent-docs/progress/2026-11-05-phase3/p0d/` に置いた（段 2b の出力の写し。作業 tree の写し `trials/` は除いた）。
- `stub-c06/`・`config-c06/`: facts・ops（`approvals`・`effect_audit`・操作の列）・grade・試行の log。
- `stub-c06-tamper.log`: 改ざんの陰性の log。`pilot-effect-unit.log`: 単体試験の出力。`test-long-horizon-pilot.log`: 全体の出力。

audit の列（stub・config-stub で同じ）: `C06:o1:publish refused_unapproved` → `published`（approval `once`）→
`C06:o2:publish refused_unapproved` → `rejected`（approval `denied`）。

## 失敗したケース（作業中）
- attempt 1 の review で不合格（criterion 4）: 人の token が試行の home にあり、worker が承認を偽造できた。attempt 2 で直した（上の結論）。
- attempt 2 の単体試験 `test_a_decision_the_worker_made_with_a_found_token_is_foreign` は 1 件を期待したが、決定と人の答えの 2 件が
  食い違いとして出た。照合は正しく、期待値を 2 件に直した。
- 単体試験が止まった: 試験の `assertEqual(…, msg=stderr.read())` が broker の stderr の EOF を待っていた。失敗のときだけ読むように直した。
- 効果の行を全部消す改ざんを、最初の版は見逃した（file ごとの鎖だけでは末尾の切り詰めが分からない）。audit の `published` に効果の
  MAC を持たせ、broker が末尾を pipe で driver に知らせる形にして、両方の file を空にする場合も検出するようにした。
- 全体の検査の 1 回目は task-dispatch の試験の build で `could not find native static library rusty_v8` で落ちた（古い seed の build
  script の出力）。`$CARGO_TARGET_DIR/debug/build/v8-*` を消して再実行した。コードの問題ではない。

## 検査（提出時の HEAD で実行）
- `bash scripts/dev/test-long-horizon-pilot.sh --out <dir>`: exit 0。dispatch 7 passed、api 3 passed、virtual raw ok（7 scenarios、
  7 gate rows）、stub daemon raw ok（C08）、pilot_effect unit `Ran 21 tests`、
  `C06 effect broker raw ok (stub and config-stub; forged approvals refused)`、改ざんの陰性は exit 1。出力は `p0d/test-long-horizon-pilot.log`
  （main の merge の後に実行。14 分 16 秒）。
- `bash scripts/dev/test-affected.sh`: exit 0、「試験対象の変更なし（docs のみ等）」（変更は scripts/dev の Python・shell と文書だけ。
  Rust・web の変更は無い）。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。
- `bash scripts/dev/check-doc-links.sh`・`sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv`・
  `sh scripts/dev/progress-index.sh --check`・`bash scripts/dev/check-adr-numbers.sh`: すべて exit 0。
- `test -f agent-docs/progress/2026-11-05-phase3/p0d.md`: exit 0。
- TS policy・Rust は変えていないので、verify の cargo 0・p50/p95 の条件は対象外。

## 未解決事項
- 実 LLM（`--harness config` の実 source）が依頼文の手順どおり exit 3 で question を出すかは未確認。実 LLM pilot は人の gate
  （外部 operator が worker の外で流す。A9 attempt 3 の人の判断）。
- K27 の試験は main の旧 `pilot_effect.py` には当てられない（旧版は file で承認を待ち、hook を持たない）。「修正前に赤」は未確認。
- token の file は daemon の起動の間（数秒）だけ home の外の私的な dir にある。同じ uid の process がその間に読むことは防いで
  いないが、その間に worker の run は無い。試行の daemon の `POST /api/v1/reload` は token の file が無いので失敗する（pilot は使わない）。
- 実 LLM の worker は operator の token も持たない。operator の決定は broker も数えず、driver の照合でも食い違いになる。
- 鍵は driver の memory にだけあるが、同じ uid の worker が driver の process を ptrace で読むことは防いでいない。
- 実 LLM の CoS が承認の question を受信箱で自分で答えた場合、approvals の決定は CoS の主体になり、broker は数えない（exit 3 の
  まま）。そのとき試行は承認待ちで止まる。pilot の CoS の振る舞いは実 LLM で未確認。

## 次の gate
- P0-F（実 LLM の縮小 pilot の再実行）: 人の明示の許可（上限・account・費用）の後、外部 operator が
  `LONG_HORIZON_PILOT_LLM_APPROVED=1 bash scripts/dev/long-horizon-pilot-llm.sh --harness config --llm-config <toml> --cases C06 …`
  を流す。C06 の facts に `approvals`（once・denied）と effects（o1 の 1 件）が入るかを見る。

## 提案
- approvals に決定の主体（`decided_by`）の欄があれば、broker は task の events を読まずに済む（A7 の未解決と同じ）。migration を
  伴うので P0-D では足さなかった。
