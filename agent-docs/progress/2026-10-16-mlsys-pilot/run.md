---
title: "MLSys pilot run: 実行せず待機（人の決定 auth-ready = 認証なし）と運用セッション向け手順"
tasks: [01M52BTDJVYDZT6KGPNQV2KE6H]
unit: pilot-run
status: waiting
updated: 2026-10-16
---
# MLSys pilot run（待機）

**状態: 未実施（waiting）。done ではない。** 実 LLM 経路は 1 回も走らせていない。本ファイルの数値はすべて「実行していない」ことを示し、実測値ではない。
**pilot 結果は較正専用**: 本測定の主張には使わない（split `dev`、campaign_id `mlsys-pilot-1`、hash を本測定と分ける。runbook Part P-7）。実行後も同じ。

## 1. 待機理由（人の決定 auth-ready: 「認証なし、手順と検証タスクを残して待機」）
1. runbook Part H の H1（この root の昇格）が未済。本番 daemon は schema 83 のまま。
2. `--executor celeris` は daemon 経由で claude-code/codex を起動する。worker の中から実行すると worker が LLM CLI を起こす形になり禁止（ADR 2026-10-07-worker-no-subagents-no-llm-cli）。run・quota の記録外にもなる。
3. `CELERIS_CAMPAIGN_TOKEN` は運用セッション（人）が自分の shell で設定するもので、worker には渡さない（preflight で worker の環境に無いことを確認済み）。
4. 本番 host・本番キーは触らない。fable は使わない。

## 2. 確認項目ごとの結果
| 確認項目 | 結果 | 理由 / 証拠 |
|---|---|---|
| A/B × 2 ケース 各 1 回（P-4〜6） | 未実施 | 上記 1〜3。manifest の validate/dry-run は preflight で exit 0（`preflight.md` §3、sha256 `a3b3ec43…773840e`、within_budget true、llm_calls_made 0） |
| Jev smoke（jev_ok/fallback 区別、lane、費用・版、basis 帰属） | 未実施 | 同上（C は pilot manifest の subset 1 件）。判定器自体は `986b7ed5`（jev-live）で実装済み・mock 検証済み |
| B/C の routing 固定（routing_override）の記録 | 未実施 | manifest の B は `routing.lower_policy=mlsys-min-1, online_update=false`、C は `policy_version=jev-1`。実 task の `routing_override` は実行時に台帳へ記録される |
| キー未登録 → 受信箱通知 → 登録 → 自動再開（隔離） | 隔離試験のみ実施（実 daemon は未実施） | `cargo test -p celeris --test credential_wait_notify` → 1 passed（preflight §4、偽 webhook・一時 DB、本番キー無関係）。実 daemon での手順は §4 |
| cluster job の wait 追跡 | 該当なし | pilot の問題 2 件は cluster job を使わない（使う場合は運用セッションが `run` を同じ引数で再実行して追跡） |

## 3. 記録すべき数値（acceptance 1）
`campaign run` を実行していないため全て未取得。実行後に stdout JSON から転記する。

| 項目 | 値 |
|---|---|
| executed | 0（未実行） |
| waiting | 0（未実行） |
| blocked | 0（未実行） |
| tokens_total | 未取得（実 LLM 未実行。参考: mock 12 試行 = 113,000 は mock 由来で実測ではない） |
| tokens_missing | 未取得 |
| max_overshoot（tokens / wall） | 未取得 |
| dry-run 推定（仮置き係数） | 251,673 tokens / 5 試行（実測ではない） |

## 4. 運用セッションが実行する手順（root 昇格後）
記号は runbook（`../2026-10-16-mlsys-final-connect/runbook.md`）に従う: `B=$CARGO_TARGET_DIR/debug/celeris-campaign`、`O=$TMPDIR/campaign`、`P=agent-docs/progress/2026-10-16-mlsys-pilot`。

0. 前提: H1 昇格、H2 `GET /api/v1/secrets` で `jev_api_key` の有無確認、H4 自分の shell で `export CELERIS_CAMPAIGN_TOKEN=…`（値は argv・file に残さない）。claude-code・codex 各 1 呼び出しで認証有効性を確認し、7 日使用率の開始値 `U0`（codex / claude）を控える。
1. ひな形の準備（`$O` は NFS 外）:
   ```sh
   mkdir -p $O && cp $P/pilot-live.json $P/pilot-task-template.json $O/
   # pilot-live.json: B/C の harness_version "<release sha12 at H1>" を昇格した sha12 に、C の evidence_table_id を `celerisctl routing evidence` の版に置換
   # input.path は $O から見た相対に直す（または $P から実行）。task-template.json の project_id を案件 id に置換
   ```
2. 条件固定: `$B validate --manifest $O/pilot-live.json`（sha256 を控える。置換で preflight の hash から変わる）、`$B dry-run --manifest $O/pilot-live.json`（within_budget true）。
3. 実行（P-4）:
   ```sh
   $B run --executor celeris --manifest $O/pilot-live.json --out $O/pilot-out \
      --api-url http://127.0.0.1:<task-api port>/api/v1 --token-env CELERIS_CAMPAIGN_TOKEN \
      --task-template $O/pilot-task-template.json \
      --repo-src <checkout> --materials-src <materials> --kb-src <kb snapshot>
   ```
   初回は A/B 各 1 試行だけ（preflight 提案）で係数が読めるか確かめてから残りを投入。`waiting>0` は同じ引数で再実行し、cluster job は wait で追跡。stdout JSON の `executed/waiting/blocked/budget_cancelled/tokens_total/tokens_missing/max_overshoot_*` を §3 に転記。
4. 検査と採点（P-5）: `$B check --manifest $O/pilot-live.json --out $O/pilot-out`（ok true、violations 空）。`$B grade --case <grade-case.json> --artifact <成果物 dir>`（未接続 1・2 が残るので手動 case が要る）。
5. 読み取り（P-6）: `$O/pilot-out/summary.json` から A/B 別の試行あたり tokens `a,b,c`（`tokens_missing` 除外）、wall、`max_overshoot_*`、子 task 数・再試行数 `ρ`、共有オーバーヘッド `j`、Δ使用率 `u_codex,u_claude`。runbook「本測定の件数・反復・上限の提案式」に入れる（`s=1.25`、`E=r(na+nbρ+m(cρ+j)+nj)`、制約 `s·E≤1,000,000` と `U0+u·s·E≤U`）。
6. Jev smoke: C の試行について `jev_ok` と `fallback` が区別して記録され、lane・費用・版・basis 帰属が出ること（jev-live の判定器の出力）を確認。**鍵が未登録なら §5 の経路になる。**
7. B/C routing 固定: 各 task の `routing_override`（B: lower_policy mlsys-min-1・online_update false、C: jev-1）が task 詳細・台帳に残ることを `GET /api/v1/tasks/<id>` で確認して転記。
8. export 接続確認: runbook H5（本番 DB は写しに対して）。

## 5. キー未登録 → 受信箱 → 自動再開（隔離 daemon、人）
本番 `jev_api_key` は消さない・触らない。手順は `preflight.md` §4 の 1〜5（別 DB・別 port の隔離 daemon、本番 llm-proxy port に相乗りしない: ADR-0139）。期待: `waiting_for_auth` → 受信箱 `credential_wait` 1 件・偽 webhook 1 通知（値なし）→ 隔離側のみ `PUT /api/v1/secrets/jev_api_key` → `secret_stored` event → 同じ WU が一度だけ再キュー。登録は測定予算の承認ではない。

## 6. 人向け検証タスク（結果の戻し先: 本ファイル §2・§3）
- [ ] H1 昇格・H2 鍵確認・H4 token 設定
- [ ] 認証有効性確認と `U0` 記録
- [ ] §4 の 1〜8 を実行し §2・§3 を実値で更新
- [ ] §5 を隔離 daemon で実施
- [ ] 結果を受けて proposal WU（消費係数と本測定案）を実測値で確定

## 7. 提案（proposal WU への申し送り）
- 実測が無いため、本測定の件数・反復・上限は現時点で確定できない。承認範囲（r=1、n≤5、m≤2、試行 12、tokens 1,000,000）の枠は runbook の式で置けるが、係数 `a,b,c,ρ,j,u` は仮置きのまま。proposal は「実測待ち」と明記し、仮置き係数での暫定値は較正前として分けて書く。
