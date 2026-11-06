---
tasks: [01M6QW0GSJR1X7797YF4Y1HWR8]
---
# verify の DB コピーで読み取り時点を固定する

## 背景
N2 の delivery prepare は release 作成後、verify の SQLite `.backup` で約900秒を費やして終了した。
prepare の verify 上限900秒と一致する。コピーが進まなかった直接の原因は未確認だが、
更新が続く DB のオンライン backup は読み取り時点を固定しなければコピーをやり直し得る。

## 決定
verify 専用の Python helper で、mode=ro の接続に BEGIN と sqlite_schema の読み取りを行ってから
SQLite backup API を呼ぶ。WAL の読み取り snapshot を固定し、更新でコピーをやり直す経路をなくす。
書き込み先は staging のみ。終了時に接続と transaction を閉じる。verify 全体の900秒上限は維持する。
一時 WAL DB のコピー中に別接続で更新する試験で、コピーが固定時点の内容を保持することを確かめる。
本番 DB のコピーや release の再実行はこの task では行わない。

## 制約
読み取り transaction の間は WAL の回収が遅れる。上限時間を延長しない。
今回の本番コピーの遅延原因がこれだけだったか、修正後の本番 verify が完走するかは未確認。
