//! ADR-0078 D1 / §6 項目 2: `scripts/cluster-login.sh`（人が張る経路）の `ssh -M -N -f` に
//! `ControlPersist=yes` と keepalive が渡る。偽の `ssh`（argv を書き出すだけ）を PATH の先頭に置く。
//! 実 ssh・外部ネットワークには出ない。

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

fn write_fake_ssh(dir: &Path, log: &Path, counter: &Path) {
    // `-O check` は 1 回目が 1（master 無し）、2 回目以降が 0（張れた）。それ以外の呼び出しは argv を記録して 0。
    let script = format!(
        "#!/bin/sh\n\
         is_check=0\n\
         for a in \"$@\"; do if [ \"$a\" = check ]; then is_check=1; fi; done\n\
         if [ \"$is_check\" = 1 ]; then\n\
           n=$(cat {counter:?} 2>/dev/null || echo 0)\n\
           n=$((n + 1))\n\
           echo $n > {counter:?}\n\
           if [ \"$n\" -ge 2 ]; then exit 0; else exit 1; fi\n\
         fi\n\
         printf '%s\\n' \"$@\" > {log:?}\n\
         exit 0\n"
    );
    let path = dir.join("ssh");
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn cluster_login_script_passes_control_persist_and_keepalive_to_the_master() {
    let bin = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let log = state.path().join("master_argv");
    let counter = state.path().join("counter");
    write_fake_ssh(bin.path(), &log, &counter);
    std::fs::create_dir(home.path().join(".ssh")).unwrap();

    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/cluster-login.sh");
    let path = format!(
        "{}:{}",
        bin.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let output = Command::new("bash")
        .arg(&script)
        .arg("fake-host")
        .env("PATH", path)
        .env("HOME", home.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let argv = std::fs::read_to_string(&log).unwrap();
    let lines: Vec<&str> = argv.lines().collect();
    let has_pair = |opt: &str| lines.windows(2).any(|w| w[0] == "-o" && w[1] == opt);
    assert!(has_pair("ControlPersist=yes"), "{argv}");
    assert!(has_pair("ServerAliveInterval=30"), "{argv}");
    assert!(has_pair("ServerAliveCountMax=3"), "{argv}");
    for flag in ["-M", "-N", "-f", "fake-host"] {
        assert!(lines.contains(&flag), "{flag} missing: {argv}");
    }
    // -o オプションは -M より前（ssh は最初の非オプションまで解釈するので、順序の崩れを検出する）。
    let persist_at = lines
        .iter()
        .position(|l| *l == "ControlPersist=yes")
        .unwrap();
    let m_at = lines.iter().position(|l| *l == "-M").unwrap();
    assert!(persist_at < m_at, "{argv}");
}
