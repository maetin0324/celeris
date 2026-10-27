# 通常の task 作成と retry の冪等キー

- 日付: 2026-10-27
- 状態: 採用
- Task: 01M5XXF7QRGY65F3WTKSKVAZVK

## 決定

`POST /tasks` と `POST /tasks/{id}/retry` は任意の `idempotency_key` を受ける。
空白だけのキーと 200 文字を超えるキーは 400。キー無しは毎回作成する。
キーは主体によらない共通の名前空間とし、method・正規化した path・キーを除く JSON 本文の
SHA-256 を保存する。JSON object の順序は無視するが、省略と明示した既定値は区別する。
同じキー・hash の再送は既存の task（retry は保存した応答）を 200 で返し、異なる要求は 409。
同時再送も task を増やさず、transaction の競合後に保存済みの結果を照合する。

SQLite の task_idempotency_keys に保存し、task の作成・retry の複製と同じ transaction で記録する。
再起動で失わず、期限による削除は行わない。retry の hash は元 task の ID を含む。
CoS operations は既存の操作単位の冪等キーを使い、本文へのキーの二重指定は拒否する。

campaign 専用の API 欄と label によるキー検索は削除し、campaign client も一般のキーへ移す。
既存の campaign 試行識別による重複防止・組織 pin 検証は保持する。既存試行を返す場合も、
新たに渡されたキーと要求 hash をその task に結び付ける。
既存 task・campaign 識別・labels・events は保持する。過去の本文 hash は復元不能なので捏造しない。

## 検証

API の再送・本文変更・別対象・同時要求・失敗後の再送と store の永続化を試験する。
生成 schema と web 型も更新する。
