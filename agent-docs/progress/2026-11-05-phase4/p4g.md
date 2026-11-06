---
title: P4-G 本番 1 案件限定 shadow 導入の準備と go/no-go
tasks: [01M6PAMC44JQ6DB90E9HQ076DW]
status: done
updated: 2026-11-06
completed: 2026-11-06
---
# P4-G 本番 1 案件限定 shadow 導入の準備と go/no-go

完了日: 2026-11-06（準備と非本番の予行の範囲）。第 4 フェーズ親 01M6PAK8E8RH3ZMQC0FAD8ZNAA の子（人の依頼 seq 2343）。基準は `git merge-base HEAD main`。ADR [2026-11-06-phase4-p4g-shadow-rollout](../../adr/2026-11-06-phase4-p4g-shadow-rollout.md)。手順書は `docs/ops/project-controller.md` の「4a. 1 案件の限定導入の道具（P4-G）」。

## 結論
- **本番の shadow は未実施。判定は `no-go (pending)`。** 理由: (1) 人の観察の承認（`kind = "observe"`）が無い。(2) G7 の実 LLM の縮小 pilot の再実行（`--cos-route` 付き）が未実施で、案件 CoS の消費が予算に入ることは fake harness の試験でしか確かめていない。(3) 本番の G2〜G5（config・他の案件・領域・dry_run の判断）は、人が本番の API と config に preflight を当てるまで未確認。
- 外部 operator の条件（01M6PYNMP4 の修正が main に入り、案件 CoS の消費が予算に入ること）: 修正の commit は main にある（G1 pass、下の「go/no-go の現在」）。消費の算入の実 LLM での確認は G7 で pending。
- 道具を足した: `scripts/dev/pc-rollout/pc_rollout.py`（preflight・snapshot・diff・monitor・apply・undo・e2e）、`scripts/dev/pc-rollout.sh`（非本番の予行）、単体試験 16 件。test-affected は `scripts/dev/pc-rollout/` の変更で単体試験を走らせる。
- 承認を分けた: 観察（dry_run・shadow）の承認と効果（canary・enabled）の承認は別の file。効果の承認は本番の 24 時間以上・違反 0 の shadow の監視の報告の sha256 を引く。観察の承認で canary に上げる操作は拒まれることを予行で確かめた。Rust の権限（mode を上げるのは人の token だけ、canary・ts は人だけ）は変えていない。
- 非本番の予行（一時 daemon・fake harness・127.0.0.1）で、本番と同じ API の順の操作が全部期待どおりに動いた（予行の期待 20 件が全部 ok）。

## main と既存実装の照合
- 読んだもの: `docs/ops/project-controller.md`（P2-J の checklist・rollback・P4-D の GC）、`agent-docs/progress/2026-11-05-phase4/cos-usage.md`、`p4f.md`（daemon-soak の一時 daemon の起こし方）、ADR P4-A の G1〜G7 の表、`crates/task-api/src/handlers/project_controller.rs`（mode の変更の規則）、`crates/task-api/src/script_domains.rs`、`crates/task-dispatch/src/dispatcher/project_wake.rs`・`crates/task-core/src/store/project_wake.rs`（due・lease・coalesce）、`crates/celeris/src/daemon/script_layer.rs`（起動時の retire）。
- 再利用したもの: daemon-soak の config の形と fake harness（`scripts/dev/daemon-soak/soak_harness.py`）、control-ts の bundle の作り方。既存の API だけを使い、DB への直接の書き込みはしない（daemon-soak の段の昇格の行の直接の書き込みとは違い、予行の mode の変更は全部 API）。
- 重複を避けた: 本番の RSS の長時間の比較は P4-F の `daemon-soak` の範囲で、ここでは標本に RSS を記録するだけにした。

## 変更（commit 件名）
- `p4g: 本番 1 案件の限定導入の go/no-go・承認の分離・監視・差分・戻しの道具と非本番の予行の raw、ADR・手順書・進捗`

| 場所 | 内容 |
|---|---|
| `scripts/dev/pc-rollout/pc_rollout.py` | 道具本体。go/no-go の条件 G1〜G7、承認の検査、差分、監視、apply・undo、e2e |
| `scripts/dev/pc-rollout/test_pc_rollout.py` | 単体試験 16 件（承認の分離・期限・worker の中の本番の承認の拒否・条件の判定・差分の違反） |
| `scripts/dev/pc-rollout.sh` | 予行の入口（celeris・script-deploy の build、bundle、e2e） |
| `scripts/dev/test-affected.sh` | `scripts/dev/pc-rollout/` の変更で単体試験を走らせる |
| `docs/ops/project-controller.md` | 4a 節（承認の 2 種類・段ごとのコマンドと go の条件・監視の違反・予行）、6 節の未実施、migration の版の記述を 117 に直した |
| `agent-docs/adr/2026-11-06-phase4-p4g-shadow-rollout.md` | D1〜D7 |
| `agent-docs/progress/2026-11-05-phase4/p4g/e2e/` | 予行の raw |

## 予行（非本番）の command と結果
- `bash scripts/dev/pc-rollout.sh --out $TMPDIR/p4g/run7` → exit 0、`summary.json` の `ok: true`・`failed: []`。raw は `p4g/e2e/` に `cp -a`、`diff -r` で差 0。初回は build 込みで 2 分 53 秒、2 回目以降は約 2 分。
- 順と結果（`p4g/e2e/ops.jsonl`）:

| 操作 | 結果 |
|---|---|
| 承認なしの dry_run の preflight | `no-go`（G1r・G6・G7 が fail/pending） |
| 観察の承認ありの dry_run の preflight | `go` |
| `apply --stage dry_run`（領域 compare・案件 dry_run） | 判断 1 件（規則 `pc/outcome-open`、効果 `suppressed`）、root 0、CoS 入力 0 |
| operator の token で shadow に上げる | 403 `project_controller_mode_requires_human` |
| shadow の preflight → `apply --stage shadow` | `go`、適用 |
| shadow の監視 91 秒（標本 19、5 秒ごと）。3 回目の標本の前に SIGKILL → 起こし直し | 違反 0、再起動を記録（`daemon_restarts` 1）、mode は shadow のまま、判断 +2（両方 `suppressed`）、起こし直し 0.22 秒 |
| 対象外の案件を人が dry_run にする（差分の検出の試験） | 200 で変わり、`diff` が `other_projects_changed` を出した |
| 観察の承認で canary に上げる | 拒否（`stage canary needs an approval of kind effects`） |
| 効果の承認（予行の 91 秒の報告の sha256）で canary の preflight | `no-go`（G5: 91 秒 < 86,400 秒、G3: 対象外の案件が dry_run） |
| operator の token で undo（off、領域は compare のまま） | `ok: true`、以後 4 秒で判断が増えない |
| 人の token で領域を rust に戻す | `ok: true` |

- 記録した資源と遅れ（予行の値。本番の推定には使わない）: `GET …/controller` p50 4.7 ms・最大 19.1 ms、daemon の RssAnon 20,544 kB（再起動前の process）→ 21,624 kB（再起動後の process）。
- 予行で分かったこと: (1) daemon の再起動は、どの領域も指していない `loaded` の generation を retire する（`startup_no_isolate`）。登録を再起動の前にすると `compare` が 422 `script_generation_invalid` になった。手順書の段 0 に「再起動の後に登録」と書いた。(2) 作った直後の案件（`proposed`）は判断が `pc/project-not-active` で止まる。予行では人の API（`PATCH /projects/{id}` `status: active`）で active にした（本番の対象は進行中の案件を選ぶ）。(3) wake から判断までは coalesce で約 30 秒かかる。

## 失敗したケース（作業中）
- 1 回目: `PUT …/mode compare` が 422 `mode compare needs a generation` → 承認に `generation` を必須にし、apply は承認の generation を渡すように直した。
- 2 回目: config の sha256 を `GET /daemon` から探して null → `GET /config` の `identity.config_sha256` に直した。
- 3 回目: generation が `not loaded or active` → 上の (1)。登録を再起動の後に移した。
- 4〜6 回目: dry_run の判断の効果 `none` を違反と数えた／shadow の判断が 0 件（案件が proposed・coalesce の 30 秒より監視が短い）→ 効果を伴う状態を `pending`・`applied`・`failed` に限り、案件を active にし、監視を 90 秒にした。

## 単体試験
- `python3 -I scripts/dev/pc-rollout/test_pc_rollout.py` → 16 tests OK。

## go/no-go の現在（2026-11-06、本番）
| 条件 | 状態 | 根拠 |
|---|---|---|
| G1 必要な修正が main にある | pass | `git_facts` の `missing_subjects: []`（cos-usage・P4-B・P4-C・P4-D の件名） |
| G1r 稼働中の release が含む | 未確認 | 本番の `GET /config` を読んでいない |
| G2・G2s・G2b config | 未確認 | 本番の config は人が読む |
| G3〜G5 | 未確認 | 本番の API に preflight を当てていない（dry_run は未実施） |
| G6 人の承認（observe） | fail | 承認が無い |
| G7 実 LLM pilot の再実行で cos_usage_null 0 | pending | operator の再実行が未実施 |

## 未実施（人の承認が要る）
- 本番の preflight・dry_run・shadow・canary・enabled、本番 config の書き換え、release の promote、daemon の再起動: どれも未実施。
- G7 の実 LLM の縮小 pilot の再実行（`--cos-route '[{ source = "claude_oauth", model = "claude-sonnet-5-5" }]'`）: 未実施。account・model・token の上限・時間の上限は人が決める。
- 24 時間以上の shadow の監視（本番）と 24〜72 時間の非本番の実稼働: 未実施。

## 人が決めること・やること
1. G7: operator に pilot の再実行を許すか（範囲・費用・account・時間・停止条件）。再実行の facts を `--pilot-facts` に渡す。
2. 対象の 1 案件を選ぶ（進行中で、root の起票が止まっても困らない案件）。
3. 観察の承認の file を書く（手順書 4a の JSON）。書いたら人が段 0〜2 を実行し、`preflight` の出力と `shadow-report.json` を残す。
4. 効果（canary）の承認は、24 時間以上・違反 0 の本番の shadow の報告を見てから別に決める。

## 未解決
- shadow 開始と canary・enabled 開始の承認の分離は運用の道具の上だけで、Rust の API は人の token なら shadow から enabled へ直接上げられる（変えていない）。
- RSS の標本は daemon と同じ host で `/proc` を読めるときだけ入る。
- 予行の差分の検出で、対象外の案件の `dry_run` は `allowed_project_ids` の外でも通る（`dry_run` は wake を積まないので許可の一覧を見ない。現行の仕様）。監視は違反として検出する。

## 提案
- Rust 側でも段を分ける: `PUT …/controller` の `enabled` への変更に、直前の shadow の期間（例: 24 時間）を要求するか、`shadow` → `enabled` の直接の昇格を人の確認の欄付きにする（ADR を書いてから）。
- `GET /api/v1/daemon` か `GET /config` に `[project_controller]` の `enabled`・`allowed_project_ids` を出すと、preflight が config の file を読まずに G2 を判定できる。

## 次の gate
- 人の判断 1〜3 の後、人が本番で段 0〜2（dry_run → 24 時間の shadow の監視）を実行する。

## 提出時の検査（提出時の HEAD で実行）
- 下の「検査の結果」に書く。
