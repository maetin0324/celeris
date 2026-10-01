# Browser capability の main 統合と ADR 番号対応

---
tasks: [01M3VSNWDCD1TD5KAG3MCVZFE0]
---

Browser Phase 1〜4 のブランチ `478e86c4` にあった Phase 3/4 の ADR は、リファクタ後の main が使用した 0081・0082・0083・0089 などの番号と衝突していた。main の既存 ADR と参照を維持し、browser 側の 16 文書を main の最大番号 0098 の後に振り直した。旧番号 0087・0088・0094 は browser ブランチ内でも各 2 文書に重複していたため、ファイルごとに対応を示す。

| 旧番号 | browser 文書 | 新番号 |
|---|---|---|
| 0081 | [Phase 3 control lease](../adr/0099-browser-phase3-control-lease.md) | 0099 |
| 0082 | [Phase 3 live proxy ACL](../adr/0100-browser-phase3-live-proxy-acl.md) | 0100 |
| 0083 | [Phase 3 identity contract](../adr/0101-browser-phase3-identity-contract.md) | 0101 |
| 0084 | [Phase 4 isolation/injection/routing](../adr/0102-browser-phase4-isolation-injection-routing.md) | 0102 |
| 0085 | [Phase 4 runtime selection](../adr/0103-browser-phase4-runtime-selection.md) | 0103 |
| 0086 | [Browser egress transport](../adr/0104-browser-egress-transport.md) | 0104 |
| 0087 | [P4-A same-UID bwrap runtime](../adr/0105-browser-p4a-same-uid-bwrap-runtime.md) | 0105 |
| 0087 | [P4-C conformance dispatch](../adr/0106-browser-phase4-conformance-dispatch.md) | 0106 |
| 0088 | [Fallback candidate preparation](../adr/0107-browser-fallback-candidate-preparation.md) | 0107 |
| 0088 | [P4-A relay/supervisor/restore](../adr/0108-browser-p4a-relay-supervisor-launch-restore.md) | 0108 |
| 0089 | [P4-B injection IPC/CDP sink](../adr/0109-browser-p4b-injection-ipc-cdp-sink.md) | 0109 |
| 0091 | [P4-B H3 shared CDP/trusted selector](../adr/0110-browser-p4b-h3-shared-cdp-trusted-selector.md) | 0110 |
| 0092 | [P4-B redisplay guard](../adr/0111-browser-p4b-redisplay-guard-wiring.md) | 0111 |
| 0093 | [P4-B conformance evidence/unlock](../adr/0112-browser-p4b-conformance-evidence-unlock.md) | 0112 |
| 0094 | [P3-C control gate/action server](../adr/0113-browser-p3c-control-gate-action-server.md) | 0113 |
| 0094 | [P4-A restore deliver state](../adr/0114-browser-p4a-restore-deliver-state.md) | 0114 |

Phase 1/2 の browser ADR-0078 と ADR-0080 は既存の番号を維持する。main に元からある ADR-0078 の重複は、この統合作業の対象外。main 由来の ADR-0081（web SPA）、ADR-0082（dispatcher 分割）、ADR-0083（source size）、ADR-0089（CoS 並列度）などの参照も維持する。Browser の機密能力や本番設定の扱いは [Phase 4 記録](phase-browser-4.md)と[追跡表](phase-browser-acceptance.md)を参照。
