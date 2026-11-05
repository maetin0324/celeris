---
title: P4-E 統括 CoS の旧新 paired 比較（準備）
tasks: [01M6PAMC3XBVDS7C2TP3WGD2E5]
status: done
updated: 2026-11-05
completed: 2026-11-05
---
# P4-E 統括 CoS の旧新 paired 比較（準備）

完了日: 2026-11-05（固定・runner・preflight・採点・stub 段の 32 試行まで）。第 4 フェーズ親 01M6PAK8E8RH3ZMQC0FAD8ZNAA の子（人の依頼 seq 2343）。基準は `git merge-base HEAD main`。ADR は `agent-docs/adr/2026-11-05-phase4-p4e.md`（D1〜D8）。

## 結論
- **実 LLM の比較は未実施。** 人の承認が無く、この run は Celeris の worker の中なので、runner の preflight 自体が実 LLM を断る（下の「実 LLM の preflight の拒否」）。相談の深さ・読取の正確さ・委譲・拒否のモデルの性能、CoS の token・cache の旧新差は**未検証**。
- 旧腕（A4 の前の main）と新腕（現行）の binary・skill・prompt を別々に固定し、同じ課題・同じ初期状態・同じ route で 32 試行を回す runner を作った。stub の CoS（LLM 0）で両腕の 32 試行が全て終わった。
- stub 段の paired（決定的な採点 20 試行）: 読取 4/4・拒否 4/4 は両腕とも合格。委譲は旧腕 0/2・新腕 2/2。旧腕は完了の要約を案件の thread に送り、元の統括 thread には「完了しました」の知らせだけが入る（`summary_in_origin` が偽）。新腕は元の統括 thread に完了の入力が戻り、task id を挙げた要約が残る。これは A4 の経路の差で、モデルの差ではない（stub の返事は両腕で同じ台本）。
- 相談 12 試行は人の採点が要るので観測に入れていない（欠測）。stub の返事は台本なので、相談の深さを stub で測る意味は無い。

## 腕の固定（manifest は `p4e/manifest.json`）
| 腕 | rev の規則 | 件名 | skill `cos-operator` の内容 hash | consult.md |
|---|---|---|---|---|
| old | 件名「a4: 統括 CoS の相談・短い読取・委譲」の最古の commit の親 | 「a2: runner の不変条件の関数型に別名を付ける（clippy）、進捗の件数を直す」 | manifest の `arms.old.skills` | 無し |
| new | 測定時の HEAD（今回は `git merge-base HEAD main` と同じ） | 「p4a: test-affected の host 依存の失敗を進捗に書く」 | manifest の `arms.new.skills`（旧と異なる） | 有り |

skill・prompt の source（`cos_chat.rs`・`preamble.rs`・`cos_chat/launch.rs`）・binary の hash はどれも旧新で異なる（`differs` 全て true）。旧腕の binary は一時 worktree で `cargo build -p celeris --bin celeris` した（83 秒、Celeris が渡した `CARGO_TARGET_DIR` の下の `p4e-old`）。

## 手順（command）
| 段 | command | exit・要点 |
|---|---|---|
| 固定 | `python3 scripts/dev/general-cos-compare/compare.py pin --out <m> --old-bin <旧 celeris> --new-bin <新 celeris>` | 0。旧新の rev・hash |
| 計画 | `compare.py plan --out <p>`（既定 repeats 2） | 0。32 試行、repeat 1 は旧→新、repeat 2 は新→旧 |
| preflight（stub） | `compare.py preflight --tier stub --manifest <m> --plan <p> --out <o>` | 0。`p4e/stub/preflight.json` |
| 実行（stub） | `compare.py run --tier stub --manifest <m> --plan <p> --out <o> --state-root $TMPDIR/...` | 0。32/32 で error なし、約 3 分。`p4e/stub/run-summary.json`・`raw/` |
| 採点票 | `compare.py grade-sheet --raw <o>/raw --salt p4e-stub` | 0。32 票（腕を伏せた blind id、対応表 `grade-key.json`） |
| 決定的な採点 | `compare.py grade-auto --raw <o>/raw --out <o>/grades` | 0。graded 20・needs_human 12（相談） |
| 観測 | `compare.py observations --raw <o>/raw --grades <o>/grades --out <o>/observations.jsonl` | 0。20 行、未採点 12 |
| 集計 | `python3 scripts/dev/general_cos_eval.py <o>/observations.jsonl` | 0。`p4e/stub/report.json` |
| 実 LLM の preflight | `compare.py preflight --tier llm --manifest <m> --plan <p> --out <o>` | **2**（拒否）。`p4e/llm-preflight-refused.json` |

## stub 段の paired（fixture tier、決定的な採点 20 試行）
| 指標 | 旧（10） | 新（10） | 新 − 旧 |
|---|---|---|---|
| 成功率 | 0.8 | 1.0 | +0.2 |
| 読取 / 委譲 / 拒否の成功率 | 1.0 / 0.0 / 1.0 | 1.0 / 1.0 / 1.0 | — |
| 無駄起票 | 0 | 0 | 0 |
| 起動（CoS+worker） | 18 | 18 | 0 |
| 道具の呼び出し（stub の API） | 8 | 8 | 0 |
| 壁時計の合計（秒） | 7.27 | 6.23 | −1.04 |
| 最初の返事までの合計（秒） | 5.20 | 4.63 | −0.57 |
| handoff | 2 | 2 | 0 |
| 人の介入 | 10 | 10 | 0 |
| token・cache・費用 | null（stub は usage を返さない） | null | null |

委譲 1 試行の内訳: 両腕とも CoS run 4・worker run 1・handoff 1。旧腕は完了の要約が案件の thread（other_threads 7 行）に行き、新腕は元の統括 thread に戻る。stub の秒は fake harness の経路の時間で、会話の応答時間として使わない。

## 実 LLM の preflight の拒否
`--tier llm` は次の 3 つで exit 2 を返した: Celeris の worker の run の中（`CELERIS_RUN_ID` がある）、人の承認 file が無い、`--llm-config` が無い。試験 `test_compare.py` は、cos を名乗る承認・上限の欠け・計画より少ない承認試行数・pin 後の binary の変更・対でない計画・本番の dir への出力も exit 2 になること、承認があり worker の外なら exit 0 になることを確かめる。

## 人に示す上限の候補（未承認）
`tests/fixtures/general-cos-eval/limits.json`: 32 試行（8 課題 × 2 腕 × 2 回）、100 万 token（input+output+cache read+cache creation の合計）、2 時間（7,200 秒）、20 USD 相当、1 試行 600 秒。どれかに達したら残りの試行を始めない（attempt 3 で USD も打ち切りに入れた。下の「attempt 3」節）。USD は task_core の静的単価表による推定で、請求額は確認していない。

人が決めること:
1. 実施の可否と account・model・harness（例: claude_max_personal・sonnet・claude_code）。
2. 上限の値と、token の上限を input+output だけで数えるか cache 込みで数えるか（P0-F の未決と同じ問い。runner の既定は cache 込み）。
3. 相談の 12 試行（3 課題 × 2 腕 × 2 回）を採点する人。

## 実施の手順（承認後、operator が worker の外で実行）
1. 旧腕を build: `git worktree add --detach <dir> <manifest の old rev>` の後、その dir で `CARGO_TARGET_DIR=<別の dir> cargo build -p celeris --bin celeris`。新腕は現行の tree で同じ build。
2. `compare.py pin --out m.json --old-bin … --new-bin …`、`compare.py plan --out p.json`。
3. 承認 file（`approved_by: human:<名前>`、`approved_at`、`account`、`model`、`harness`、`limits` の 4 値）と、人が書く `--llm-config`（`[llm_sources]`・`[harnesses]`・`[route_defaults]`・`[harness_selection]`・`[accounts]`）を用意する。
3a. seed の段（初期状態を作る daemon）は tier に依らず常に stub（`scripted_cos.py`）の harness で起こす。`--llm-config` と `--cos-route` は CoS を有効にする本番の段だけが使う。seed の段で fake 以外の adapter の run ができた試行は失敗にし、残りを始めない（[p4e-seed](p4e-seed.md)）。
4. `compare.py preflight --tier llm …` が exit 0 なのを確かめ、`compare.py run --tier llm --manifest m.json --plan p.json --approval a.json --llm-config c.toml --cos-route '<route>' --out <dir>`。
5. `grade-sheet` を採点者に渡し、`grade-auto` は読取・委譲・拒否の一次の検査に使う（採点者が確認する）。`observations --conditions <model・harness・account の JSON>` と `general_cos_eval.py` で集計する。

## 変更（commit の件名）
- `p4e: 統括 CoS の旧新 paired 比較の腕の固定・runner・preflight・採点と集計 v2 を足す`
- `p4e: 進捗と stub 段の raw を書く`
- `p4e: 進捗の表から hash に見える内容 hash を除く`
- `p4e: 進捗に文書の検査の結果を書く`
- `p4e: USD の上限で試行を止め、承認者を human: の主体に限る`

主な file: `scripts/dev/general-cos-compare/compare.py`（pin・plan・preflight・run・grade-sheet・grade-auto・observations）、`scripted_cos.py`（stub の CoS と worker）、`test_compare.py`、`scripts/dev/general_cos_eval.py`（v2 に置き換え、v1 の行は拒否）、`tests/fixtures/general-cos-eval/{cases,arms,limits}.json`。

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）
- `bash scripts/dev/test-affected.sh` → exit 0。「試験対象の変更なし（docs のみ等）」。Rust の変更は無い。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- `python3 scripts/dev/test_general_cos_eval.py` → exit 0、13 tests OK。`python3 scripts/dev/general-cos-compare/test_compare.py` → exit 0、21 tests OK（attempt 3 で 4 件を足した）。
- `cargo nextest run --workspace -E 'test(milestone_lifecycle_endpoints_are_gone)'` → 1 passed（旧 milestones API の 410 は維持）。
- `sh scripts/dev/progress-index.sh --check`・`sh scripts/dev/check-doc-links.sh`・`sh scripts/dev/check-adr-numbers.sh`・`sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv`・`git diff --check refs/heads/main`・`test -f agent-docs/progress/2026-11-05-phase4/p4e.md` → すべて exit 0。progress-index は最初、表の内容 hash の先頭 12 桁を commit hash とみなして exit 1 だった。manifest の欄の参照に置き換えて直した。

## 失敗ケース（この run で出て直したもの）
- 試行の daemon が host の `~/.local/celeris/releases` を読み、未昇格の release が受信箱に入って CoS run が増えた。`[selfdeploy] releases_dir` と `[maintenance] disk_watch = []` を試行の dir に向けて直した。
- seed の task の完了・失敗の知らせで CoS が起動し、課題の前の返事を課題の返事と数えた。seed を CoS 無効の daemon で作り、静かになった後の seq を基準にして直した（実 LLM で seed の分を消費しないためでもある）。
- 委譲の試行で、終わりの判定の直後に完了の知らせの一次対応が起きる回と起きない回があった（新腕 r1 だけ CoS run 4、r2 は 2）。終わりの後に 5 秒続けて静かになるまで待ってから集めるように直し、両腕・両回で 4 に揃った。

## attempt 3（review の差し戻し 2 件の修正）
1. USD の上限が効かなかった。`compare.py` の `run` は token と壁時計しか見ず、原票の `cost_usd` は常に null だった。直した内容:
   - 原票の `cost_usd` は、CoS chat run と worker・reviewer の run の `usage.cost_usd` の合計にした。`usage.cost_usd` は task_core の静的単価表の推定で、請求額ではない（請求額は確認していない）。`cost_usd` の無い run が 1 件でもあれば null にし、`cost_unknown_runs` に件数を書く。stub は null のまま。
   - `stop_reason` が token・壁時計・USD の順に上限を見る。実 LLM で費用が null の試行が 1 件でもあれば `usd_unknown` で残りの試行を始めない（fail closed）。打ち切りの理由は `run-summary.json` の `stopped.reason`、累計は `spent.usd`・`spent.usd_unknown_trials` に出る。
   - `observations` の `cost_usd` は原票の値を写す（以前は null 固定）。
   - 注意: 承認者が単価表に無い model を選ぶと、最初の試行の後に `usd_unknown` で止まる。実 LLM の前の 1 対（提案 1）で `cost_usd` が埋まることを確かめる。
2. 承認者の検査が緩かった。`approved_by` は正規表現 `human:[^\s:]+` に一致するときだけ通す。`claude`・`reviewer`・`rmaeda`・`human:`・`human: rmaeda`・`operator:claude-code`・`Human:x` は exit 2。
- 試験（`test_compare.py` に 4 件）: `test_approved_by_must_be_a_human_principal`、`Limits.test_stop_reason`、`Limits.test_run_stops_when_usd_reaches_the_limit`（0.6 USD の試行 3 件で上限 1.5 に達し、4 件目を始めない）、`Limits.test_run_stops_when_cost_is_unknown`（2 件目の費用が null で止まる）。run の試験は試行を偽物に差し替え、daemon と LLM を起こさない。
- ADR D5 と `limits.json` の note を同じ内容に直した。stub 段の raw（`p4e/stub/`）は修正前のコードの出力で、`run-summary.json` に `spent.usd` は無い。stub の費用は元から null で、採点と集計の数値は変わらない。

## 境界の確認
- 実 LLM・有料 API・本番の DB/config/daemon・systemctl・promote・本番 shadow・24〜72 時間の稼働はしていない。試行は `$TMPDIR` の一時 home・一時 DB・loopback の daemon と fake harness だけ。CPU を焼く負荷はかけていない。
- 旧 milestones API の 410、human_required、許可の保護、TS と Rust の分担、K2/K4/IEEE Access の凍結評価には触れていない。TS の policy・bundle と Rust のコードは変えていない（TS-only の edit→verify の cargo 0 回にも影響しない）。

## 未実施（人の承認が要る）
- 実 LLM の 32 試行（上の上限の候補で）。相談の深さ・読取の正確さ・不要な root の抑制・安全な拒否・元 thread への要約の**モデルとしての**旧新差、CoS の token・cache・usage の旧新差は未検証。

## 未解決
1. 相談の採点は人が要る（決定的な採点器は相談の欄を埋めない）。採点者が決まっていない。
2. 実 LLM の段では、旧腕の daemon が新しい `--llm-config` の欄を読めない可能性がある（旧腕は A4 の前日の main で、config の欄の差は未確認）。preflight はこれを検査しない。operator が最初の 1 対で確かめる（提案 1）。
3. stub の秒は経路の時間で、実 LLM の応答時間の予測には使えない。
4. 旧腕の build の worktree は run の `$TMPDIR` に作り、run の終わりに `git worktree remove` で消した。

## 次の gate
- 人が上の「人が決めること」1〜3 を決める。承認後、operator が worker の外で「実施の手順」を実行し、raw と採点を `agent-docs/progress/2026-11-05-phase4/p4e/llm/` に取り込む。観測した失敗だけを直す。

## 提案
1. 実 LLM の前に、承認の範囲内で 1 対（例: `read-three` の旧新 2 試行、`compare.py run --only 7,8`）だけ流し、旧腕の config の読み込みと usage の記録を確かめてから残りを流す。
2. token の上限の数え方は P0-F の未決と一緒に決める。
