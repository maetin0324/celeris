//! ADR-0075 D5 (b)（Phase G3）: sccache の webdav backend に対する Celeris の階層 cache。
//!
//! - **L1** = ローカルのディレクトリ（`<scratch>/cache-l1/<k0k1>/<key>`。PUT されたバイト列そのまま）。
//! - **L2** = NFS 上の content-addressed な immutable object（`<l2>/<k0k1>/<key>.zst`。PUT されたバイト列を
//!   zstd〈content checksum 付き〉で包んだもの）。
//! - GET は L1 → L2 → miss。L2 hit は L1 へ promote する。PUT は L1 に書いてすぐ返し、L2 へは flusher が
//!   token bucket の帯域で非同期に書く（tmp → write → fsync → rename）。
//! - L2 の I/O は上限付きのスレッドとタイムアウトに閉じ込め、連続失敗で切り離す（degraded）。NFS が止まっても
//!   L1 だけで動く。
//! - L2 の GC は `l2_max_bytes` を超えた分を mtime の古い順に消す（hit で mtime を 1 日 1 回まで touch）。
//!
//! HTTP（`server`）は sccache 0.18 が実際に発行するメソッド（U5 の実測: GET / PUT / PROPFIND Depth 0 / MKCOL）
//! と HEAD だけを実装する。

pub mod bucket;
pub mod clock;
pub mod key;
pub mod l2;
pub mod server;
pub mod store;

pub use bucket::TokenBucket;
pub use clock::{Clock, SystemClock};
pub use l2::{DirL2, L2Backend, L2Object};
pub use store::{FlushOutcome, GcReport, Lookup, StoreConfig, TieredStore};

/// `time` の RFC 3339（task-worker の `scratch::rfc3339` と同じ形）。
pub(crate) fn rfc3339(t: std::time::SystemTime) -> String {
    time::OffsetDateTime::from(t)
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests;
