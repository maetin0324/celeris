//! ADR-0126 B2/B3: user namespace（unshare/`CLONE_NEWUSER`/newuidmap/実 browser・runtime・launcher）
//! を前提にする試験の既定 skip と環境変数の優先順位を 1 関数に寄せる。
//!
//! 優先順位（上が強い）:
//! 1. `CELERIS_ISOLATION_TESTS=skip` → skip。
//! 2. `CELERIS_USERNS_TESTS=1`、または従来の `CELERIS_ISOLATION_TESTS=require` → 走らせる
//!    （環境が無ければ skip ではなく、呼び出し側の assert/panic で fail させる）。
//! 3. どれも無い → skip（既定）。
#![allow(dead_code)]

pub fn skip_unless_userns_tests() -> bool {
    if std::env::var("CELERIS_ISOLATION_TESTS").as_deref() == Ok("skip") {
        eprintln!("SKIPPED (not passed): CELERIS_ISOLATION_TESTS=skip");
        return true;
    }
    if std::env::var("CELERIS_USERNS_TESTS").as_deref() == Ok("1")
        || std::env::var("CELERIS_ISOLATION_TESTS").as_deref() == Ok("require")
    {
        return false;
    }
    eprintln!("SKIPPED (userns test, not passed): set CELERIS_USERNS_TESTS=1 to run (ADR-0126)");
    true
}
