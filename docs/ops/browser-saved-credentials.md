---
tasks: [01M4KG0J2XVR99NYH0FA01G9M0]
---
# 保存済みログイン情報の再利用と削除

credentiald・daemon・web をこの変更を含む同じ release に更新する。launcher protocol の変更はない。更新と設定変更は運用セッションで人が行う。

## 使用する

初回は従来どおり ID・パスワードを本人確認済みの入力画面から登録する。次の run で agent が同じ site policy の credential を要求すると、再入力画面を経ず「credential_use」の使用承認が開く。login URL、ID 欄と password 欄の selector、ログイン後の read_origins、consent のボタンを毎回確認して承認する。保存済みでも自動承認はしない。

この instance の人 `owner` が署名して登録した情報だけを自動再利用する。task をまたいでも同じ owner、policy_id、登録時の TrustedLogin 全体が現在の site policy と一致する場合だけ使う。worker・agent が owner を選ぶ口はない。別の actor_id で登録した情報と、owner を持たない旧 vault の情報は自動再利用されない。旧登録を使っていた task は更新後に一度再入力する。

## 保存期間を設定する

credentiald の環境変数 `CELERIS_CREDENTIAL_MAX_AGE_DAYS` で、新規登録の保存期間を **1〜90 日** に設定できる。既定および範囲外・非整数の値は90日。期限は登録時刻から計算し、再利用しても延長しない。設定を変えても既存登録の期限は変わらない。既存情報を早く使えなくするには削除する。

例えば30日にする場合、人が対象 instance の credentiald の service override に以下を設定し、credentiald を再起動する（稼働 browser session がない時に行う）。

```ini
[Service]
Environment=CELERIS_CREDENTIAL_MAX_AGE_DAYS=30
```

対象 unit は `celeris-credentiald@<instance>.service`。人が `systemctl --user edit celeris-credentiald@<instance>.service` と `systemctl --user restart celeris-credentiald@<instance>.service` を実行する。新しく登録した項目の登録日・期限を設定画面で確認する。パスワードをコマンド・環境変数・ログへ書かない。

期限切れの情報は検索・describe・使用を拒否する。一覧では期限切れも残り、削除できる。暗号化ファイルの自動掃除は行わない。

## 一覧と削除

web の `/browser/settings` を開いて本人確認を済ませ、「保存済みログイン情報」の policy ID・登録日・期限を確認する。「削除」から対象を確認して削除する。ID・パスワード・ciphertext は一覧に表示しない。

削除は credentiald vault のファイルを消す。次の要求は再入力になり、削除前に開いた承認も使用直前の再照合で再入力になる。既に認証した browser session のログアウトまでは行わない。必要ならその session も終了する。

API は `GET /api/v1/browser/credentials` と `DELETE /api/v1/browser/credentials/{id}`。どちらも daemon の admin bearer だけでは使えず、owner session の署名（一覧/削除用途、actor、削除対象、30秒以内の期限）を検証する。web gateway は削除に CSRF 検証も行う。

## 再入力に戻る場合

- site policy の login URL・selector・read_origins・consent 等が変わった。
- credentiald が describe できない、登録が削除された、期限を過ぎた。
- 認証失敗、または `post_login_idp_login_form` 等でログイン後の読み取りを再開できない。

認証失敗時は保存を削除し、browser session を終了して再入力を開く。同じ情報で自動再試行しない。削除・session 終了を確認できない場合はエラーで止める。credentiald の接続と session の終了を運用側で確認する。

確認用: 初回登録と承認→次の task/run の要求で承認だけになる→未承認では使用されない→削除後の要求で再入力になる、の順で確認する。入力値をスクリーンショット・run log・成果物へ残さない。
