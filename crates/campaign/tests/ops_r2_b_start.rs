//! ops-r2 の B の daemon 起動（ADR 2026-10-31-ops-r2-preflight-fixes 付記 D-bwork・D-bstart）。
//! 手順書（docs/ops/ops-r2-k2-preflight.md §2.2・§6・§6.1・§6.2）の順に、実物の `ops-r1-measure-config.sh`（stub harness）・
//! `ops-r2-seed.sh make|place`・`ops-r2-trial-a-command.sh`・`ops-r2-b-daemon-command.sh` を走らせ、出た 1 行を sh で評価する。
//! daemon と claude は偽物で、起動時の argv と作業ツリーの一覧を記録するだけなので、展開が起動の前に済んでいること、
//! A と B の木が同じこと、B の案件の body（setup/project.json）の workspace がその木を指すことを決定的に確かめられる。
//! sandbox は既定で argv を記録する偽物。実 bwrap（`ops-r2-sandbox.sh`）で B を包む試験は CELERIS_USERNS_TESTS=1 のときだけ走る。
//! 実 sirius・LLM・外部ネットワークには出ない（socket は試験の中で開く unix socket）。
use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output},
};

const SID: &str = "0b9e4c1a-2f3d-4e5f-8a6b-7c8d9e0f1a2b";
const SHA12: &str = "0123456789ab";
const SUB_COMMIT: &str = "9ecd51ee9b5fe62831f8e46c3d98a371359664c0";

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn campaign_script(name: &str) -> PathBuf {
    repo_root().join("scripts/campaign").join(name)
}

fn write_exec(path: &Path, body: &str) {
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn ok(o: &Output, what: &str) {
    assert!(
        o.status.success(),
        "{what}: exit {:?}\n{}\n{}",
        o.status.code(),
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
}

fn sha256(p: &Path) -> String {
    String::from_utf8(Command::new("sha256sum").arg(p).output().unwrap().stdout).unwrap()[..64]
        .to_string()
}

fn git(dir: &Path, args: &[&str]) {
    let s = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
        .args(args)
        .status()
        .unwrap();
    assert!(s.success(), "git {args:?}");
}

/// `dir` の下の全 entry の相対 path → 中身（dir は `<dir>`、symlink は `-> 先`）。
fn tree(dir: &Path) -> BTreeMap<String, String> {
    fn walk(root: &Path, d: &Path, out: &mut BTreeMap<String, String>) {
        for e in fs::read_dir(d).unwrap() {
            let p = e.unwrap().path();
            let rel = p.strip_prefix(root).unwrap().display().to_string();
            let md = fs::symlink_metadata(&p).unwrap();
            if md.file_type().is_symlink() {
                out.insert(rel, format!("-> {}", fs::read_link(&p).unwrap().display()));
            } else if md.is_dir() {
                out.insert(rel, "<dir>".into());
                walk(root, &p, out);
            } else {
                out.insert(rel, String::from_utf8_lossy(&fs::read(&p).unwrap()).into());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

/// 手順書の §2.2・§6.1 まで済んだ A・B の state と、起動 command に渡す path。
struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    /// 2 つの command script と sandbox（偽物か実物）を置いた dir。
    scripts: PathBuf,
    s_a: PathBuf,
    s_b: PathBuf,
    release: PathBuf,
    cdir: PathBuf,
    acct_a: PathBuf,
    acct_b: PathBuf,
    cfg: PathBuf,
    sock: PathBuf,
    _listener: std::os::unix::net::UnixListener,
    seed_sha: String,
}

/// 種を `make` で作る（公開の初期 tar ＋ submodule の tar 1 件。ops_r2_seed.rs と同じ形）。
fn make_seed(root: &Path) -> (PathBuf, String) {
    let repo = root.join("upstream");
    fs::create_dir_all(repo.join("src")).unwrap();
    fs::create_dir_all(repo.join("lib/pluvio")).unwrap();
    fs::write(repo.join("src/lib.c"), "int x;\n").unwrap();
    fs::write(repo.join("lib/pluvio/.placeholder"), "").unwrap();
    git(&repo, &["init", "-q"]);
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-qm", "seed"]);
    let archive = root.join("initial.tar");
    git(
        &repo,
        &[
            "archive",
            "--format=tar",
            "-o",
            archive.to_str().unwrap(),
            "HEAD",
        ],
    );
    // 作業ツリーに .git を残さない（sandbox は祖先・子孫の .git を拒む）。
    fs::remove_dir_all(&repo).unwrap();
    let assets = root.join("assets");
    let sub_src = root.join("sub-src");
    fs::create_dir_all(assets.join("submodules")).unwrap();
    fs::create_dir_all(&sub_src).unwrap();
    fs::write(sub_src.join("Cargo.toml"), "[package]\n").unwrap();
    let sub_tar = assets.join(format!("submodules/pluvio-{}.tar", &SUB_COMMIT[..8]));
    let s = Command::new("tar")
        .arg("-cf")
        .arg(&sub_tar)
        .arg("-C")
        .arg(&sub_src)
        .arg("Cargo.toml")
        .status()
        .unwrap();
    assert!(s.success());
    fs::write(
        assets.join("SHA256SUMS"),
        format!(
            "{}  submodules/pluvio-{}.tar\n",
            sha256(&sub_tar),
            &SUB_COMMIT[..8]
        ),
    )
    .unwrap();
    let case = root.join("case.json");
    fs::write(
        &case,
        format!(
            r#"{{"initial":{{"archive_sha256":"{}","submodules":["lib/pluvio@{SUB_COMMIT}"]}}}}"#,
            sha256(&archive)
        ),
    )
    .unwrap();
    let tar = root.join("benchfs.tar");
    let o = Command::new("sh")
        .arg(campaign_script("ops-r2-seed.sh"))
        .arg("make")
        .arg("--archive")
        .arg(&archive)
        .arg("--case")
        .arg(&case)
        .arg("--assets")
        .arg(&assets)
        .arg("--out")
        .arg(&tar)
        .output()
        .unwrap();
    ok(&o, "ops-r2-seed.sh make");
    (tar, String::from_utf8_lossy(&o.stdout).trim().to_string())
}

/// cos-inbox-triage の skill の版（measure-config の --policy-version と同じ値を渡す）。
fn skill_version() -> String {
    let text =
        fs::read_to_string(repo_root().join("config/skills/cos-inbox-triage/SKILL.md")).unwrap();
    text.lines()
        .find_map(|l| l.trim().strip_prefix("version:"))
        .expect("version in front matter")
        .trim()
        .trim_matches('"')
        .to_string()
}

impl Fixture {
    /// path には空白と `'` を含める（実 bwrap の試験も同じ path で通す）。
    /// `real_sandbox` なら command script の隣に実物の `ops-r2-sandbox.sh` を置く。
    fn new(real_sandbox: bool) -> Self {
        // ops-r2-sandbox.sh は workspaces・state の下の path を拒むので、cargo の target の一時 dir を使う。
        let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
        let root = dir.path().canonicalize().unwrap().join("it's a dir");
        fs::create_dir_all(&root).unwrap();
        let scripts = root.join("scripts");
        fs::create_dir_all(&scripts).unwrap();
        for name in ["ops-r2-trial-a-command.sh", "ops-r2-b-daemon-command.sh"] {
            fs::copy(campaign_script(name), scripts.join(name)).unwrap();
        }
        if real_sandbox {
            fs::copy(
                campaign_script("ops-r2-sandbox.sh"),
                scripts.join("ops-r2-sandbox.sh"),
            )
            .unwrap();
        } else {
            // 偽の sandbox: argv を <state>/../sandbox-argv-<state の名前>.txt に 1 行ずつ記録し、`--` の後ろを実行する。
            write_exec(
                &scripts.join("ops-r2-sandbox.sh"),
                "#!/bin/sh\nst=$2\nfor a in \"$@\"; do printf '%s\\n' \"$a\"; done > \"$(dirname \"$st\")/sandbox-argv-$(basename \"$st\").txt\"\n\
                 while [ \"$1\" != -- ]; do shift; done; shift; exec \"$@\"\n",
            );
        }
        let (s_a, s_b) = (root.join("a-state"), root.join("b-state"));
        let release = root.join("release");
        let cdir = root.join("claude-bin");
        let (acct_a, acct_b) = (root.join("acct-a"), root.join("acct-b"));
        let ssh = root.join("ssh");
        // unix socket の path は 108 byte 未満。ssh master と同じく `@`・`:` を含む短い名前を tempdir の直下に置く。
        let mux = dir.path().canonicalize().unwrap();
        for d in [&release.join("bin"), &cdir, &acct_a, &acct_b, &ssh, &mux] {
            fs::create_dir_all(d).unwrap();
        }
        // 偽の celeris: argv・cwd と、起動した時点の作業ツリーの一覧を state に記録して終わる（daemon は起動しない）。
        write_exec(
            &release.join("bin/celeris"),
            "#!/bin/sh\nst=$(dirname \"$2\")\nfor a in \"$@\"; do printf '%s\\n' \"$a\"; done > \"$st/celeris-argv.txt\"\n\
             printf '%s\\n' \"$PWD\" > \"$st/celeris-cwd.txt\"\n\
             (cd \"$st/work/benchfs\" && find . | LC_ALL=C sort) > \"$st/celeris-tree.txt\"\n",
        );
        fs::write(
            release.join("manifest.json"),
            format!(r#"{{"sha12": "{SHA12}"}}"#),
        )
        .unwrap();
        // 偽の claude: 起動した時点の作業ツリーの一覧を transcript（stdout）に出す。
        write_exec(
            &cdir.join("claude-2.1.303"),
            "#!/bin/sh\nfind . | LC_ALL=C sort\n",
        );
        fs::write(acct_b.join(".credentials.json"), "{}").unwrap();
        let cfg = ssh.join("config");
        fs::write(&cfg, "Host sirius\n").unwrap();
        let sock = mux.join("mux-r@h:22");
        let listener = std::os::unix::net::UnixListener::bind(&sock).unwrap();

        // §2.2: B の測定 daemon の config（stub harness。port は指定して bind しない）。
        let stub = root.join("stub-harness");
        write_exec(&stub, "#!/bin/sh\nexit 0\n");
        let o = Command::new("sh")
            .arg(campaign_script("ops-r1-measure-config.sh"))
            .arg("--state")
            .arg(&s_b)
            .arg("--release-dir")
            .arg(&release)
            .args(["--models", "all-sonnet", "--policy-version"])
            .arg(skill_version())
            .arg("--out")
            .arg(s_b.join("daemon.toml"))
            .args(["--port", "17999", "--harness", "stub", "--stub-command"])
            .arg(&stub)
            .output()
            .unwrap();
        ok(&o, "ops-r1-measure-config.sh");

        // §6.1: 同じ種を A・B に置き、包みの中で使う seed script を写す。
        let (tar, seed_sha) = make_seed(&root.join("seed-src"));
        for state in [&s_a, &s_b] {
            let o = Command::new("sh")
                .arg(campaign_script("ops-r2-seed.sh"))
                .arg("place")
                .arg("--seed")
                .arg(&tar)
                .arg("--sha256")
                .arg(&seed_sha)
                .arg("--state")
                .arg(state)
                .output()
                .unwrap();
            ok(&o, "ops-r2-seed.sh place");
            fs::create_dir_all(state.join("bin")).unwrap();
            fs::copy(
                campaign_script("ops-r2-seed.sh"),
                state.join("bin/ops-r2-seed.sh"),
            )
            .unwrap();
        }
        fs::write(s_a.join("prompt.md"), "the prompt").unwrap();
        Fixture {
            _dir: dir,
            root,
            scripts,
            s_a,
            s_b,
            release,
            cdir,
            acct_a,
            acct_b,
            cfg,
            sock,
            _listener: listener,
            seed_sha,
        }
    }

    fn b_command(&self, overrides: &[(&str, &str)]) -> Output {
        let mut args: Vec<(String, String)> = [
            ("--state", &self.s_b),
            ("--release", &self.release),
            ("--claude-dir", &self.cdir),
            ("--account", &self.acct_b),
            ("--ssh-config", &self.cfg),
            ("--socket", &self.sock),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.display().to_string()))
        .collect();
        args.push(("--seed-sha256".into(), self.seed_sha.clone()));
        for (k, v) in overrides {
            args.iter_mut().find(|(a, _)| a == k).unwrap().1 = v.to_string();
        }
        let mut c = Command::new("sh");
        c.arg(self.scripts.join("ops-r2-b-daemon-command.sh"));
        for (k, v) in args {
            c.arg(k).arg(v);
        }
        c.output().unwrap()
    }

    fn a_command(&self) -> Output {
        Command::new("sh")
            .arg(self.scripts.join("ops-r2-trial-a-command.sh"))
            .arg("--state")
            .arg(&self.s_a)
            .arg("--claude-dir")
            .arg(&self.cdir)
            .arg("--account")
            .arg(&self.acct_a)
            .arg("--ssh-config")
            .arg(&self.cfg)
            .arg("--socket")
            .arg(&self.sock)
            .args(["--seed-sha256", &self.seed_sha, "--session-id", SID])
            .output()
            .unwrap()
    }

    /// 出した 1 行を、人がするように sh で評価する。
    fn eval(&self, line: &str) -> Output {
        Command::new("sh")
            .args(["-c", line])
            .current_dir(&self.root)
            .output()
            .unwrap()
    }
}

fn one_line(o: &Output) -> String {
    ok(o, "command script");
    let line = String::from_utf8(o.stdout.clone()).unwrap();
    assert_eq!(line.lines().count(), 1, "one line: {line}");
    assert!(!line.contains(" -F "), "trial must not be told -F: {line}");
    line
}

fn lines(p: &Path) -> Vec<String> {
    fs::read_to_string(p)
        .unwrap_or_else(|e| panic!("{}: {e}", p.display()))
        .lines()
        .map(str::to_owned)
        .collect()
}

#[test]
fn b_daemon_starts_after_the_seed_is_unpacked_into_the_project_repo_and_matches_a() {
    let f = Fixture::new(false);
    let d = |p: &Path| p.display().to_string();
    let work_b = f.s_b.join("work/benchfs");

    // B の案件の body: 作業場所は B の state の作業ツリー（付記 D-bwork）。
    let body: serde_json::Value =
        serde_json::from_slice(&fs::read(f.s_b.join("setup/project.json")).unwrap()).unwrap();
    assert_eq!(
        body["workspace"],
        serde_json::json!({"kind": "local", "path": d(&work_b)})
    );

    let b_line = one_line(&f.b_command(&[]));
    assert!(!work_b.exists(), "the script itself must not unpack");
    let ran = f.eval(&b_line);
    ok(&ran, "B command");

    // sandbox の引数: B の daemon の形（--ro claude・--ssh-config・--socket）で、command は sh -c。
    let sandbox = lines(&f.root.join("sandbox-argv-b-state.txt"));
    assert_eq!(
        sandbox[..15],
        [
            "--state".into(),
            d(&f.s_b),
            "--release".into(),
            d(&f.release),
            "--account".into(),
            d(&f.acct_b),
            "--ro".into(),
            d(&f.cdir),
            "--ssh-config".into(),
            d(&f.cfg),
            "--socket".into(),
            d(&f.sock),
            "--".into(),
            "sh".into(),
            "-c".into(),
        ]
    );
    let inner = &sandbox[15];
    assert!(
        inner.find(" unpack ").unwrap() < inner.find("&& exec ").unwrap(),
        "unpack before exec: {inner}"
    );
    // daemon の argv: release の celeris が測定 config と release の sha12 で起動する。
    assert_eq!(
        lines(&f.s_b.join("celeris-argv.txt")),
        [
            "--config".to_string(),
            d(&f.s_b.join("daemon.toml")),
            "--release".into(),
            SHA12.into(),
        ]
    );
    // 起動した時点で種が展開済み（submodule の中身を含み、.git なし）。
    let at_start = lines(&f.s_b.join("celeris-tree.txt"));
    for want in ["./src/lib.c", "./lib/pluvio/Cargo.toml"] {
        assert!(at_start.iter().any(|l| l == want), "{want} in {at_start:?}");
    }
    assert!(!at_start.iter().any(|l| l.contains(".git")), "{at_start:?}");

    // A の command を同じ種で走らせ、A と B の初期ツリーが同じであることを確かめる。
    let a_line = one_line(&f.a_command());
    ok(&f.eval(&a_line), "A command");
    let work_a = f.s_a.join("work/benchfs");
    assert_eq!(lines(&f.s_a.join("a-transcript.jsonl")), at_start);
    assert_eq!(tree(&work_a), tree(&work_b));
    assert_eq!(
        work_a.strip_prefix(&f.s_a).unwrap(),
        work_b.strip_prefix(&f.s_b).unwrap()
    );

    // 同じ state で 2 回目は始めない（作業ツリーが空でない）。
    assert_eq!(f.b_command(&[]).status.code(), Some(1));
}

#[test]
fn b_command_refuses_bad_arguments_and_missing_prerequisites() {
    let f = Fixture::new(false);
    let sha_typo = "A".repeat(64);
    assert_eq!(
        f.b_command(&[("--seed-sha256", &sha_typo)]).status.code(),
        Some(2)
    );
    assert_eq!(
        f.b_command(&[("--socket", "relative/sock")]).status.code(),
        Some(2)
    );
    // release-store が別の release を指す（付記 D-store の形でない）。
    let other = f.root.join("other-release");
    fs::create_dir_all(other.join("bin")).unwrap();
    fs::copy(f.release.join("bin/celeris"), other.join("bin/celeris")).unwrap();
    fs::copy(f.release.join("manifest.json"), other.join("manifest.json")).unwrap();
    let o = f.b_command(&[("--release", other.to_str().unwrap())]);
    assert_eq!(
        o.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    // 案件の body の workspace が作業ツリーを指さない。
    let pj = f.s_b.join("setup/project.json");
    let saved = fs::read(&pj).unwrap();
    fs::write(&pj, r#"{"title": "t", "request": "r"}"#).unwrap();
    assert_eq!(f.b_command(&[]).status.code(), Some(1));
    fs::write(&pj, &saved).unwrap();
    // 種が無い。
    let seed = f.s_b.join("seed/benchfs.tar");
    fs::rename(&seed, f.root.join("moved.tar")).unwrap();
    assert_eq!(f.b_command(&[]).status.code(), Some(1));
    fs::rename(f.root.join("moved.tar"), &seed).unwrap();
    // 前提が揃えば出す。
    assert_eq!(f.b_command(&[]).status.code(), Some(0));
}

/// 実 bwrap の包み（ops-r2-sandbox.sh）の中で、種の展開から daemon の exec までを通す。
#[test]
fn b_command_runs_in_the_real_sandbox() {
    let setting = std::env::var("CELERIS_USERNS_TESTS").unwrap_or_default();
    if setting != "1" && setting != "require" {
        eprintln!("skip: real bwrap start requires CELERIS_USERNS_TESTS=1 (or require)");
        return;
    }
    let f = Fixture::new(true);
    let ran = f.eval(&one_line(&f.b_command(&[])));
    ok(&ran, "B command in ops-r2-sandbox.sh");
    let at_start = lines(&f.s_b.join("celeris-tree.txt"));
    assert!(
        at_start.iter().any(|l| l == "./lib/pluvio/Cargo.toml"),
        "{at_start:?}"
    );
    // 包みの cwd は state（--chdir）。
    assert_eq!(
        lines(&f.s_b.join("celeris-cwd.txt")),
        [f.s_b.display().to_string()]
    );
}
