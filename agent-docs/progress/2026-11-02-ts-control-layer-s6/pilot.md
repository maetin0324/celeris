---
title: S6 pilot — 自己改善 pilot（TS 制御コードの方針改善 → 検証 → 候補 bundle → 段階昇格 → rollback）
tasks: [01M6FPPJN7A8H427MXFVYFY7H4]
status: done
completed: 2026-11-03
updated: 2026-11-03
---
# S6 pilot — 自己改善 pilot

unit pilot。[境界 ADR](../../adr/2026-11-02-ts-control-layer-boundary.md) の付記 S5-1〜S5-7・S5-10 に従った。
**これは方針改善の記録で、言語移行の測定（policy・cos・policy-ts・cos-ts の jsonl）とは別である。** 数値を S6 の edit→activate の p50・p95 に混ぜない。

## 結論
- `scripts/dev/ts-pilot.sh` を作り、1 件通した。記録は [pilot/record.jsonl](pilot/record.jsonl)（16 段）、改善の差分は [pilot/candidate.patch](pilot/candidate.patch)、ts-verify の段ごとの秒は [pilot/ts-verify.json](pilot/ts-verify.json)。
- 流れ: 使い捨ての作業ツリーで改善 → `ts-verify.sh`（10 段、cargo 0 回）→ 候補 bundle → 一時 DB の daemon に `script-deploy` で配備・generation 登録 → compare（人でない operator の token）→ canary（試験 daemon の人の token）→ 差分の記録 → 再起動して canary の復元を確認 → rust へ rollback → 再起動して rust の復元を確認。
- 全体の壁時計は 14.9 秒（提出時の HEAD で走らせた 2 回目）。内訳は ts-verify 7.8 秒、作業ツリーの用意と編集 4.4 秒、daemon の段（起動・登録・切替・再起動 2 回・rollback）合計 約 2.5 秒。
- 本番は使っていない。範囲: daemon は `$TMPDIR` の一時 DB・一時 state dir・一時 build cache・loopback の port で起こし、終わったら止めて消した。本番の mode・deploy・config・DB・`~/.local/celeris` には触れていない。IEEE Access・K2・K4 の環境にも触れていない。
- 評価器・hard safety gate は弱めていない。`ts-verify.sh` はそのまま走らせ、golden（Rust の参照実装の出力）との一致を通した。canary の前提の計数だけは一時 DB に積んだ（下の「canary の前提の計数」）。

## 改善の内容（領域 `cos_digest`）
- 変更: digest の見出しに出す run の失敗の理由（`run は失敗: <reason>`）を 200 文字（code point）までにし、超えたら「…」で切る。例外の stack など長い理由が見出しを埋めないようにするため。cards の理由は変えない。
- file: `control-ts/src/domains/cos_digest.ts`（`capRunReason` を足し、`digestMessage` の 1 行で使う）と `cos_digest.test.ts`（200 文字・201 文字・サロゲートペアの試験）。この branch の `control-ts/src` には commit していない（作業ツリーは `git worktree --detach HEAD` で作り、終わりに消した）。
- 差分の記録（段 `diff`）: 改善前（HEAD）と候補の `decide` を比べた。
  - fixture `cos_digest/cases.jsonl` の 2066 件: 差分 0 件（fixture の run の理由は最長 11 文字なので当たらない。ts-verify の golden と同じ結果）。
  - 改善の当たる入力（fixture の failed の入力 3 件の理由を 150・200・201・500・4000 文字に替えた 15 件）: 差分 9 件。201 文字以上の 9 件だけが違い、候補は見出しの理由が 200 文字と「…」になった。

## 段ごとの記録（record.jsonl の要約）
| seq | 段 | actor | 秒 | event（表の追記） |
|---|---|---|---|---|
| 1 | meta | worker:ts-pilot | 0.027 | — |
| 2 | edit | worker:ts-pilot | 4.380 | — |
| 3 | ts_verify | worker:ts-pilot | 7.796 | — |
| 4 | bundle | worker:ts-pilot | 0.108 | — |
| 5 | daemon_start | worker:ts-pilot | 0.442 | — |
| 6 | deploy | worker:ts-pilot | 0.048 | — |
| 7 | register | operator:pilot | 0.033 | `script_generation_loaded`（system:script-runtime） |
| 8 | compare | operator:pilot | 0.006 | `script_domain_mode_changed` rust→compare、`script_generation_activated`（どちらも operator:pilot） |
| 9 | canary_denied_non_human | operator:pilot | 0.006 | なし（403 `script_mode_requires_human`、表は不変） |
| 10 | canary_not_ready | human:pilot | 0.004 | なし（422 `script_canary_not_ready`、clean_streak 0/200） |
| 11 | guard_seed | worker:ts-pilot | 0.017 | なし（synthetic。一時 DB の `script_guard_state`） |
| 12 | canary | human:pilot | 0.006 | `script_domain_mode_changed` compare→canary、canary_bps 5000（human:pilot） |
| 13 | diff | worker:ts-pilot | 0.230 | — |
| 14 | restart_restore_canary | worker:ts-pilot | 0.304 | なし（起動時の復元は表を変えない） |
| 15 | rollback | operator:pilot | 1.015 | `script_domain_mode_changed` canary→rust、`script_generation_draining`（operator:pilot）、`script_generation_retired`（system:script-runtime、in-flight 0） |
| 16 | restart_restore_rust | worker:ts-pilot | 0.275 | なし |

- 時刻は各行の `started_at`・`ended_at`（UTC）にある。秒は 2 回目の実行の値。
- compare は人でない主体（CoS 相当）の token で行った。CoS の run credential は daemon の CoS run が発行するもので、CoS を無効にした試験 daemon には無い。そのため付記 S5-4 で同じ権限の段にある外部 operator の token（`operator:pilot`）を使った。
- canary は試験 daemon の人の token（`human:pilot`）で行った。**本番ではこの段が人の承認に当たる**（compare→canary・canary→ts は人の token だけ。付記 S5-4）。記録の `canary.detail.human_approval_in_production` にも書いた。
- 復元: 1 回目の再起動で mode canary・canary_bps 5000・generation active を新しい isolate で load し直した。rollback の後の 2 回目の再起動では mode rust のままで、retired の generation を load しなかった。

## canary の前提の計数（synthetic）
- 試験 daemon には CoS の実 pass が無いので、(f) の「compare→canary へ進む条件」（CoS の経路は 200 件以上で説明のつかない不一致 0 件）に届かない。段 10 で 422 になることを確かめた後、一時 DB の `script_guard_state` に成功の呼び出し 200 件の計数を書いた。gen-e2e（`crates/celeris/tests/script_generation_e2e.rs` の `observe_until_ready`）と同じ扱いである。
- 判定のコード（`script_guard::readiness`）と数値は変えていない。本番では compare の実 pass 200 件が要り、計数を書き換えない。記録の `guard_seed.detail.synthetic` は true で、`--validate` はこの印が無ければ落ちる。

## 分かったこと（方針改善を TS で出すときの制約）
- 候補は Rust の参照実装と、201 文字以上の理由の入力で違う。compare・canary の間にその入力が来ると、script-guard は説明のつかない不一致として数える（S5 は正規化の規則を持たない）。canary なら rust へ戻し、compare なら clean_streak を 0 にする。
- そのため、二重実装の間に TS だけで方針を変えると、その入力が来た時点で自動で rust に戻る。方針改善を出すには、Rust の参照実装にも同じ変更を入れて golden を作り直すか、ADR に正規化の規則を足す必要がある。pilot は fixture に当たらない改善を選んだので ts-verify は通ったが、実運用の入力では不一致になりうる。

## 証拠（提出時の HEAD で実行）
- `bash scripts/dev/ts-pilot.sh --out agent-docs/progress/2026-11-02-ts-control-layer-s6/pilot`: exit 0、16 段、「記録 OK（16 段）」。
- `bash scripts/dev/ts-pilot.sh --validate agent-docs/progress/2026-11-02-ts-control-layer-s6/pilot/record.jsonl`: exit 0。
- `bash scripts/dev/tests/ts-pilot-validate.sh`: exit 0。記録済みの record は通り、段の欠け・synthetic の印の欠け・人でない主体の canary・fixture の差分・拒否の code の食い違い・復元の失敗・JSON の壊れ・file の欠けの 8 件は exit 1。
- `git diff --name-only $CELERIS_WU_BASE -- control-ts/`: 出力なし（control-ts に差分なし）。
- test-affected・clippy・fmt の結果は WU の報告に書く。

## LLM 利用量
- `ts-pilot.sh` は LLM を呼ばない（llm source は `fake`、CoS は無効）。
- pilot を作って走らせた worker run（Claude Code）の LLM 利用量は、run の中から記録を読めないので**未確認**。Celeris の run の記録（usage）から取る。

## 未解決
1. canary の前提の計数は synthetic。試験 daemon で実の CoS pass を 200 件流す方法（偽の CoS の run を 200 回起こす）は、1 leaf の時間に収まるか未確認。
2. canary の間の TS 側の実の判定（canary の対象になった pass）は、この pilot では起きていない（試験 daemon に CoS の run が無い）。canary の選択と適用は gen-e2e と canary-dispatch の試験が確かめている。

## 提案
- 方針改善を TS で出す手順を ADR に足す: (1) Rust の参照実装にも同じ変更を入れ golden を作り直す、または (2) 領域ごとの正規化の規則（「この差は改善として説明がつく」）を ADR に書いてから compare を始める。どちらも無いまま canary に上げると、当たる入力で自動の戻しになる。

## 本番への移行
- なし。この unit は本番の mode・deploy・config を変えていない。本番で同じ流れを行うときの canary・ts への昇格は、人が人の token で API を呼ぶ手順になる（s5-close の「本番への移行」と同じ）。
