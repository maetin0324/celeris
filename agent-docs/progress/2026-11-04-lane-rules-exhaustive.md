---
title: lane_rules の網羅の操作と昇格の前提 100 件化（task 全体の完了状況と本番への移行）
tasks: [01M6J81DZ7C8SMY7HNZTW8GT0Z]
status: running
updated: 2026-11-04
---
# lane_rules の網羅の操作と昇格の前提 100 件化

task 01M6J81DZ7C8SMY7HNZTW8GT0Z。設計は [ADR 2026-11-04-lane-rules-exhaustive-evidence](../adr/2026-11-04-lane-rules-exhaustive-evidence.md)、
元の基準 (f) の付記は [境界 ADR](../adr/2026-11-02-ts-control-layer-boundary.md) の「付記 2026-11-04」。
WorkUnit ごとの記録は `2026-11-04-lane-rules-exhaustive/` の下（adr・evidence・store・runner・api・report）。
完了日: 未（`integrate-docs` の統合と review の後）。

## 完了状況（WorkUnit `report` の時点）
- adr・evidence・store・runner・api: 完了（各 WU の進捗ファイルを参照）。
- report: `scripts/dev/script-compare.py report` を新しい readiness に合わせた。
  - `lane_rules` の網羅の証拠（表 `script_exhaustive_runs`）を別の節 `exhaustive` に出す。table 形式では `exhaustive standing` の行と
    `exhaustive_run` の行（id・generation・state・stop_code・calls・matches・mismatches・timeouts・failures・safety・uncovered・
    p50/p95/max の latency・started_at・finished_at・daemon instance・requested_by・参照が今の binary と同じか）を出す。
  - 立場は `task_core::exhaustive_standing` と同じ規則（今の参照の hash の行だけ。不一致・安全違反があれば failed。無ければ終わった行の最新で決める。
    aborted・running は数えない）。合格（39,366 呼び出し・不一致 0・安全違反 0・未網羅 0・timeout と失敗が 0.1% 以下）なら `required` は 100 で、
    窓の timeout・失敗が 0 件でなければ ready にしない。無い・不合格・参照が違うときは 1000 件のまま。`route_priority`・`cos_digest` は変えない。
  - 参照の hash は `--lane-reference-sha256`。省略時は repo の `control-ts/fixtures/lane_rules/all-features.jsonl` の sha256。どちらも得られなければ
    `exhaustive_reference_unverified` の blocker で ready にしない。
  - 網羅の件数は live の件数（match など）に足さない。表が無い DB（0109 より前）でも読め、証拠は missing になる。
  - 試験は `scripts/dev/tests/script-compare.sh` に足した（合格＋100 件で ready、99 件・証拠無し・不合格・aborted・timeout 40 件・参照違い・窓の timeout 1 件は
    ready でない、表の表示、live の件数に入らない、DB の byte が変わらない）。
- 残り: 工程 `integrate-docs` の統合。

## 証拠（提出時の HEAD で実行。基準は $CELERIS_WU_BASE）
- `bash scripts/dev/tests/script-compare.sh` → exit 0（`script-compare: ok`）。
- 他の検査（`bash scripts/dev/test-affected.sh`、`cargo clippy --workspace -- -D warnings`、`cargo fmt --all -- --check`）は WU `report` の記録（`2026-11-04-lane-rules-exhaustive/report.md`）に書く。

## 本番への移行
**worker は本番の daemon・DB・config・release に触れていない**（この task の作業は repo の作業ツリーと一時 DB だけ）。
次はすべて operator（または人）が実行する手順で、release・verify・promote も operator が行う。状態を変える手順は下の「戻し方」とセットで行う。

### 0. 前提
- 本番は `lane_rules`・`cos_digest` が compare（2026-11-04T02:41:21Z から。release 9bccc68d0fda、bundle `c7c3cce5f82c820d19ecaaa97d08b3d5b573db748a4e3b143c136aaef44f9083`、
  generation 01M6J6X3HE4Z3MXM11GHJ1B6M4）。この task の変更（migration 0109・runner・API・readiness）は新しい release に入れて promote するまで本番に無い。
- 本番の config の書き換えは要らない（新しい欄は無い）。migration 0109 は daemon の起動時に自動で適用される（追記のみ。既存の表・events は変えない）。

### 1. release → verify → promote（operator）
1. main に入った commit から `bash scripts/selfdeploy/release.sh <git-ref>`（release gate。全試験と bundle gate を含む）。
2. `bash scripts/selfdeploy/verify.sh <sha12>`。
3. `SD_ACTOR=… SD_REASON=… SD_POLICY_REF=… bash scripts/selfdeploy/promote.sh <sha12>`（必須環境変数は `promote.sh` の先頭の説明）。
4. promote の後、`celerisctl script domains` で `lane_rules` が compare のままで generation・version が変わっていないことを確かめる。
   release を替えても bundle が同じなら generation は同じ。bundle が変わっていれば手順 6 の「generation が変わったとき」へ。

### 2. 網羅の起こし方（人または external operator の API token。CoS からは起こせない）
```
celerisctl script exhaustive run lane_rules \
  --generation c7c3cce5f82c820d19ecaaa97d08b3d5b573db748a4e3b143c136aaef44f9083 \
  --run-reason "lane_rules 全入力の網羅（ADR 2026-11-04-lane-rules-exhaustive-evidence）"
```
- `--generation` は `celerisctl script domains` に出る、今 compare（または canary）にある generation の `bundle_sha256`。違うと 409 `script_exhaustive_mode_conflict`。
- 応答は 202。実行は daemon の裏で続く（tick を止めない。1 件ごとに 5 ms 空け、live の呼び出しが入っていれば待つ。30 分で打ち切る）。
  走っている間の再実行は 409 `script_exhaustive_running`、全体停止の間は 409 `script_guard_halted`。
- 所要時間は未確認。本番での 39,366 呼び出しの時間は証拠の行の `started_at`・`finished_at` と latency で分かる。

### 3. 確認
1. `celerisctl script exhaustive status lane_rules`（`--json` で全欄）。`standing` が `passed`、最新の行が `state=passed`・`calls=39366`・`mismatches=0`・`safety_violations=0`・`uncovered_inputs=0`・
   timeout と失敗が 39 件以下（0.1%）であること。`running` の間は待つ。
2. 本番 DB の読み取り専用の集計（DB はコピーでなく本番の file を `-readonly` で開く script。何も書かない）:
   ```
   python3 -I scripts/dev/script-compare.py report --db <本番の celeris.sqlite3> --format table \
     --lane-reference-sha256 <release の binary の参照 hash> --require-ready lane_rules
   ```
   - 参照 hash は release の作業ツリーの `control-ts/fixtures/lane_rules/all-features.jsonl` の sha256（省略時はこの script が置かれた repo の同じ file を使う。
     release と repo の版が違うなら必ず渡す）。`exhaustive standing lane_rules <generation> = passed (required 100)` になり、`exhaustive_run` の行の最後が `True`（参照が一致）であること。
   - exit 4 は live の連続が 100 件に届いていない（約 48 件/日なら約 2 日）。`not ready:` の行を読む。exit 1 は problem あり（記録と計数の不一致など）で、昇格に進まない。
3. live の件数が網羅で増えていないこと: `script_guard_state` の `calls`・`clean_streak` と report の `match` が、網羅の前後で（live の呼び出しの分を除き）同じ。
4. tick が止まっていないこと: 網羅の間の `GET /api/v1/daemon` の `snapshot.script_tick_wait` と daemon の log の `slow tick`・`script_tick_wait` を `script-compare.py sample`・`--tick-log` で見る。
5. 昇格（compare→canary→ts）は今までどおり人の token だけ。`--require-ready lane_rules` が exit 0 になってから `celerisctl script mode lane_rules canary …` を人が決める。
   証拠が無い・不合格のときの昇格は 422 `script_canary_not_ready`（`detail` に `required`・`exhaustive`）。

### 4. 不合格のとき・戻し方
- `state=failed` の行の `stop_code` と `first_problem_json`（最初の不一致・安全違反の入力・Rust の出力・TS の出力）を読む。
  - `mismatch`（不一致）: mode が canary・ts なら daemon が自動で `rust` へ戻し（actor `system:script-guard`）、受信箱 `script_control` に項目が出る。compare なら mode は変わらない。
    証拠は failed のまま消えず、その generation の `required` は 1000 件に戻る。原因は bundle か Rust の規則表の違いなので、直した新しい bundle を新しい generation として載せる（手順 6）。
  - `safety_violation`: generation を `invalid` にし、それを指す全領域を `rust` へ戻す。受信箱を人が見て原因を直す。
  - `consecutive_errors`・`rate_exceeded`（timeout・失敗の多発）: mode は変わらない。本番の isolate の負荷・deadline 50 ms を調べ、直してからもう一度流す（この 2 つの失敗は最新の行で決まるので、次に合格すれば passed に戻る。不一致・安全違反は戻らない）。
- `state=aborted`（`generation_gone`・`wall_limit`・`daemon_stopped`・`daemon_restarted`）は証拠に数えない。daemon が再起動された場合は流し直す。
- 手動で戻すとき: `celerisctl script mode lane_rules compare --expected-version <n> --generation <sha256>`（rust への戻しは `--generation` 不要で、誰でも可）。
  戻しは events・証拠の行を消さない（追記のみ）。
- release ごと戻すとき: `bash scripts/selfdeploy/rollback.sh`（`--restore-db` は使わない。0109 の表は旧 binary が読まないだけで害は無い）。旧 binary の readiness は 1000 件に戻る。

### 5. 100 件の要件のまとめ
- 網羅が `passed`、かつ live の連続が 100 件（compare は `clean_streak`、canary は `ts_side_clean_streak`）、窓の timeout・失敗が 0 件、安全違反 0 件で、`compare→canary` と `canary→ts` の gate が通る。
- 網羅が無い・failed・参照違いは 1000 件のまま。`route_priority`（1000）・`cos_digest`（200）は変わらない。

### 6. generation が変わったとき・規則表が変わったとき
- bundle を替える（`celerisctl script generations register …` で新しい generation を載せる）と、証拠の key が generation なので新しい generation の立場は `missing` になる。
  新しい generation が compare（または canary）にある状態で手順 2 の網羅を流し直し、手順 3 で確かめる。生の記録の 100 件も新しい generation で数え直す。
- Rust の規則表（`lane_fixture_jsonl`）が変わる release でも参照の hash が変わるので `missing` になる。流し直す。
- 旧 generation の証拠は消えない（追記のみ）。別 generation の証拠は数えない。

## 未解決
- 本番の網羅の所要時間と latency は未確認（上の手順 2・3 で operator が記録する）。
- `script-compare.py preflight` の最小 schema は 107 のまま（0109 を要求していない）。release が 0109 を含むことは手順 1 の gate が確かめる。
