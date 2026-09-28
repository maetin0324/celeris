# Browser capability Phase 1

---
tasks: [01M3MBV3AKXZGEG5RXR60XC62J]
---

2026-09-28、[ADR-0078](../adr/0078-browser-execution-capability.md) の Phase 1 を実装。
運用手順は [browser-capability.md](../browser-capability.md)。本番の profile 設定と昇格は行っていない。

- 既存 profile の管理者 grant と `browser-enabled` skill の要求を分離。ACP/OpenCode を既定に、Claude Code を明示選択できる。
- task/run ごとの isolated session、非空の upstream action allowlist、domain policy、content boundaries を生成。
- supervisor の lifecycle と秘密を含まない操作監査、task artifacts 登録を追加。raw harness logs / RPC error反射を抑止。
- task/run GUI から既存 dashboard を開く。token URL を保存せず、実行終了後のリンクも無効化。旧 API では browser panel を省略する。
- browser agent loop、DOM/ref解決、画面転送は既存部品を利用。CredentialBroker、認証state再利用、durable wait、直接stream統合、container/egress境界は後続 Phase。

固定版 agent-browser 0.38.1 の実機 smoke で navigation/click/extract/screenshot/download、危険操作拒否、session 間の storage 分離を確認。
別 namespace の既存 dashboard も Playwright で live canvas を確認し、検証用 session/dashboard は終了した。
Rust は routing・profile・実APIのfilter/page・supervisor・raw log非露出、Python はCLI/secret sentinel、GUI は URL/旧API/終了stateを検証する。
最終 workspace gate、release/verify の機械向け証跡と検証済み SHA は task artifacts の result.json/gate.json/verify.json に保存する。

Phase 1 は公開・未認証ページ向け。同一 UID の任意 shell を隔離するものでも、任意 Web content/最終 summary の秘密を自動検出するものでもない。
機密業務への適用前に ADR の後続 policy/broker/隔離タスクと人間 decision を完了する。

既存の組織プロフィール編集は grant を保持する。初回 release の mobile audit で task タブの初期 JS が 533.1KB となり
532KB の既存予算を超えたため、browser panel は session がある場合だけ遅延ロードする。予算値は変更しない。
