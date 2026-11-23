---
title: 戦略層 v1 子 7 obs-design — 案件の概要・chat の案の採否・publish manifest と人だけの署名の ADR
tasks: [01M7XQ9R1EHA8GQ0TZVRM4FBVX]
wu: obs-design
status: done
updated: 2026-11-23
completed: 2026-11-23
---
# obs-design（WorkUnit。設計だけでコードは書かない）

完了日: 2026-11-23。設計の正本は [ADR 2026-11-23-project-observability-publish](../../adr/2026-11-23-project-observability-publish.md)。
基準はこの WU の `$CELERIS_WU_BASE`。検査は提出時の HEAD で実行した。

## 読んだもの

- 子 0 の [design-brief](../2026-11-21-strategy-v1-0/design-brief.md) §1・§3・§4・§7。
- 子 2〜6 の ADR 5 件（strategy-evidence-contract・chat-discussion-mode・external-llm-discussion-exchange・strategy-review-critic・parent-checkpoint-evaluator）。
- `docs/architecture-map.md` の owner の行（project の DTO `task-ops::view`、controller の要約 `task-ops::project_controller_view`、
  Web の `web::features::projects`、CoS chat の REST `task-api::chat`、決定 `task-ops::decision`）と、その file の関数の実在。

## 決めたこと（ADR の節）

- D1 `GET /api/v1/projects/{id}/overview`（`celeris.project-overview/1`、読むだけ）。14 項目（「仮説と検証状況」を仮説と検証の状況に分けて数える）ごとに
  正本・返すもの・印（confirmed・estimated・needs_decision・contradicted）・強い警告の code・drill-down を表で固定した。記録の無い項目は印を付けない。
  既存の GET（project・roadmap・controller・decisions・chat messages）の欄は写さず、id と導出だけを返す。link は task・artifact・notebook・log・git・literature・chat の 7 種。
- D1.5 戦略の版の一覧は `decisions` の key `strategy-approve-v` から引き（本番 6 ms の照会、子 2 D3.4）、成果物の全走査（1.65 秒）をしない。下書きの版は概要に出ない。
- D2 `POST /api/v1/chat/threads/{t}/messages/{m}/review`（adopt・reject・comment）。人だけ（他は 403 `proposal_verdict_requires_human`）、記録は同じ thread の人の message、
  adopt は execute の人の指示の message で承認ではない（子 2 の経路へ進める）、reject・comment は discuss。revision・sha256 が違えば 409。採否の状態は導く。
  wire に `ChatMessage.proposal`・`ChatMessage.external`（untrusted の印）を足す。
- D3 `celeris.publish-manifest/1`（claims → evidence → 再現条件 → limitations・未解決）を成果物 `publish/<manifest_id>/v<n>.json` に置く。検査 M1〜M10。
  署名は決定 `publish-signoff-v<n>` への人の `sign` の回答。主体の拒否（403 `publication_requires_human`）は HTTP・CoS・MCP の共有の owner
  `task_ops::decision::plan_answer`・`plan_revise` に置く。本文が変われば `stale`、新しい版で `superseded`。Celeris は外へ何も送らない。
- D4 Web の画面の置き場所、D5 足さないもの（表・列・migration・event の種類・Manager・trait・状態機械・Agent サービス）。
- 保証の表: 予定の Rust の試験の正式名 65 件（`publish_manifest_` 14・`publish_signoff_` 13・`proposal_review_` 16・`project_overview_` 22。base には同じ名前の `fn` が 0 件）と e2e の spec 3 件。

## 証拠（提出時の HEAD で実行）

- `bash scripts/dev/check-doc-links.sh` → `check-doc-links: ok`、exit 0。
- `bash scripts/dev/check-adr-numbers.sh` → `check-adr-numbers: ok (481 files)`、exit 0。
- source の差分: `git diff --name-only "$CELERIS_WU_BASE" -- crates web scripts docs` が 0 行（変えたのは ADR とこの進捗だけ）。
- 保証の表の backtick: 節を awk で切り出し、決めた 4 つの接頭辞の試験名と 3 つの spec の path 以外の backtick が 0 件、略記（`…_`）が 0 件、重複が 0 件。
- 測定: production と test の LoC の差分は 0（文書だけ）。足した試験は無い（試験名は後の WU が実装する予定の名前）。

## 未解決・引き継ぎ

- ADR の「未確認」のとおり: 概要の応答時間の本番の大きさでの測定、子 6 の評価器の入力の欠け（goal の軸）、knowledge の page への直接の link の口、
  `.ipynb` の viewer、adopt の後の CoS の run の prompt の段の誘導、`human:local` を署名に数えない規則と戦略の承認の規則の差。
- 3 つの backend の WU は `crates/task-api/src/schema.rs`・`docs/api/v1`・`web/api/generated` を同じ段で変えるので、段の統合で生成物を再生成する必要がありうる。
- close の WU は、保証の表の試験名を `git grep -nE 'fn <名前>\(' -- crates` で 1 件ずつ確かめ、実装で名前が変わった行を正式名で直す。

## 提案

- 子 2 D2.2 の戦略の承認も `human:local` を数えない規則に揃えるかを、子 9 の closeout で人に聞く。
