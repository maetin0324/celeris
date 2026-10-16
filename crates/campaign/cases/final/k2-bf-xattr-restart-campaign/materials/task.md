# K2: BenchFS の再起動後 size 復元の実装→検証→短い実験→報告→レビュー

初期 commit の BenchFS で、server の SIGKILL＋再起動後に別 node から file size と内容が正しく読めることを確かめ、壊れていれば直す。
実験: UCX × {io_uring, POSIX} の 2 セル、3 host、IOR `-e -R`、write → 再起動 → read（direct chunk と by-ID chunk の両方）。
報告には job ID、実効設定、再起動前後の size、生データの場所を書く。
