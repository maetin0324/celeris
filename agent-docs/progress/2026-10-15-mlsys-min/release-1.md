---
unit: release-1
status: blocked-on-human
---
# release-1

worker の sandbox では release.sh / verify.sh を実行できない（`/local/celeris/state/releases` が読み取り専用）。運用セッションが実行する。

## 今回の作業
- release.sh 96d19db0b517 は gate の cargo-fmt-check で失敗していた。`cargo fmt --all` は HEAD 57d4d304（"style: cargo fmt --all"）で適用済みで、`cargo fmt --all -- --check` は exit 0、差分なし。
- 運用セッションが release・verify を実行する対象 sha は 57d4d30421cf（12 桁 `57d4d30421cf`）。

## 運用セッションの手順（人）
```
scripts/selfdeploy/release.sh 57d4d30421cf
scripts/selfdeploy/verify.sh 57d4d30421cf    # verify.json の ok と live_ok が真か確認
scripts/selfdeploy/status.sh
```
昇格は人が GUI の『リリース』画面で行う（worker は promote しない）。

## migration 互換
- SCHEMA_VERSION = 82。0081_credential_waits と 0082_decision_log は追加のみで、空 DB と既存 DB の両方に順に適用される（試験あり）。
- 既存の表・列は消さないので、旧版 daemon が走る間も DB は読める。

## rollback 手順
- `scripts/selfdeploy/rollback.sh` で previous を昇格し直す（人のみ）。
- 昇格後に DB を戻す場合だけ `rollback.sh --restore-db`（昇格後の仕事は失われる。通常は不要）。
- 詳細は docs/ops/selfdeploy.md §5。

## 昇格後の API キー登録手順（人）
1. GUI の『アカウント』画面を開く。
2. 対象 source（Jev 等）の秘密登録でキーを入力して保存する（値は prompt・ログ・git に出さない）。
3. 認証検証が通ると、credential 待ちの work unit だけが一度だけ自動再開する。受信箱の通知は消える。
4. その後 live-smoke を実行する。

## 未確認
- verify.json の ok / live_ok（運用セッションの実行結果待ち）。
