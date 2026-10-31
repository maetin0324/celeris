//! ops-r2-trial-a-command.sh（ADR 2026-10-31-ops-r2-preflight-fixes 付記 D-trial-a・D-trial-a2）。
//! script の写しの隣に argv を記録する偽の sandbox を置き、偽の seed script・claude で出した 1 行を sh で評価する。
//! 実 sirius・LLM・bwrap には出ない。承認票の条件（版・model・3600 秒）、K4 の A と同じ権限の形、
//! `@`・`:`・空白・`'` を含む path がそのまま渡ることを確かめる。
use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output},
};

const SID: &str = "0b9e4c1a-2f3d-4e5f-8a6b-7c8d9e0f1a2b";

fn script() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../scripts/campaign/ops-r2-trial-a-command.sh")
}

fn write_exec(path: &Path, body: &str) {
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn lines(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

struct Fixture {
    root: PathBuf,
    script: PathBuf,
    state: PathBuf,
    cdir: PathBuf,
    account: PathBuf,
    cfg: PathBuf,
    sock: PathBuf,
}

impl Fixture {
    /// `root` の下に、空白と `'` を含む dir と ssh-mux の形の socket path を作る。
    fn new(root: &Path) -> Self {
        let scripts = root.join("scripts");
        std::fs::create_dir_all(&scripts).unwrap();
        let script_copy = scripts.join("ops-r2-trial-a-command.sh");
        std::fs::copy(script(), &script_copy).unwrap();
        // 偽の sandbox: argv を 1 行ずつ記録し、`--` の後ろの command を実行する。
        let argv_log = root.join("sandbox-argv.txt");
        write_exec(
            &scripts.join("ops-r2-sandbox.sh"),
            &format!(
                "#!/bin/sh\nfor a in \"$@\"; do printf '%s\\n' \"$a\"; done > '{}'\n\
                 while [ \"$1\" != -- ]; do shift; done; shift; exec \"$@\"\n",
                argv_log.display()
            ),
        );
        let base = root.join("it's a dir");
        let state = base.join("a-state");
        let cdir = base.join("bin");
        let account = base.join("acct");
        let mux = base.join("run");
        for d in [
            state.join("seed"),
            state.join("bin"),
            state.join("work"),
            cdir.clone(),
            account.clone(),
            mux.clone(),
        ] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(state.join("seed/benchfs.tar"), b"x").unwrap();
        // 偽の seed script: argv を記録し、展開先を作る。
        std::fs::write(
            state.join("bin/ops-r2-seed.sh"),
            "for a in \"$@\"; do printf '%s\\n' \"$a\"; done > \"$(dirname \"$0\")/../seed-argv.txt\"\n\
             while [ \"$1\" != --into ]; do shift; done; mkdir -p \"$2\"\n",
        )
        .unwrap();
        std::fs::write(state.join("prompt.md"), b"the prompt").unwrap();
        // 偽の claude: argv・CLAUDE_CONFIG_DIR・cwd・stdin を記録する（stdout は transcript へ行く）。
        write_exec(
            &cdir.join("claude-2.1.303"),
            "#!/bin/sh\nfor a in \"$@\"; do printf '%s\\n' \"$a\"; done\n\
             printf 'CLAUDE_CONFIG_DIR=%s\\n' \"$CLAUDE_CONFIG_DIR\"\nprintf 'PWD=%s\\n' \"$PWD\"\ncat\n",
        );
        let cfg = base.join("ssh config");
        std::fs::write(&cfg, b"").unwrap();
        let sock = mux.join("ssh-mux-rmaeda@130.158.241.2:22");
        Fixture {
            root: root.to_owned(),
            script: script_copy,
            state,
            cdir,
            account,
            cfg,
            sock,
        }
    }

    fn run(&self, sha: &str, sid: &str) -> Output {
        let mut c = Command::new("sh");
        c.arg(&self.script);
        for (k, v) in [
            ("--state", &self.state),
            ("--claude-dir", &self.cdir),
            ("--account", &self.account),
            ("--ssh-config", &self.cfg),
            ("--socket", &self.sock),
        ] {
            c.arg(k).arg(v);
        }
        c.args(["--seed-sha256", sha, "--session-id", sid]);
        c.output().unwrap()
    }
}

#[test]
fn printed_command_runs_claude_with_k4_permission_shape_and_quoted_paths() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let f = Fixture::new(&root);
    let sha = "a".repeat(64);
    let out = f.run(&sha, SID);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let line = String::from_utf8(out.stdout).unwrap();
    assert_eq!(line.lines().count(), 1, "one line: {line}");
    assert!(!line.contains(" -F "), "trial must not be told -F: {line}");

    // 人がする通りに出した 1 行を sh で評価する。
    let ran = Command::new("sh")
        .args(["-c", &line])
        .current_dir(&f.root)
        .output()
        .unwrap();
    assert_eq!(
        ran.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&ran.stderr)
    );

    let sandbox = lines(&f.root.join("sandbox-argv.txt"));
    let d = |p: &Path| p.display().to_string();
    assert_eq!(
        sandbox[..12],
        [
            "--state".into(),
            d(&f.state),
            "--release".into(),
            d(&f.cdir),
            "--account".into(),
            d(&f.account),
            "--ssh-config".into(),
            d(&f.cfg),
            "--socket".into(),
            d(&f.sock),
            "--".into(),
            "sh".into(),
        ]
    );
    assert!(d(&f.sock).ends_with("/ssh-mux-rmaeda@130.158.241.2:22"));

    let seed = lines(&f.state.join("seed-argv.txt"));
    let work = f.state.join("work/benchfs");
    assert_eq!(
        seed,
        [
            "unpack".into(),
            "--seed".into(),
            d(&f.state.join("seed/benchfs.tar")),
            "--sha256".into(),
            sha.clone(),
            "--into".into(),
            d(&work),
        ]
    );

    let transcript = lines(&f.state.join("a-transcript.jsonl"));
    let want: Vec<String> = [
        "-p",
        "--model",
        "claude-sonnet-5-5",
        "--session-id",
        SID,
        "--output-format",
        "stream-json",
        "--verbose",
        "--permission-mode",
        "bypassPermissions",
        "--disallowedTools",
        "Task",
    ]
    .into_iter()
    .map(String::from)
    .chain([
        format!("CLAUDE_CONFIG_DIR={}", d(&f.account)),
        format!("PWD={}", d(&work)),
        "the prompt".into(),
    ])
    .collect();
    assert_eq!(transcript, want);
    // timeout 3600 で claude を exec する（承認票の 1 試行 3600 秒）。
    assert!(
        line.contains("exec timeout 3600 "),
        "missing timeout in {line}"
    );
}

#[test]
fn rejects_bad_arguments_and_missing_prerequisites() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let f = Fixture::new(&root);
    let sha = "a".repeat(64);
    assert_eq!(f.run("XYZ", SID).status.code(), Some(2));
    for sid in ["", "not-a-uuid", &SID.to_uppercase()] {
        assert_eq!(f.run(&sha, sid).status.code(), Some(2), "sid {sid:?}");
    }
    std::fs::remove_file(f.state.join("prompt.md")).unwrap();
    assert_eq!(f.run(&sha, SID).status.code(), Some(1));
}
