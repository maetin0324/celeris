---
tasks: [01M46W97H391DSFW1XJ745W0G9]
---

# Browser web Live View 実機確認

`scripts/dev/browser-web-live-check.sh` と `docs/ops/browser-web-live-check.md` を追加した。台本は未 opt-in 時に何も起動せず exit 2。opt-in 時は試験専用の launcher socket、使い捨て DB の daemon、loopback 試験ページ、web gateway を起動し、browser task・Live View・owner session・control lease・decision wait を確認して `checks.json` を残す。試験 daemon と launcher は別 UID で動かす。

この worker sandbox では user namespace を作れず、root 所有 launcher 設定・別 UID・subuid・Chrome・実測 conformance 台帳も使えないため、実機実行と証跡採取は配送後に Fable が [運用手順](../../../docs/ops/browser-web-live-check.md) に従って行う。実行時は範囲外 origin への実ブラウザ遷移が拒否された harness 記録も必須とし、API のみの成功を実機合格と数えない。
