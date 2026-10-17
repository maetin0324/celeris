---
title: "MLSys 本測定 run-final: 運用セッションが人の shell で実行するコマンド列"
tasks: [01M52D2AM5K6GCFGEJZW4REWM2]
unit: freeze-final
status: ready-for-operator
updated: 2026-10-16
---
# run-final（運用セッション用。worker・agent は実行しない）

根拠: runbook（`../2026-10-16-mlsys-final-connect/runbook.md`）Part M・Part H の H5、条件は `freeze-final.md`。**値（token・鍵）はこのファイルに書かない。** `--token-env` には環境変数の**名前**だけを渡す。
本測定は人の決定で承認済みの枠の内側。当初は「pilot の提案どおり」（試行 12 以内・tokens 1,000,000・fable 不使用）。2026-10-16 の人の決定で、pilot の実測（cache を含めると 1 試行 11〜25 万 tokens）を受けて token_basis = all（cache を含む全量）に定め、上限を全体 3,000,000・per_case 300,000・per_arm 1,300,000 に、claude の 7 日使用率の上限を 45% に引き上げた（試行 12 以内・fable 不使用は据え置き）。枠を超える変更は新 campaign_id と新しい決定が要る。

## 0. 記号と前提
```sh
cd <この repo の checkout>               # worktree の agent-platform
B=$CARGO_TARGET_DIR/debug/celeris-campaign
CT=$CARGO_TARGET_DIR/debug/celerisctl    # 本番では ~/.local/celeris/current/bin/celerisctl
P=agent-docs/progress/2026-10-16-mlsys-final
D=$P/data                                # 出力置き場（final-out / final-export）
```
前提（人）: H1 昇格（sha12 を控える）、H2 `GET /api/v1/secrets` で `jev_api_key` の有無、H4 自分の shell で `export CELERIS_CAMPAIGN_TOKEN=...`（値は argv・file に残さない）。7 日使用率の開始値 `U0`（codex / claude）を控える。

## 0.5 campaign 専用の案件と A のアカウント（2026-10-17 追加。人が 1 回だけ作る。ADR 2026-10-17-mlsys-pilot2-fixes D4/D5）
試行 task は自己改善の main に自動取り込みされてはならず、`POST /tasks` は local workspace とリモート repo の組合せを 422 で拒否する。
専用案件は `auto_integrate=false`（既定）で、選ばれる repo（`task-template.json` の `repos`、無ければ primary）が **ローカル**の空 git repo か、repo 無し。
```sh
API=http://127.0.0.1:<task-api port>; AUTH="Authorization: Bearer $(cat ~/.config/celeris/api.token)"   # 値は表示しない
git init -q $HOME/.local/celeris/campaign-scratch                      # 試行は repo を使わない。空でよい
curl -s -H "$AUTH" -H 'Content-Type: application/json' -X POST $API/api/v1/projects -d '{"title":"campaign mlsys (no auto integrate)","request":"campaign 試行専用。取り込みはしない"}'
#   → 返った id を <案件 id> に。auto_integrate が false であることを GET /api/v1/projects/<id> で確かめる
curl -s -H "$AUTH" -H 'Content-Type: application/json' -X POST $API/api/v1/projects/<案件 id>/repos -d '{"location":{"kind":"local","path":"'$HOME'/.local/celeris/campaign-scratch"},"name":"scratch","is_primary":true}'
```
`task-template.json` は `{"project_id":"<案件 id>","repos":["scratch"],"acceptance":[…]}`。preflight が 422 の組合せと `auto_integrate` を投入前に拒否する。
A のアカウント: `GET /api/v1/llm/accounts` に出る claude_oauth の `account_id`（ログイン済み）を `--a-account` に渡す。A はその `CLAUDE_CONFIG_DIR` で動く（既定の `~/.claude` は使わない）ので、消費はそのアカウントの 7 日使用率に出る。A は bubblewrap の中で動く（`bwrap` が必須。preflight が実際に起動して確かめる）。

## 1. 置換（H1 後に 1 回だけ。置換したら hash が変わるので 2 で控え直す）
`$P/final.json` の次を編集する（それ以外は変えない）:
- B・C の `harness_version` の `<release sha12 at H1>` → 昇格した sha12（12 桁 hex）。
- A の `harness_version` の `claude-code@<claude-code version at H1>` → 実際の版（`installed` 等の alias は不可）。
- C の `routing.evidence_table_id` の `<routing evidence table id at H1>` → `$CT routing evidence` の版（pilot と同じ版）。`lower_policy`/`online_update` は変えない。
- 未解決の `<…>`・alias・routing 欠落があると `validate`（final split）が失敗し、起動前に止まる。
- `$P/task-template.json` の `project_id` `<案件 id>` → 案件 id。acceptance は API の形 `{"type": "reviewer", "text": …}`（入れ子の `check` は起動時に拒否）。
- `final.json` の `budget.token_basis` は人の決定（2026-10-16）で `all` に固定済み。**既定は無い**ので、別 campaign で変えるときも明示する。埋めないと manifest が読めず run は始まらない。試行記録・summary・export には基準に関係なく input / output / cache_read / cache_creation が全部残る。
`harness_version` の置換後の `final.json` の sha256 が実条件。以後 manifest・入力・snapshot を変えない（変えるなら新 campaign_id）。

## 2. 条件固定（Part M-1, M-2）
```sh
$B validate --manifest $P/final.json         # sha256 を控える（freeze-final.md の置換前 hash と別物になる）
$B dry-run  --manifest $P/final.json         # within_budget true、total_trials 6 を確認
```

## 2.5 preflight（run の前に必ず通す。4 の `run` も内部で同じ検査を通ってから始まる）
```sh
export CELERIS_CAMPAIGN_TOKEN=...      # 人の shell で。値は preflight の出力・ログに出ない（名前の有無だけ見る）
$B preflight --manifest $P/final.json --api-url http://127.0.0.1:<task-api port> --token-env CELERIS_CAMPAIGN_TOKEN \
   --task-template $P/task-template.json --materials-src $M --kb-src $K --repo-src $R --inputs-src $I --a-account <account_id> \
   --cases-root crates/campaign/cases --grader-root crates/campaign/fixtures/grader-private \
   --grade-case-root crates/campaign/fixtures/grade-cases      # 既定値と同じ。A の実行ファイルは --a-program（既定 claude）
echo $?       # 0 = 全件 OK。非 0 = stderr の JSON の `missing`（check 名 + 不足の内容）を直してやり直す
```
検査: 外部入力（`--inputs-src` に case の宣言した file が全て在り sha256 が一致。k4 の `main-v3-n2-43706.tgz`。k2 は初期 repo の `--repo-src`）／task template が 422 になる組合せ・自動取り込み案件でないか（task-api への GET のみ）／A の隔離（bwrap）とアカウント／manifest／版（placeholder・alias・routing）／入力 manifest／全 case 入力（control・split・材料 hash）／grade-case/1 全件（固定 hash・公開／隠しテストの実体）／候補構成／認証・環境（A の実行ファイル、API origin、token 環境変数の有無）／予算・時間（承認枠 300 万・30 万・130 万・並列 1・token_basis all、K2/K4 の時間 ≦ trial timeout ≦ per_case、dry-run 見積り）。
一件でも不足なら A を含め有料実行・task 作成・台帳作成を 0 件で非 0 終了する。preflight 自体は有料実行をしない（`paid_calls_made` 0）。**本番昇格（H1）と実測は人が行う。** preflight が見られないもの: claude-code の実際の版が manifest の値と一致するか、token が API に受理されるか（実経路は本測定の初回で確認）。

## 3. 入力の配送（試行へ渡す公開部だけ。grader-private は渡さない）
```sh
M=$TMPDIR/final-materials; K=$TMPDIR/final-kb; R=$TMPDIR/final-repo   # NFS 外
mkdir -p $M $K $R
for c in k2-bf-xattr-restart-campaign k4-cmp4-validity-analysis; do
  cp -r crates/campaign/cases/final/$c/materials $M/$c
done
cp -r crates/campaign/cases/final/k2-bf-xattr-restart-campaign/kb/. $K/
# R: k2 の初期 tree（BenchFS 086c4618、case.json の initial.fetch の手順で tar を展開）。k4 は repo 不要。
# I: 外部入力（case.json の inputs[]）。<name> の平らな配置。run が sha256 を照合して各試行の workspace の inputs/<name> に置く（A/B/C 全部）。
I=$TMPDIR/final-inputs; mkdir -p $I
cp <main-v3-n2-43706.tgz の実物> $I/main-v3-n2-43706.tgz   # sha256 は case.json の値と一致しなければ preflight/run が拒否
```
`$M` の hash は `final.json` の `snapshot.materials_sha256`（`freeze-final.md` に値）と一致しなければ run が拒否する。

## 4. 本測定（Part M-3。A/B 主・B/C 副）
```sh
$B run --executor celeris --manifest $P/final.json --out $D/final-out \
   --api-url http://127.0.0.1:<task-api port> --token-env CELERIS_CAMPAIGN_TOKEN \
   --task-template $P/task-template.json \
   --repo-src $R --materials-src $M --kb-src $K --inputs-src $I --a-account <account_id>
```
`$B run --executor harness` は A だけを走らせる確認用（同じ `--a-account`・`--api-url`・`--token-env` が要る）。A の実行中の token は上限（実効上限 `min(per_case, per_arm の残り)`）を超えると約 1 秒の周期で止められ、`cancelled`（`case_budget`）になる。最大超過は `summary.json` の `overshoot_granularity`。
`--api-url` は server の origin だけ（`/api/v1` を付けると拒否。実行器が足す）。`--out` は相対でもよい（起動時に絶対 path にする）。`$D` は NFS 外。
stdout の JSON（`executed / waiting / parallel_deferred / no_artifact / executor_errors / blocked / budget_cancelled / tokens_total / tokens_missing / max_overshoot_*`）を `$D/final-run-N.json` に保存する（N は実行回数）。
`budget.max_parallel`（1）は in-flight の上限: 1 回の run は 1 試行だけを投入して `waiting`（`parallel_deferred`）で戻る。**`waiting=0` になるまで同じコマンドを繰り返す**（前の試行の結果・使用量を記録してから次を出す）。`executor_errors>0` なら `trials.jsonl` の `error` 欄（HTTP status と本文）を見る。
**再開（Part M-4）**: 中断・`waiting>0`・daemon 再起動の後は上と**同じコマンドをそのまま**再実行する（`resumed_skipped` に数えられる）。別 hash で同じ `--out` は拒否される。cancel された試行の cluster job は `$D/final-out/*/cluster-cancel.jsonl` の `scancel`/`qdel` を人が実行（H6）。

## 4.5 7 日使用率と A の消費（run のたびに）
```sh
$B usage --manifest $P/final.json --out $D/final-out --api-url http://127.0.0.1:<task-api port> --token-env CELERIS_CAMPAIGN_TOKEN --a-account <account_id> > $D/usage-N.json
```
exit 0 = A のアカウントを含む claude が 45%・codex が 21% 以内（`--max-7d` / `--max-7d-codex` で変えられるが、承認枠を超える変更は新しい決定）。
A のアカウントの使用率が観測できない（`usage` が null）ときは非 0。腕ごとの tokens（A を含む）が `campaign_tokens_by_arm` に出る。

## 5. 検査・独立採点（Part M-5, M-6）
```sh
$B check --manifest $P/final.json --out $D/final-out          # ok true、violations 空。complete=false なら incomplete_reasons を読む（未実行・作成失敗の試行、未採点）
# 試行ごと（対応表は freeze-final.md §4）。grade-case は $P/grade-case/<case-id>.json（grade-export WU が作る）。評価 command は実際に走る。
$B grade --case $P/grade-case/<case-id>.json --artifact $D/final-out/trials/<A|B|C>/<case-id>/rep1/workspace > $D/grade-<arm>-<case-id>.json
# まとめて: grade-case dir の <problem_id>.json を引き、$D/final-out/grades.jsonl を書く（grade-case が無い problem は missing_cases に出て exit 1）
$B grade-out --manifest $P/final.json --out $D/final-out --cases $P/grade-case
```
exit 0 = final_pass、非 0 = 判定 JSON が stderr（`2> $D/grade-….err` に残す）。**6 試行すべてに判定を付ける**。失敗・timeout・cancelled・未完了も分母に残す（除外しない）。

## 6. export（Part M-7, M-8 と H5）
```sh
$B export --manifest $P/final.json --out $D/final-out --dest $D/final-export
# Celeris 側の消費・判断ログ。本番 DB は読むだけ、写しに対して実行
$CT db backup $TMPDIR/final-db.sqlite3 --src <本番 DB path>   # 読み取り専用接続の VACUUM INTO。稼働中でも 1 snapshot で終わる
$CT --db $TMPDIR/final-db.sqlite3 campaign export mlsys-final-1 --out $D/exp-db --verify-connected --manifest-hash <手順 2 の sha256>
echo $?            # 0 = 接続 OK。非 0 は stderr の `not connected:` 行
```
`summary.json` の `shared_overhead`、`elapsed_ms` と `run_wall_sum_ms`、`total.cash_cost_usd`（取れなければ null）、終了時の 7 日使用率（codex 21%・claude 45% 以内か）を進捗（`../2026-10-16-mlsys-final/` の次 WU の file）に転記する。

## 7. 注意
- 本番 host 操作（昇格・systemctl・config 変更）はここに含めない。H1 は GUI/`promote.sh`。
- pilot の結果は較正専用で、本測定の主張に混ぜない。dev の case・入力・設定を final に持ち込まない。
