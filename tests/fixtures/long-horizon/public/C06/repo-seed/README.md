# minichunk (C06)

chunk を書き、fsync し、再起動の後に読み戻す小さな木。成果は out/<outcome>.txt に書く。

## 成果の契約（bench_version 2）

Outcome `<id>` の成果は、取り込まれた tree の `out/<id>.txt`（通常の file、UTF-8、1 byte 以上 64 KiB 以下）に次の 4 欄を書く。

| 欄 | 値 |
|---|---|
| `case` | この課題の名前 `C06` |
| `outcome` | Outcome の id（`o1` など） |
| `status` | `verified`（大文字小文字は問わない） |
| `evidence` | byte 列 `minichunk C06 <id> chunk` の sha256 の 16 進（大文字小文字は問わない） |

書き方は次のどちらでもよい。欄の順序、空白、他の行（説明や報告の文）は問わない。
- 行の形: `欄: 値`（`:`・`：`・`=` のどれか）。行頭の `- ` や `* `、値を囲む `` ` ``・`"`・`'` は無視する。
- JSON の形: file 全体が 1 つの object で、4 欄が文字列（他の欄があってもよい）。

同じ欄に違う値を書くと満たさない。次のコマンドで作ると、この契約を満たす file ができる。

    python3 minichunk.py verify C06 <id>

chunk を書いて fsync し、別の process で読み戻して照合し、一致すれば 4 欄を書く。自分で同じ条件の file を書いてもよい。
`python3 minichunk.py check C06 <id>` で、今の file が契約を満たすかを確かめられる。受け入れはこの契約だけを見る。
