//! deny list を file で渡す probe・preflight（ADR 2026-10-31 付記: 引数に path を並べない）と、
//! 測定用 claude の固定 copy の sha256 照合（`--claude` / `--claude-sha256`）と、probe の写し元（起動した path が
//! 置き換わっても走っている binary を写す。`probe_stage_`）。
//! 外部ネットワーク・LLM は使わない。bwrap を使う試験は CELERIS_USERNS_TESTS=1 のときだけ走る。
use std::{fs, path::PathBuf};

use celeris_campaign::preflight_isolation::{
    IsolationInputs, ProbeRequest, Wrap, claude_findings, load_deny_list, run,
};
use sha2::{Digest, Sha256};

fn synthetic(dir: &std::path::Path, n: usize) -> Vec<PathBuf> {
    let long = "y".repeat(200);
    (0..n)
        .map(|i| dir.join(format!("absent-{i}-{long}")).join("secret"))
        .collect()
}

fn write_list(dir: &std::path::Path, paths: &[PathBuf]) -> PathBuf {
    let f = dir.join("deny.list");
    let text: String = paths.iter().map(|p| format!("{}\n", p.display())).collect();
    fs::write(&f, text).unwrap();
    f
}

#[test]
fn deny_list_of_15000_paths_loads_in_full() {
    let d = tempfile::tempdir().unwrap();
    let paths = synthetic(d.path(), 15_000);
    let f = write_list(d.path(), &paths);
    assert_eq!(load_deny_list(&f).unwrap(), paths);
}

#[test]
fn claude_copy_sha256_must_match_the_approved_one() {
    let d = tempfile::tempdir().unwrap();
    let copy = d.path().join("claude");
    fs::write(&copy, b"claude-binary").unwrap();
    let good = format!("{:x}", Sha256::digest(b"claude-binary"));
    let iso = |c: Option<&PathBuf>, h: Option<&str>| IsolationInputs {
        claude: c.cloned(),
        claude_sha256: h.map(str::to_string),
        ..Default::default()
    };
    assert!(claude_findings(&iso(Some(&copy), Some(&good))).is_empty());
    assert!(claude_findings(&iso(Some(&copy), Some(&good.to_uppercase()))).is_empty());
    assert!(claude_findings(&iso(None, None)).is_empty());
    let bad = claude_findings(&iso(Some(&copy), Some(&"0".repeat(64))));
    assert!(bad[0].starts_with("deny_readable:claude:") && bad[0].contains("differs"));
    assert_eq!(claude_findings(&iso(Some(&copy), None)).len(), 1);
    assert_eq!(claude_findings(&iso(None, Some(&good))).len(), 1);
    let gone = d.path().join("gone");
    assert_eq!(claude_findings(&iso(Some(&gone), Some(&good))).len(), 1);
}

/// 15,000 件の deny file を bwrap の中の probe-read へ渡しても引数上限に当たらず、全件が拒否と判定される。
#[test]
fn bwrap_probe_runs_with_15000_deny_paths() {
    let setting = std::env::var("CELERIS_USERNS_TESTS").unwrap_or_default();
    if setting != "1" && setting != "require" {
        eprintln!("skip: bwrap deny-file probe requires CELERIS_USERNS_TESTS=1 (or require)");
        return;
    }
    let d = tempfile::tempdir().unwrap();
    let stage = d.path().join("ws");
    fs::create_dir_all(&stage).unwrap();
    let deny = synthetic(d.path(), 15_000);
    let args: Vec<String> = [
        "--unshare-user",
        "--ro-bind",
        "/",
        "/",
        "--dev",
        "/dev",
        "--proc",
        "/proc",
        "--bind",
        stage.to_str().unwrap(),
        stage.to_str().unwrap(),
    ]
    .map(String::from)
    .to_vec();
    let req = ProbeRequest {
        principal: "A",
        program: PathBuf::from(std::env::var("CARGO_BIN_EXE_celeris-campaign").unwrap()),
        wrap: Wrap::Bwrap { args, stage },
        deny,
    };
    let p = run(&req).expect("probe ran without hitting the argv limit");
    assert_eq!(p.paths.len(), 15_000);
    assert!(p.paths.iter().all(|x| !x.readable));
}

// ---- probe の写し元: 起動した path が置き換わっても、走っている binary そのものを stage する ----
// 守る不変条件: `--probe-program` が無いとき、A・B・operator の包みに写す probe は、preflight を走らせている
// process の実行ファイルそのもの。試験中に別の試験（crates/celeris/tests/ops_r1_pipeline.rs の `cargo build -p
// celeris-campaign`）が同じ target の binary を置き換えても、stage は ENOENT にならず、置き換え後の別物も写さない。
// 既存の CLI 試験は binary を置き換えないので、この不具合（統合で 1 回だけ落ちた）を検出できない。

/// sh に書かせて終わりを待つ（この process が書き込み用 fd を持ったまま exec して ETXTBSY になるのを避ける）。
fn sh_write(path: &std::path::Path, contents: &str) {
    use std::io::Write;
    let mut child = std::process::Command::new("sh")
        .args(["-c", r#"cat > "$1" && chmod 755 "$1""#, "sh"])
        .arg(path)
        .stdin(std::process::Stdio::piped())
        .spawn()
        .expect("spawn writer");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(contents.as_bytes())
        .expect("write");
    assert!(child.wait().expect("wait").success());
}

/// `--sandbox-script` と PATH 先頭の `bwrap` を兼ねる包み。`--print-args` には許可表を 1 組返す。
/// 実行では `--` の後の staged probe の sha256 を `<rec>/staged-<principal>.sha256` に書き、probe をそのまま起動する。
fn stage_recorder(rec: &std::path::Path) -> String {
    format!(
        r#"#!/bin/sh
for a; do [ "$a" = --print-args ] && {{ printf '%s\n' --ro-bind /usr /usr; exit 0; }}; done
while [ $# -gt 0 ] && [ "$1" != -- ]; do shift; done
[ $# -gt 0 ] || exit 0
shift
if [ "$2" = probe-read ]; then sha256sum "$1" | cut -d' ' -f1 > '{}/staged-'"$4"'.sha256'; fi
exec "$@"
"#,
        rec.display()
    )
}

/// 走っている `celeris-campaign` の起動 path を、cargo と同じく消して別の file を置いた後に preflight が probe を stage する。
/// 子は `--deny-list` の FIFO を読んでいる間（`requests` が写し元を決める前）に止まるので、置き換えの順序は時間や負荷に依らない。
#[test]
fn probe_stage_copies_the_running_binary_after_its_path_is_replaced() {
    use std::{
        io::Write,
        process::{Command, Output, Stdio},
        sync::mpsc,
        thread,
        time::Duration,
    };
    /// 出来事待ちの保険（決定的な順序で進むので、通常はすぐ終わる）。
    const LONG: Duration = Duration::from_secs(300);
    let d = tempfile::tempdir().unwrap();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let original = PathBuf::from(std::env::var("CARGO_BIN_EXE_celeris-campaign").unwrap());
    let want = format!("{:x}", Sha256::digest(fs::read(&original).unwrap()));

    // 起動に使う path（cargo の target/debug/celeris-campaign の代わり）。cp に書かせて ETXTBSY を避ける。
    let bin_dir = d.path().join("target-debug");
    fs::create_dir_all(&bin_dir).unwrap();
    let bin = bin_dir.join("celeris-campaign");
    assert!(
        Command::new("cp")
            .arg(&original)
            .arg(&bin)
            .status()
            .unwrap()
            .success()
    );

    let rec = d.path().join("rec");
    let fakebin = d.path().join("fakebin");
    fs::create_dir_all(&rec).unwrap();
    fs::create_dir_all(&fakebin).unwrap();
    let wrap = fakebin.join("bwrap");
    sh_write(&wrap, &stage_recorder(&rec));
    // A の実行ファイルの代わり（host の claude を起動しない）。
    let a_program = fakebin.join("fixture-a");
    sh_write(&a_program, "#!/bin/sh\necho 'fxa 2.1.0'\n");

    let iso = d.path().join("iso");
    let mut args: Vec<String> = vec!["preflight".into(), "--manifest".into()];
    args.push(
        root.join("fixtures/frozen-manifests/ops-r2-k2.json")
            .display()
            .to_string(),
    );
    args.extend([
        "--cases-root".into(),
        root.join("cases").display().to_string(),
    ]);
    args.extend(["--a-program".into(), a_program.display().to_string()]);
    let fifo = iso.join("deny.fifo");
    fs::create_dir_all(&iso).unwrap();
    assert!(
        Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    args.extend(["--deny-list".into(), fifo.display().to_string()]);
    args.extend(["--sandbox-script".into(), wrap.display().to_string()]);
    for p in ["b", "operator"] {
        for (k, dir) in [
            ("state", format!("{p}-state")),
            ("release", "release".to_string()),
            ("account", format!("{p}-account")),
        ] {
            let dir = iso.join(dir);
            fs::create_dir_all(&dir).unwrap();
            args.extend([format!("--{p}-{k}"), dir.display().to_string()]);
        }
    }
    let path = format!(
        "{}:{}",
        fakebin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let child = Command::new(&bin)
        .args(&args)
        .env("PATH", path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn celeris-campaign");
    let pid = child.id();

    enum Ev {
        Opened(fs::File),
        Exited(Output),
    }
    let (tx, rx) = mpsc::channel();
    {
        let tx = tx.clone();
        thread::spawn(move || {
            let _ = tx.send(Ev::Exited(child.wait_with_output().expect("wait")));
        });
    }
    {
        let fifo = fifo.clone();
        thread::spawn(move || {
            // 子が FIFO を読みに開くまで、書き手の open は返らない。
            if let Ok(f) = fs::OpenOptions::new().write(true).open(&fifo) {
                let _ = tx.send(Ev::Opened(f));
            }
        });
    }
    let text = |o: &Output| {
        format!(
            "{}{}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        )
    };
    let mut writer = match rx.recv_timeout(LONG) {
        Ok(Ev::Opened(f)) => f,
        Ok(Ev::Exited(o)) => panic!(
            "preflight exited before reading the deny list: {}",
            text(&o)
        ),
        Err(e) => panic!("preflight did not open the deny list: {e}"),
    };

    // 子は deny list を読み終えていない（写し元はまだ決まっていない）。cargo と同じく path を消して別の file を置く。
    fs::remove_file(&bin).unwrap();
    sh_write(&bin, "#!/bin/sh\nexit 97\n");
    let exe = fs::read_link(format!("/proc/{pid}/exe")).unwrap();
    assert!(
        exe.to_string_lossy().ends_with(" (deleted)"),
        "the running binary's path must be gone: {}",
        exe.display()
    );

    let deny = [
        iso.join("old-out/answer.json"),
        iso.join("absent/private.json"),
    ];
    for p in &deny {
        writeln!(writer, "{}", p.display()).unwrap();
    }
    drop(writer);
    let out = match rx.recv_timeout(LONG) {
        Ok(Ev::Exited(o)) => o,
        Ok(Ev::Opened(_)) => unreachable!("the deny list is opened once"),
        Err(e) => panic!("preflight did not finish: {e}"),
    };
    let t = text(&out);
    assert!(!t.contains("stage probe"), "{t}");
    for principal in ["A", "B", "operator"] {
        let got = fs::read_to_string(rec.join(format!("staged-{principal}.sha256")))
            .unwrap_or_else(|e| panic!("{principal}: the probe was not staged ({e}): {t}"));
        assert_eq!(
            got.trim(),
            want,
            "{principal}: the staged probe must be the running binary, not the file now at its path"
        );
    }
}
