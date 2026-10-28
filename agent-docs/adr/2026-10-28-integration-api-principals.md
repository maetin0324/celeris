# ADR: 統合 API を共通認証に通し、介入に主体を残す

- Date: 2026-10-28
- Status: Accepted
- Task: 01M60JC1P2CYMFPZR9KY8E6494

## 背景

統合の router が guard 適用後に merge されており、管理 token の照合が成功しても
Caller の extension が無く、actor が human:local になった。受信箱の answer の内部転送も同じ構造だった。

## 決定

1. integrations を handlers の共通 router に guard より先に組み込む。受信箱の内部転送も同じ router を使う。
2. Caller は extension の欠落を拒否する。human:local を作れるのは認証 middleware が認証無し構成と判定した場合だけ。
3. 既存の認可は維持する。human/operator は統合処分を実行でき、MCP token は読み取り専用、CoS の直接 mutation は拒否する。
   CoS の統合依頼回答・integrate-task は既存の operations 経由で cos:<thread> を記録する。
4. IntegrationMerged / Discarded / Held / Reopened の actor と IntegrateTaskRequested の by を decision log の recovery 介入へ写す。
   human-ops は既存の actor_kind 別集計を使い、人以外を人の件数に含めない。
   sweep の検出・not_applicable は操作ではないので介入に含めない。既存 events は書き換えない。

根拠: [A1](2026-10-27-api-principals.md)、[A2](2026-10-27-event-principals.md)、
[介入の主体](2026-10-27-decision-log-intervention-actors.md)。schema・本番 config の移行は不要。
