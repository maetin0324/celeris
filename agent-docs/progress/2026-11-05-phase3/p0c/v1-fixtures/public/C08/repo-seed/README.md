# minichunk (C08)

chunk を書き、fsync し、再起動の後に読み戻す小さな木。成果は out/<outcome>.txt に書く。

各 Outcome の成果は、作業 tree の直下で次を実行して作る（<outcome> は o1 などの Outcome id）。

    python3 minichunk.py verify C08 <outcome>

chunk を書いて fsync し、別の process で読み戻して照合する。一致すれば out/<outcome>.txt に検証の 1 行を書く。
隠した受け入れは、取り込まれた tree の out/<outcome>.txt が、このコマンドの書く内容と同じであることを確かめる。
内容を手で書き換えない。
