//! ADR-0075 §5 G3: WebDAV のサブセットの統合テスト（loopback の port 0。U5 で記録した sccache 0.18 の順序をなぞる）。

mod common;

use std::time::{Duration, Instant};

use common::{request, start};
use scratch_cache::{StoreConfig, TieredStore};

const TOKEN: &str = "test-token";
const AUTH: (&str, &str) = ("Authorization", "Bearer test-token");

fn key(n: u8) -> String {
    format!("{n:02x}{}", "7".repeat(62))
}

fn dav_path(k: &str) -> String {
    format!("/sccache/{}/{}/{}/{k}", &k[0..1], &k[1..2], &k[2..3])
}

fn store(root: &std::path::Path) -> TieredStore {
    std::fs::create_dir_all(root.join("l2")).unwrap();
    let mut c = StoreConfig::new(root.join("l1"), Some(root.join("l2")));
    c.l2_gc_interval = Duration::ZERO;
    TieredStore::open(c).unwrap()
}

fn stats(port: u16) -> serde_json::Value {
    let r = request(port, "GET", "/stats", &[], b"");
    assert_eq!(r.status, 200);
    serde_json::from_slice(&r.body).unwrap()
}

/// U5 で sccache 0.18 が発行した順（storage check → miss → PROPFIND → PUT → hit）が通る。
#[test]
fn sccache_request_sequence_round_trips() {
    let tmp = tempfile::tempdir().unwrap();
    let s = store(tmp.path());
    let srv = start(s.clone(), Some(TOKEN));
    let p = srv.port;

    // storage check。
    assert_eq!(
        request(p, "GET", "/sccache/.sccache_check", &[AUTH], b"").status,
        404
    );
    let r = request(
        p,
        "PROPFIND",
        "/sccache/",
        &[AUTH, ("Depth", "0")],
        b"<propfind/>",
    );
    assert_eq!(r.status, 207);
    let xml = String::from_utf8(r.body).unwrap();
    assert!(
        xml.contains("<D:collection/>") && xml.contains("<D:getlastmodified>"),
        "{xml}"
    );
    assert!(xml.contains(" GMT</D:getlastmodified>"), "{xml}");
    assert_eq!(
        request(
            p,
            "PUT",
            "/sccache/.sccache_check",
            &[AUTH],
            b"Hello, World!"
        )
        .status,
        201
    );
    let r = request(p, "GET", "/sccache/.sccache_check", &[AUTH], b"");
    assert_eq!((r.status, r.body.as_slice()), (200, &b"Hello, World!"[..]));

    // 1 回目のビルド: miss → 親の collection → PUT。
    let k = key(0x98);
    let path = dav_path(&k);
    assert_eq!(request(p, "GET", &path, &[AUTH], b"").status, 404);
    let parent = format!("{}/", path.rsplit_once('/').unwrap().0);
    assert_eq!(
        request(p, "PROPFIND", &parent, &[AUTH, ("Depth", "0")], b"").status,
        207
    );
    assert_eq!(request(p, "MKCOL", &parent, &[AUTH], b"").status, 201);
    let entry = vec![0x50u8; 10_868];
    let started = Instant::now();
    assert_eq!(request(p, "PUT", &path, &[AUTH], &entry).status, 201);
    assert!(started.elapsed() < Duration::from_secs(2));

    // 2 回目のビルド: hit。HEAD と PROPFIND（ファイル）も在ると答える。
    let r = request(p, "GET", &path, &[AUTH], b"");
    assert_eq!((r.status, r.body.len()), (200, entry.len()));
    assert_eq!(r.body, entry);
    let r = request(p, "HEAD", &path, &[AUTH], b"");
    assert_eq!(r.status, 200);
    assert!(
        r.headers
            .to_ascii_lowercase()
            .contains("content-length: 10868"),
        "{}",
        r.headers
    );
    assert_eq!(
        request(p, "PROPFIND", &path, &[AUTH, ("Depth", "0")], b"").status,
        207
    );
    assert_eq!(
        request(
            p,
            "PROPFIND",
            &dav_path(&key(0x11)),
            &[AUTH, ("Depth", "0")],
            b""
        )
        .status,
        404
    );

    let st = stats(p);
    assert_eq!(st["schema"], "celeris.scratch-cache-stats/1");
    // HEAD も GET と同じに数える（sccache は HEAD を送らない。U5）。
    assert_eq!(st["gets"], 3);
    assert_eq!(st["puts"], 1);
    assert_eq!(st["l1_hits"], 2);
    assert_eq!(st["misses"], 1);
    assert_eq!(st["l2_state"], "ok");
    // flusher が L2 へ書く（非同期）。
    let deadline = Instant::now() + Duration::from_secs(60);
    while stats(p)["flush_written"] != 1 {
        assert!(Instant::now() < deadline, "flusher did not write L2");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        tmp.path()
            .join("l2")
            .join(&k[..2])
            .join(format!("{k}.zst"))
            .is_file()
    );
    srv.stop();
    s.shutdown();
}

/// 認証（Bearer）・key の検査・未対応のメソッド。`/healthz` と `/stats` は認証しない。
#[test]
fn auth_and_invalid_requests_are_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let s = store(tmp.path());
    let srv = start(s.clone(), Some(TOKEN));
    let p = srv.port;
    let path = dav_path(&key(1));
    assert_eq!(request(p, "GET", &path, &[], b"").status, 401);
    assert_eq!(
        request(p, "PUT", &path, &[("Authorization", "Bearer wrong")], b"x").status,
        401
    );
    assert_eq!(
        request(p, "GET", "/sccache/a/b/c/not.valid", &[AUTH], b"").status,
        400
    );
    assert_eq!(
        request(p, "PUT", "/sccache/a/b/c/..", &[AUTH], b"x").status,
        400
    );
    assert_eq!(request(p, "DELETE", &path, &[AUTH], b"").status, 405);
    let r = request(p, "GET", "/healthz", &[], b"");
    assert_eq!((r.status, r.body.as_slice()), (200, &b"ok\n"[..]));
    assert_eq!(request(p, "GET", "/stats", &[], b"").status, 200);
    srv.stop();
    s.shutdown();
}

/// L1 を失った（別のマシン・L1 を消した）cache server でも、L2 から引いて L1 へ promote する。
#[test]
fn l2_hit_through_http_is_promoted() {
    let tmp = tempfile::tempdir().unwrap();
    let k = key(0xab);
    let path = dav_path(&k);
    {
        let s = store(tmp.path());
        let srv = start(s.clone(), None);
        assert_eq!(
            request(srv.port, "PUT", &path, &[], b"artifact").status,
            201
        );
        let deadline = Instant::now() + Duration::from_secs(60);
        while s.queue_len() > 0 {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(20));
        }
        srv.stop();
        s.shutdown();
    }
    std::fs::remove_dir_all(tmp.path().join("l1")).unwrap();
    let s = store(tmp.path());
    let srv = start(s.clone(), None);
    let r = request(srv.port, "GET", &path, &[], b"");
    assert_eq!((r.status, r.body.as_slice()), (200, &b"artifact"[..]));
    let st = stats(srv.port);
    assert_eq!(
        (st["l2_hits"].as_u64(), st["promotes"].as_u64()),
        (Some(1), Some(1))
    );
    let r = request(srv.port, "GET", &path, &[], b"");
    assert_eq!(r.status, 200);
    assert_eq!(stats(srv.port)["l1_hits"], 1);
    srv.stop();
    s.shutdown();
}
