---
title: TS 制御層の本番運用準備と意図的な TS 方針改善（段階移行 2）— 親 task の分割
tasks: [01M6HG7T6ZKTX239KGH0QY4HFQ]
status: running
updated: 2026-11-03
---
# TS 制御層の本番運用準備と意図的な TS 方針改善（段階移行 2）

親 task 01M6HG7T6ZKTX239KGH0QY4HFQ。前回親 01M6DCMB75G5F0DSE58ZNZMBAK（S0〜S6 done）の続き。この記録は親 task の分割・依存・停止条件を書く。実装・試験・計測は子 task が行い、子の記録は `agent-docs/progress/2026-11-03-ts-control-ops/<key>.md` に置く。

## 結論
- 親 task はこの task（01M6HG7T6ZKTX239KGH0QY4HFQ）を使う。`celerisctl ls` で未完の重複 task が無いことを確かめた（TS 制御層の未完 task はこの 1 件だけ）。
- 子 task を 8 件に分けた（下の表）。最初に並列で走るのは p0a-release-gate・p0b-cos-digest-persist・p1c-ops-observe の 3 件。
- この run（親）は分割と記録だけを行い、crates/・scripts/ を変えていない。本番・release・mode・guard・IEEE Access/K2/K4 の環境に触れていない。

## 精査した現行実装と未解決（最新 main = `git merge-base HEAD main`）
| 依頼 | 現行の file・関数 | 未解決（出所） |
|---|---|---|
| P0-A | `scripts/selfdeploy/release.sh`（control-ts の bundle を作らない。`CELERIS_SCRIPT_REQUIRE_BUNDLE` を設定しない）、`crates/celeris/tests/support/script_compare.rs`（bundle を `$TMPDIR` に作る、付記 S5-9/S5-13）、`scripts/dev/ts-verify.sh` | S3 未解決 10・S4 未解決 8・S4 の node_modules/.bin shim 書き換え（KB `control-ts-bundle-node-bin-side-effect`）。S5 を含む release の gate 秒は未計測（S6 未解決 4） |
| P0-B | `crates/task-dispatch/src/dispatcher/cos_chat/digest.rs` の `cos_digest_pass`・`apply_digest_pass`・`append_pass_record`（apply の後に別 transaction で `script_compare_log_append_observed`、失敗は警告だけ） | S6 未解決 5（apply 済み・記録なしの crash 窓、既知の制約 11） |
| P1-C | `crates/celerisctl`（script mode は rust・compare だけ、canary は curl）、`crates/celeris/src/daemon/script_guard.rs`（`script_compare_log` を毎回全件読む） | S5 未解決 6・提案 4、S6 既知の制約 2、RSS の退役繰り返しは未確認（S6「RSS・isolate 数」） |
| P1-D | 境界 ADR (f)（不一致は自動 rollback）、`scripts/dev/ts-pilot.sh`（200 文字打ち切りの pilot、差分 9 件は意図した変更） | 意図した差分を説明する仕組みが無い |
| P1-E | S5 の「本番への移行」1.〜10.、S6「決定一致率」の数え方 | 本番の一致率は未計測、本番は全領域 rust（S6 未解決 1・2） |

## 子 task と依存
| key | 内容 | 依存 | 止まる gate |
|---|---|---|---|
| p0a-release-gate | release/verify gate に bundle 作成・host API 適合・ESM/manifest hash 検証・`CELERIS_SCRIPT_REQUIRE_BUNDLE=1` の実 bundle 試験を必須化、shim 書き換えの防止、cold/warm の時間計測 | なし | release の実行は人。gate の実測は手元の release.sh の dry 相当で行う |
| p0b-cos-digest-persist | apply と compare_log・guard 計数の原子化（同一 transaction か冪等 outbox）、crash 前/中/後・再起動・重複配送・CAS 競合・rollback の試験 | なし | 原子化できない場合は理由を ADR に書き cos_digest の canary/ts を API で拒否 |
| p1c-ops-observe | celerisctl script の generations list/register/retire・canary bps・guard clear（human のみ）、未配備 hash の扱い、script_tick_wait の log/metric、compare-log の cursor 走査、RSS 推移、route_priority canary 上書きの試験 | なし（p0a と並列） | guard 解除・昇格の権限を operator/CoS に広げない |
| p1d-intended-change | parity と behavior change の分離、版付き改善仕様・expected diff fixture・人の承認・golden 更新規約（ADR と実装）、200 文字 pilot の再評価 e2e | p0b | 未承認差分・安全違反・無効評価は fail-closed のまま |
| p1e-shadow-preflight | lane_rules の本番 compare の preflight・人の手順・集計 script（本番は変えない） | p0a、p1c | 本番の操作は人 |
| p1e-shadow-measure | 人の承認と実施の後、本番 compare の実件数（純粋判定 >=1000、CoS >=200）の読み取り専用の集計と報告 | p1e-shadow-preflight | 人の承認が無ければ question で止まる。canary→ts は別の人の承認 |
| p2f-expand-port | 次の移植領域の優先順の決定と 1 領域の同等移植 | p1e-shadow-measure | 本番 compare の観測が無ければ着手しない |
| p2g-llm-self-improve | 実 LLM の CoS/worker による TS 改善の非本番実験 | p1d-intended-change | 本番の承認は自律化しない。K2/K4 評価器・hidden 資料に触れない |

## 本番切替に要る人の判断
1. p1e-shadow-measure の前: lane_rules の generation 登録と mode `compare` への切替を本番で行うか（人が S5「本番への移行」4.・5. を実行する）。
2. compare の実件数が揃った後: canary（bps）→ ts へ上げるか（領域ごとに別の承認）。
3. p0a の gate を含む release を作り promote するか。
4. p0b が原子化できず canary/ts を禁止した場合、その禁止を受け入れるか。

## 証拠（提出時の HEAD で実行）
- 基準は `git merge-base HEAD main`。この run の差分は進捗ファイル 1 件だけ。
- `bash scripts/dev/test-affected.sh` → exit 0（「試験対象の変更なし（docs のみ等）」、文書検査は ok）。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- `sh scripts/dev/progress-index.sh --check` → exit 0。
- 子 task の起票は run の成果物の delegate.json（8 件）で行う。受け付けられた件数は run の後に celeris が決める（未確認）。

## 未解決
- 子 task の結果は未着。親の完了は子の結果で決まる。
- 2026-11-03 20:12Z（親の attempt 3）時点: P0-A（01M6HGFR1T4M08TJM7KD39D443）・P0-B（01M6HGFR1TDAP02XY98ES1ZDA1）・P1-C（01M6HGFR1TR6PJ8822DHW7NNNT）は running（lease 期限 20:28Z）。P1-D・P1-E(1)・P1-E(2)・P2-F・P2-G は ready（依存待ち）。done は 0 件。
- 親の review が子の完了前に 2 回走り、条件 0・1 で不合格になった。親が子の範囲を自分の branch で実装し直すことは重複の禁止（ADR 2026-10-18-no-duplicate-child-delegation）に反するので行わない。人に、子の完了まで親の review を止めるよう依頼した（親の result.json の question）。
- 人の回答（2026-11-03）: 子の完了まで親の review を止め、子の完了後に親を再開してよい。親は子の範囲を実装し直さない。本番切替・release・canary/ts 昇格・guard 解除の承認は含まない。
- 回答後の親 run（attempt 3 の再開）時点でも P0-A・P0-B・P1-C は running、ほか 5 件は ready で done は 0 件。親は再び question で止まり、子の完了後の再開を待つ。

## 提案
なし。
