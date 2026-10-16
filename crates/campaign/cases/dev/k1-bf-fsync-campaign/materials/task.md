# K1: BenchFS の fsync 経路の実装→検証→短い実験→報告→レビュー

初期 commit の BenchFS で、書いた data が fsync 後に別 node から読め、server の SIGKILL＋再起動後にも読めることを確かめ、
壊れていれば直す。受入は既存の `cargo test --release --lib` が全て通ることと、固有の再現試験を足して通すこと。
実験: transport {UCX, Locusta} × storage {io_uring, POSIX} の 4 セルを 3 host（server・writer・別 host の reader）で回す。
IOR は fsync（`-e`）と checkRead（`-R`）を有効にし、write → 別 node read → server SIGKILL＋再起動 → read の順に測る。
報告には job ID、各セルの実効設定（fsync・checkRead）、生データの場所を書く。実効値が 0 なら有効と書いてはならない。
