---
title: "ADR: 最小 campaign 評価基盤（manifest・hash・split・dry-run）"
status: accepted
date: 2026-10-15
tasks: [01M4Z2989XX5XK159CCDNV1HYH]
---
# 最小 campaign 評価基盤

## 文脈
MLSys 向けに A（既存 baseline）/ B（Celeris+fixed）/ C（Celeris+Jev）を比べる。別の巨大オーケストレータは作らず、routing-eval の manifest/hash/split を再利用する。

## 決定
- D1: 新 crate `crates/campaign`（`celeris-campaign`）。campaign manifest は routing-eval の入力 manifest を path+sha256 で固定参照し、problem id・split（dev↔`dev`、final↔`held-out`）を照合する。routing-eval 本体は変えず、同じ方式の hash を campaign 側 `manifest_hash` に持つ（scope 制約）。
- D2: hash は正規化（problems・subset・configs 整列、キー順固定の canonical JSON）後の sha256。`sha256` 欄は空なら未封印、入っていれば内容と一致を要求。
- D3: dev と final は 1 campaign 内で混ぜない。manifest 検査（problem の split 不一致を拒否）と集計（`campaign_sha256`・split 不一致の trial を拒否）の両方で強制する。
- D4: 主比較は A/B 固定、B/C は manifest の `secondary.subset`（事前登録）だけ。budget・termination の欠落／0 は拒否。
- D5: campaign 側の集計は error/timeout/cancelled/missing を分母（宣言した問題数）から除かず、各内訳を出す。routing-eval の根拠表は従来どおり error/cancelled を分母から外して除外数を出す（意図した別物）。
- D6: routing-eval `aggregate::build` は問題の所属を見ない。呼び出し側が `campaign::aggregate::dev_trials` で manifest の dev 問題だけに絞ってから渡す。routing-eval 本体は変更しない。回帰試験は `crates/campaign/tests/routing_eval_identity.rs`。
- D7: dry-run は実 LLM を呼ばない。token/wall/disk/並列の見積りは全て `estimated` ラベルで、仮定値を出力の `assumptions` に載せる。実測値が無い欠測の仮置きで、実性能・実費用の主張に使わない。

## 帰結
runner・checker・export・fixture は後続 unit。

## 追補（2026-10-16: 実実行アダプタ。tasks: 01M51QR9CCRP89GWXZDR4XB3S6）
実装: `crates/campaign` の `ledger.rs`・`harness_exec.rs`・`celeris_exec.rs`・`dispatch.rs`・`curl_transport.rs`。CLI `--executor mock|harness|celeris`（未知はエラー）。

- D8: 台帳キーと永続化順序。台帳の key は (manifest sha256, problem id, arm, repetition) で、外部へ渡す冪等キーは `campaign-<sha256 hex>`。順序は「(1) planned を fsync 付きで台帳へ追記（routing 設定も同時に記録）→ (2) 冪等キーで照会（lookup）→ (3) 無ければ投入（submit）→ (4) task/run ID を追記（submitted）→ (5) 外部待ちなら waiting」。投入成功後に応答が失われても、再起動後は (2) の照会で同じ task に辿り着き、重複投入しない。台帳は追記専用で、`planned` のまま task_id が無い行は照会から再開する。task-api に冪等キー欄が無いため label（先頭 64 文字）で代用し、投入前後の照会と競合時は最古へ寄せて新しい方を cancel する。
- D9: task 欄 routing 固定。B=`fixed`、C=`decision_assisted` を試行ごとに `POST /tasks` の `routing_override` で固定し、台帳へ投入前に同じ値を記録する（`lower_policy` 省略時は `mlsys-min-1` を明示、`online_update` 省略時 false、C は `evidence_table_id` 必須で最新版への暗黙追従を拒否）。本番全体 config の切替には依存しない。A は Celeris に投入しないので routing を持たず、manifest に書くと validate が拒否する。
- D10: A の無制限方針。A は既存の強いハーネス（`claude -p …`）を子 process としてそのまま包み、許可・turn 数・token 等を後付けで制限しない。足すのは試行 timeout（manifest の `trial_timeout_secs`）による process group 終了と、台帳（pid・start ticks で再起動後も同じ process を追跡）だけ。起動目印が残り meta が無い（spawn 中に落ちた）場合は二重起動を避けて止め、人が確認する。
- D11: Wait と workspace。`ExecStatus::Wait` の試行は台帳が参照する workspace を runner が消さない（`Ledger::is_referenced`）。外部待ちは結果を記録せず次回の resume に回し、完了後に初めて解放する。
- D12: 子 task・クラスタ job の追跡と cancel 方針。`track`/`poll` は `GET /tasks/{id}` の `children` を幅優先で辿り、子 task ID と cluster job（`scheduler:cluster:job_id`）を台帳へ積み増す。根が done でも子孫と cluster job が全て終端になるまで Wait。打ち切り（試行 timeout・予算停止）では、終端でない子孫を深い順 → 根の順に `POST /tasks/{id}/cancel`（409=既に終端は成功扱い、全対象を試してから Err を集約）。cluster job は自動で消さず、`scancel`/`qdel` の人手手順を `out/cluster-cancel.jsonl` に追記する（`executed:false`）。
- D13: 秘密の扱い。`--executor celeris` が取る引数は `--api-url` と token の環境変数「名前」（`--token-env`）だけ。値は要求ごとに環境から読み、curl の設定を標準入力で渡して argv・出力・台帳に出さない。task の雛形（acceptance・project 等、秘密でない）は `--task-template` で渡せる。run 出力には台帳の要約（行数と状態別件数）を足す。

## 帰結（追補）
実 API・LLM を使う実経路は本 ADR の範囲で未検証。親 root に書かれた承認済み予算の範囲で、人または認証の使える環境が別途実施する。
