---
title: LLM の health finding の根拠を照合する
tasks: [01M6ZDF9Q4BWD9DX6S9KX3KKS1]
status: done
updated: 2026-11-09
---
# LLM の health finding の根拠を照合する

完了日: 2026-11-09

## 変更
- `phase 1: ground blocking health findings in candidate evidence` — `health_review/verdict.rs` で候補の差分と根拠を照合し、根拠のない blocking は `health:ungrounded` 付きの advisory にして残す。

## 実装前 brief
1. 根本原因: blocking の自己申告を無条件に不合格へ写していた。
2. 責務 owner: 地図の「健全性の review」にある `health_review/verdict.rs` の `with_health`。
3. 再利用先: `HealthReviewInput` の差分・report・owner 行・brief、既存の `blocking_repair_hint` と repair 分類。
4. 選択肢: 呼び出し元ごとの判定ではなく、verdict の写像で一度だけ照合する。根拠は既存の入力から読む。
5. 追加: 根拠を照合する純粋関数。新しい状態・fallback・外部 API は無し。
6. 撤去: blocking の無条件な不合格への写像。
7. 不変条件: 候補の差分内の path と根拠が揃った finding だけが不合格になる。advisory は reviewer 自身の判定より前に置き、元の不合格を上書きしない。未知の kind・severity と欠落した health は引き続き不合格。

## 契約と試験
既存の [G3 ADR](../../adr/2026-11-08-health-enforcement-loop.md) D3.3 を実装した。根拠として、該当 repo の reviewable な report の fingerprint、該当 path の owner 行全文、brief.removals の項目を reason 内で照合する。空文字は根拠にしない。
この unit の Objective に従い、具体的な path と正の行番号（`path:12`・`path#L12`）も受け入れる。行の内容の真偽をここで再検証するのではなく、reviewer が具体的な証拠を引用しているかを判定する。

ADR の後続 unit 欄は `health_limits_ungrounded_…` を limits に割り当てているが、この unit が `health_enforce_llm_finding_` の試験を持つ。既存の分類試験は根拠の有無を区別していなかったため、根拠の欠落・差分外・別 repo の材料・空の anchor・advisory の順序の試験を追加した。既存の repair/replan 分類試験と scenario の偽 reviewer には path と行の引用を補い、分類の期待値は維持した。

WU の check・段の統合への LLM の配線は追加していない。担当範囲は task review の写像だけ。

## 証拠（提出時の HEAD で実行。基準は $CELERIS_WU_BASE）
- `cargo nextest run -p task-dispatch -E 'test(health_enforce_llm_finding_) | test(health_review)'` → exit 0、96 passed（追加7件を含む）。
- `bash scripts/dev/test-affected.sh` → exit 0、3614 passed、11 skipped。対象は celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch。doc test も exit 0。
- 同台本内の `check-health.sh` → exit 0、必須 rule はすべて pass、fail_grade=0、worsened=0。作業ツリー検査なので report の reviewable=false。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0、差分無し。
- `sh scripts/dev/check-doc-links.sh`・`sh scripts/dev/check-adr-numbers.sh`・`sh scripts/dev/progress-index.sh --check` → それぞれ exit 0。
- `sh "$CELERIS_WU_SCOPE_PATHS"` と許可した4 path の照合 → 範囲外無し。`git diff --check` → exit 0。
- 最後の同期以降に検査したソースを変更せず提出。ログは run の artifacts の grounding-tests.log・test-affected.log・clippy.log・fmt.log。

## 未解決
control-ts の offline install が失敗し、検査台本は bundle を作れなかった。bundle に依存する59件は内部 skip（上の3614 passedに含まれる）であり、その動作は未検証。この unit は control-ts を変更していないため台本は exit 0。既存の vendor の deprecated warning と試験の unused import warning が出たが、workspace clippy は通った。

## 提案
無し。
