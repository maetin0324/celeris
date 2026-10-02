# cargo-nextest の導入と版の固定（Phase SD-2、ADR-0041 §8）

release.sh の gate の `cargo-test` 段は `scripts/dev/test-parallel.sh` でテストバイナリを**並列に**回す
（`cargo nextest run --workspace` + `cargo test --doc --workspace`）。そのために release.sh を動かすホストに
`cargo-nextest` が要る。**版は `tools/nextest/VERSION`（1 行）で固定**し、違う版が入っていると test-parallel.sh は止まる。

開発者は従来どおり `cargo test --workspace` でよい（CLAUDE.md のとおり。gate の前提として両方とも通ることを保つ）。
並列で回したいときは `scripts/dev/test-parallel.sh`（引数はそのまま `cargo nextest run` に渡る。例: `-p task-core`）。

## 入れる（人・エージェントが 1 回だけ。ネットワークに出る）

```sh
# 版は tools/nextest/VERSION と同じにする
cargo install cargo-nextest --locked --version "$(cat tools/nextest/VERSION)"
# → ~/.cargo/bin/cargo-nextest（cargo のサブコマンドとして `cargo nextest` で呼ばれる）
cargo nextest --version    # → cargo-nextest 0.9.146 (...)
```

- crates.io からの取得が遅く数十分かかることがある（途中で切れたら、取得済みの分を使ってもう一度回せば入る）。
  `/tmp` で cargo を回さない規則に合わせ、`--target-dir` と `TMPDIR` を scratch に向けてよい:
  `TMPDIR=/var/lib/celeris/scratch/targets/<owner>/tmp cargo install cargo-nextest --locked --version "$(cat tools/nextest/VERSION)" --target-dir /var/lib/celeris/scratch/targets/<owner>/install-target`
  （終わったら `<owner>` ごと消す）。
- バイナリをリポジトリに置かない（vendor しない）。GitHub の配布バイナリも使わない（照合する sha256 を持たない。sccache と同じ方針。
  [sccache-l1.md](sccache-l1.md)）。
- `cargo test` の一部ではない（ネットワークに出る。ADR-0009 P-34）。

## 版を上げる

1. `cargo install cargo-nextest --locked --version <new> --force`
2. `tools/nextest/VERSION` を `<new>` にして、`scripts/dev/test-parallel.sh` を 3 回通す（新しい版の既定の変化で直列の
   前提が崩れていないか）。
3. commit。release.sh は**ビルドする sha の**`.config/nextest.toml` と、release.sh の隣の `scripts/dev/test-parallel.sh` と
   `tools/nextest/VERSION` を使う。

## 無いとき・壊れたとき

- release.sh は作業ツリーを作る前に `cargo nextest --version` を確かめ、無ければ
  `cargo-nextest is not installed; install once: cargo install cargo-nextest --locked --version <VERSION> (docs/ops/nextest.md), or set SD_GATE_TEST_RUNNER=cargo-test` で落ちる（exit 1）。
- 版が違う: test-parallel.sh が `cargo-nextest X is installed but tools/nextest/VERSION pins Y` で落ちる（gate の `cargo-test` 段）。
  一時的に許すなら `CELERIS_NEXTEST_ANY_VERSION=1`。
- 非常用: `SD_GATE_TEST_RUNNER=cargo-test scripts/selfdeploy/release.sh <ref>` で従来の直列の `cargo test --workspace` に戻る
  （gate.json の `cargo_test.runner` が `cargo-test` になる）。

## 並列数

`CELERIS_TEST_JOBS`（既定 `min(8, max(2, nproc/3))`。このホストは 24 CPU → 8）。daemon の run（cargo のビルド・LLM の
worker）が同居するので、nproc いっぱいにはしない。時間に敏感なテストが高負荷で落ちるときは下げる。

## 直列が要るテスト

`.config/nextest.toml` の `[test-groups]` と `[[profile.default.overrides]]` で縛る（テストを消さない）。
何をなぜ縛ったかは [ADR-0041](../../agent-docs/adr/0041-self-improvement-loop-hardening.md) §8。nextest はテストごとに別プロセスで回すので、同じバイナリの中の `static Mutex` による直列化は
nextest の下では効かない（その代わりプロセスが別なので、プロセス全体の状態〈env・waitpid(-1)・シグナル〉は互いに干渉しない）。
