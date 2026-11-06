---
tasks: [01M6QW0GSM7ZHFNP3PWZJJ4SNF]
---
# ADR 2026-11-06: P4-E の実 LLM の承認に id・期限・範囲（scope.seqs）を必須にし、旧腕の config を LLM なしで確かめる（第5フェーズ N3）

- 状態: 採用。実装は `scripts/dev/general-cos-compare/compare.py`。
- 決めた人: N3（01M6QW0GSM7ZHFNP3PWZJJ4SNF）の worker。task の objective「既存承認が今も有効（範囲・期限・上限）なときだけ再試行」
  「32 試行全体を 1 対の承認で実行しない」の範囲で決めた。
- 関係: [ADR 2026-11-05-phase4-p4e](2026-11-05-phase4-p4e.md) の D5（実 LLM の門）を D1・D2 で改める。
  [ADR 2026-11-06-budget-usd-caps](2026-11-06-budget-usd-caps.md)（上限は USD。token の上限は持たない）。

## 背景
- 2026-11-05 の承認 file（`/local/celeris/data/scratch/p4e/approval.json`）には期限も範囲も無かった。note には「1 対の後、同じ上限で残り 30 を流す」
  と書かれていたが、runner はこれを区別できず、同じ file で 32 試行全部を流せた。
- 旧腕（A4 の前の main）の daemon が新しい `--llm-config` を読めるかは、実 LLM の 1 対を流すまで分からなかった（P4-E の未解決 2）。

## 決定
- D1 承認 file は `id`・`approved_by`（`human:<名前>`）・`approved_at`・`expires_at`（どちらも ISO 8601 の UTC）・`account`・`model`・`harness`・
  `limits`（`max_trials`・`max_wall_secs`・`max_usd`）・`scope.seqs`（承認した試行の seq の list）を必須にする。preflight と `run --tier llm` は
  次のどれかで exit 2 にする: 欄の欠け、`expires_at` を過ぎた・`approved_at` が未来・`expires_at <= approved_at`、選んだ試行（`--only`、無ければ計画の全部）
  に `scope.seqs` の外の seq がある、選んだ試行が旧新の対でない、選んだ試行の数が `max_trials` を超える。1 対の承認（`scope.seqs: [7, 8]`）で
  残りの 30 試行は流せない。残りは別の id の承認で流す。
- D2 `compare.py config-check` を足す。旧腕・新腕の binary を、本番の段の config（tier=llm なら `--llm-config` を足したもの）で `--mode verify`
  （dispatch しない・worker を起こさない）で起こし、API が答えたら止める。`[accounts] claude_dir` は空の一時 dir に差し替え、本番の account の dir を
  読ませない。config の中に本番の path（`PROTECTED`）が無いこと、`releases_dir` と `disk_watch = []` が試行の dir を向くことも記録する。LLM を起こさない
  ので worker の run の中でも実行できる。
- D3 token（cache 込み 100 万）の上限は runner で強制しない（D5 の改め以降、上限は USD だけ）。`run-summary.json` の `spent.tokens`
  （input+output+cache read+cache creation）に記録し、人が承認の範囲と照らす。

## 結果
- 承認の欄が増えたので、2026-11-05 の承認 file はそのまま使えない（`max_tokens` を含むことでも既に拒否されていた）。人が D1 の形で書き直す。
- 試験は `test_compare.py`（期限・範囲・欄の欠け・run の拒否・config-check の失敗の報告）。
