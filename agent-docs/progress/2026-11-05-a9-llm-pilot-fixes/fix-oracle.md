---
title: 課題と隠した受け入れを公開の指示だけで通る形に直す（1）（fix-oracle）
tasks: [01M6MJX174JF8F53GWFAEW2BEP]
status: done
completed: 2026-11-05
updated: 2026-11-05
---
# 課題と隠した受け入れを公開の指示だけで通る形に直す（1）

完了日: 2026-11-05

## 結論
llm_pilot の C06・C08 では、公開の acceptance に従えば hidden の期待に届く。acceptance は成果の作り方を 1 つのコマンド（`python3 minichunk.py verify <case> <id>`）で書き、そのコマンドの実装は公開の repo-seed にある。
コマンドは chunk を書いて fsync し、別の process で読み戻して照合する。一致すれば `out/<id>.txt` に検証の 1 行を書く。その内容の sha256 は、元の `hidden/expected.json` とすべて一致する。そのため expected.json は変えていない。
oracle の文字列も hash も公開側に写していない（試験で確かめる）。

## 変えたもの
- `tests/fixtures/long-horizon/public/{C06,C08}/repo-seed/minichunk.py`: `verify <case> <outcome>` と `read <path>` を足した。従来の引数 2 個の呼び方は残る。
- `public/{C06,C08}/repo-seed/README.md`: 手順と、hidden が何を確かめるかを書いた。
- `public/{C06,C08}/case.json` と `oracle/{C06,C08}/scenario.json` の acceptance: 手順を書いた同じ文に変えた。接頭語（`kestrane-o1:` など）は残した。`content` は残した（scripted 試験の台本の値で、公開の手順の出力と同じ）。
- `oracle/MANIFEST.sha256`: 変えた 8 file の hash を更新した。
- `scripts/dev/long-horizon-pilot/stub_harness.py`: acceptance に公開の手順があれば、作業 tree の直下でその手順を実行する。手順の無い Outcome だけ台本の答えを書く。
- `scripts/dev/long-horizon-pilot/driver.py`: 公開の手順がある Outcome の content は stub に渡さない。
- `scripts/dev/test-long-horizon-pilot.sh`: `sh`（dash）で呼ばれたら bash で再実行する。以前は `sh` で呼ぶと `BASH_SOURCE` で落ちていた。
- 新しい試験 `crates/campaign/tests/long_horizon_public_contract.rs`（3 件）:
  - 公開の repo-seed だけを写し、acceptance のコマンドを実行すると、採点と同じ `outcome_reached` で到達になる。
  - 手で書いた報告や別の case 名で実行した出力は、到達にならない。
  - acceptance は public と scenario で同じ。content は公開の手順の出力と同じ。公開側は content も hash も文字どおりには持たない。

## commit（件名）
- fix-oracle: C06・C08 の hidden を公開の手順から導ける形にする

基準は `git merge-base HEAD main`。

## 検査（提出時の HEAD で実行）
- `bash scripts/dev/test-affected.sh` → exit 0（nextest 3352 passed、9 skipped）
- `sh scripts/dev/test-long-horizon-pilot.sh` → exit 0（dispatch 7 passed、api 3 passed、stub daemon raw ok。C08 の new 腕は outcome_reached(o1)〜(o4) がすべて pass）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0

## 未解決事項
- ADR 2026-11-05-a9-llm-pilot-findings D2 の決定 2（hidden を検査 script の合否にし、sha256 照合を消す）は、この task ではしていない。公開の手順の出力が決まった値になるので、hash 照合のままでも公開の指示だけで通る。ただし、文面を変えた妥当な出力は落ちる。script 化をするかどうかは、続きの task（2）か ADR 付記で決める。
- bench_version は 1 のまま。hidden の期待値（expected.json）は変わっていないが、public の acceptance は変わった。D2 の決定 5 は版を上げることを求めている。上げると、commit 済みの a8b replay の raw（版 1）による採点試験が落ちる。版を上げるのは raw の作り直しと一緒に行う。
- C04・C09・C10 の公開側は、まだ手順を持たない（stub は従来どおり台本の答えを書く）。
- 実 LLM は起こしていない。実 LLM が手順に従うかどうかは未確認。

## 提案
- 次の実 LLM pilot の前に、C06 の effect の配線（D3）と、到達の判定（D1）を入れる。
