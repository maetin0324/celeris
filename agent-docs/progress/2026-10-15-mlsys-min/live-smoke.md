---
unit: live-smoke
status: blocked-on-human
---
# live-smoke

## 状況（2026-10-15）
- Jev 実 API 統合: **未検証**（実接続スモークは未実施。キー値は一切扱っていない）。
- 本番の稼働版は 444308ac0b1e。/local/celeris/state/releases に 57d4d30421cf（credential 待ち・Jev adapter を含む版）の release は無い。新しい credential 待ち経路（受信箱・Discord 通知・アカウント画面リンク・登録での自動再開）は本番にまだ無く、実経路を通せない。
- worker は release.sh / verify.sh / 昇格を実行できない（sandbox、本番 host 不変の方針）。

## 人への依頼
1. `scripts/selfdeploy/release.sh 57d4d30421cf && scripts/selfdeploy/verify.sh 57d4d30421cf`（verify.json の ok / live_ok を確認）
2. GUI『リリース』画面で昇格
3. この unit を再実行。Jev の credential が無ければ credential 待ち → 受信箱／Discord 通知のリンクから GUI『アカウント』で登録 → 自動再開 → 分類 1〜数件のスモーク。
