---
title: R5 再計測の require の item 指定と除外の理由
tasks: [01M6TYVJT53598CPKMGSQGA7XM]
status: done
updated: 2026-11-12
---
# R5 再計測の require の item 指定と除外の理由

完了日: 2026-11-12

## 実装前 brief

1. 根本原因: base（`git merge-base HEAD main`）の `scripts/dev/test-affected.sh` は doc test を `cargo test --doc -p <対象>` で走らせる。lib target の無い celerisctl だけの変更では `no library targets found` で exit 101 になる。`pair()` は両側の `doc_test_exit == 0` を求めるので、celerisctl の edit_verify の組は負荷と関係なく除外される。`--require-compare` は crate 単位で全 item を求めるので、`celerisctl` を挙げると必ず exit 1 になる。除外の理由も記録に残らず、表からは分からなかった（meas-edit-a の plan_issue。記録は branch `celeris-wu/01M6TYVJT53598CPKMGSQGA7XM/meas-edit-a` の `meas-edit-a.md`）。
2. owner: `measure_summary.py` の `pair()`・`require()`・`summary()` と `measure.sh` の使い方の説明（meas-tools の成果）。
3. 再利用: 既存の `pair()` の判定、`comparisons()`、`summary --check` の byte 一致、`measure.sh --require-compare` の経路。
4. 選択: 除外の規則はそのままにし、require の指定の粒度と理由の表示だけを足す。exit 101 を成功扱いにする案と、doc test を比較から外す案は、除外の規則を弱めるので採らない。
5. 新しい状態・分岐: 比較の行の `reason` 欄（採用の行は null）。require の `<crate>:<item>`。fallback・互換分岐は無し。
6. 撤去: 無し。表の列は置き換えた（旧い表の形は残さない）。
7. 不変条件: 除外の判定は前と同じ（比 2 以上・比が定まらない・負荷待ちが ready でない・exit が 0 でないか欠ける・秒が欠けるか無効）。`excluded` は理由があるときだけ真。fixture の試験と、旧い `pair()` との突き合わせで確かめた。

## 変更

- commit『phase 1: meas-items: --require-compare に crate:item の指定と表に除外の理由の列を足す』
- `pair()` は除外の理由を `reason` に返す。理由は `load ratio >= 2`・`load wait expired`・`<base|head> <exit の欄> <値>`・`missing or invalid measurement` をこの順に `; ` で並べる。片側の load が 0 で比が定まらない組は `load ratio >= 2` とする。exit の欄が無いか null のときは `missing or invalid measurement` とする。
- `require()`（`measure.sh --require-compare`）は `<crate>` と `<crate>:<item>` を受ける。item は build・edit_verify・run。`<crate>` は今までどおり記録の schema の全 item を求める。記録の schema に無い item・未知の item・空の crate は exit 2。
- `compare`・`run-compare` は crate 名だけを受ける。`:` を含む指定と空の crate は測る前に exit 2 にする。
- `summary` の表に `base nextest s`・`HEAD nextest s`・`reason` の列を足した。nextest の秒は edit_verify の記録に `edit_to_verify_secs`（doc test を除く edit→verify）があれば出し、無ければ `-`。`summary --check` の判定（BEGIN・END が 1 つずつで、生成した表と byte で一致）は変えていない。
- `measure.sh` の冒頭の使い方の説明と、引数の誤りのときの usage の行を直した。

meas-edit-a への影響: `--require-compare <out> task-api,celeris,celerisctl:build` で、celerisctl は build の組だけを求められる。celerisctl の edit_verify の組は除外されたまま表に残り、理由の列に `base doc_test_exit 101` と出る。

## 証拠

提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`。この leaf では cargo の計測を走らせていない。

- `python3 agent-docs/progress/2026-11-10-health-v2-r5/test_measure_summary.py` → exit 0、17 件合格（前の 13 件と、足した 4 件）。足した試験は次を確かめる。
  - base の `doc_test_exit` が 101 の celerisctl の記録で、`measure.sh --require-compare` の exit は `celerisctl:build` が 0、`celerisctl` と `celerisctl:edit_verify` が 1、`celerisctl:run`（build-compare の schema に無い item）・`celerisctl:unknown`・`:build`・`celerisctl:`・`celerisctl,` が 2。表の edit_verify の行に `5.18 | 29.55`（nextest の秒）と `excluded | base doc_test_exit 101` が出る。
  - 理由の 4 種と複数の理由の並び、採用の行の理由は null。
  - run-compare の記録では `a:run` が通り、`a:build` は ValueError（exit 2）。
  - compare に `crate:item` と空の crate を渡すと、git と cargo を呼ぶ前に ValueError。
- 旧い `pair()`（`$CELERIS_WU_BASE` の版）と新しい `pair()` に同じ乱数の記録 200,000 組を与え、`excluded` と `ratio` が全組で一致した（差 0）。`excluded` が真の組と、`reason` が null でない組も一致した。
- `bash agent-docs/progress/2026-11-10-health-v2-r5/measure.sh --validate agent-docs/progress/2026-11-10-health-v2-r5/after-build.json` → exit 0（`ok: compare 4 crates (both sides built: 4), 1 skipped item(s)`）。
- `bash -n agent-docs/progress/2026-11-10-health-v2-r5/measure.sh`、`git diff --check` → exit 0。

## 未解決

- この変更より前に書かれた /2 の記録（meas-edit-a の branch の `remeasure-edit-a.json` など）は `comparisons` に `reason` 欄が無いので、`--validate` と `--require-compare` が exit 1 になる。`comparisons` は記録から導く欄で、規則が変わったら書き直す設計である。meas-edit-a は計測をやり直すので、新しい記録には `reason` が入る。この branch には /2 の記録は無い。

## 提案

- compare の再試行は `require(d, [crate])`（crate の全 item）で残りの crate を決める。celerisctl の edit_verify は base の doc test で必ず除外されるので、`compare ... celerisctl` は再試行のたびに celerisctl を測り直す。meas-edit-a は、celerisctl を最後の attempt まで測り直すことを見込んで `MEASURE_DEADLINE` を決めるか、別の compare で celerisctl を測る。compare に item の指定を足すかは、この leaf の範囲の外とした。
