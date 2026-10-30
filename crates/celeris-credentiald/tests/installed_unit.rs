//! Exercise the installed ExecStart with the real broker and a stub systemctl.
//! No host systemd/config/vault is accessed.
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

struct Guard(Child);
impl Drop for Guard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn installed_credentiald_reads_config_in_home_and_hot_layouts() {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    for hot in [false, true] {
        let tmp = short_tempdir().unwrap();
        let home = tmp.path().join("home");
        let runtime = tmp.path().join("run");
        let state = if hot {
            tmp.path().join("hot")
        } else {
            home.join(".local/celeris")
        };
        let units = tmp.path().join("units");
        let bin = tmp.path().join("bin");
        for dir in [
            home.join(".config/celeris"),
            runtime.clone(),
            state.clone(),
            bin.clone(),
        ] {
            fs::create_dir_all(&dir).unwrap();
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let config = home.join(".config/celeris/config.toml");
        let uid = unsafe { libc::geteuid() };
        let launcher_uid = if uid == 995 { 996 } else { 995 };
        fs::write(
            &config,
            format!("[browser]\nruntime = 'launcher'\nlauncher_uid = {launcher_uid}\n"),
        )
        .unwrap();
        let systemctl = bin.join("systemctl");
        fs::write(&systemctl, format!("#!/bin/sh\ncase \"$*\" in\n  *MainPID*) echo {} ;;\n  '--user daemon-reload') ;;\n  *) exit 1 ;;\nesac\n", std::process::id())).unwrap();
        fs::set_permissions(&systemctl, fs::Permissions::from_mode(0o700)).unwrap();
        let path = format!("{}:/usr/bin:/bin", bin.display());
        let mut install = Command::new("bash");
        install
            .arg(repo.join("scripts/selfdeploy/install-units.sh"))
            .env_clear()
            .env("PATH", &path)
            .env("HOME", &home)
            .env("TMPDIR", tmp.path())
            .env("SD_UNIT_DIR", &units);
        if hot {
            install.env("CELERIS_STATE_DIR", &state);
        }
        let out = install.output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let unit_path = units.join("celeris-credentiald@.service");
        let old_unit = fs::read_to_string(&unit_path).unwrap().replace(
            "--config %h/.config/celeris/config.toml",
            &format!("--launcher-uid={launcher_uid}"),
        );
        fs::write(&unit_path, &old_unit).unwrap();
        assert!(!install.output().unwrap().status.success());
        assert_eq!(fs::read_to_string(&unit_path).unwrap(), old_unit);
        let out = install.arg("--replace-existing").output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let unit = fs::read_to_string(&unit_path).unwrap();
        assert!(!unit.contains("--launcher-uid"));
        let daemon = fs::read_to_string(units.join("celeris@.service")).unwrap();
        for unit in [&unit, &daemon] {
            assert!(unit.contains("--config %h/.config/celeris/config.toml"));
        }
        let release_bin = state.join("releases/test/bin");
        fs::create_dir_all(&release_bin).unwrap();
        symlink(
            std::env::var("CARGO_BIN_EXE_celeris-credentiald").unwrap(),
            release_bin.join("celeris-credentiald"),
        )
        .unwrap();
        let exec = unit
            .lines()
            .find_map(|l| l.strip_prefix("ExecStart="))
            .unwrap()
            .replace("%h", home.to_str().unwrap())
            .replace("%i", "test");
        // The unit's only command wrapper is /bin/sh -c '...'. Run its body
        // directly so Guard owns the broker PID after the shell's exec.
        let body = exec
            .strip_prefix("/bin/sh -c '")
            .unwrap()
            .strip_suffix('\'')
            .unwrap();
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", body])
            .env_clear()
            .env("HOME", &home)
            .env("PATH", &path)
            .env("XDG_RUNTIME_DIR", &runtime)
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        // Apply the literal Environment entries generated for the hot root.
        for line in unit.lines().filter_map(|l| l.strip_prefix("Environment=")) {
            let (key, value) = line.split_once('=').unwrap();
            if key != "PATH" {
                command.env(key, value);
            }
        }
        let mut child = Guard(command.spawn().unwrap());
        let control = runtime.join("celeris-credentiald/control.sock");
        let deadline = Instant::now() + Duration::from_secs(10);
        while !control.exists() {
            assert!(
                child.0.try_wait().unwrap().is_none(),
                "broker exited before readiness"
            );
            assert!(Instant::now() < deadline, "broker readiness timeout");
            std::thread::sleep(Duration::from_millis(10));
        }
        let reply = celeris_credentiald::ipc::call(&control, br#"{"op":"ping"}"#).unwrap();
        assert!(reply.success);
        drop(child);
        // The very same installed unit must now refuse a bad config: this proves
        // ExecStart actually loads it rather than merely accepting the argument.
        fs::write(&config, "[browser]\nlauncher_uid = 'invalid'").unwrap();
        let bad = command.output().unwrap();
        assert!(!bad.status.success());
        assert_eq!(
            String::from_utf8(bad.stderr).unwrap().trim(),
            "invalid_request"
        );
        fs::remove_file(config).unwrap();
        assert!(!command.output().unwrap().status.success());
    }
}

/// Unix socket を置く試験用の一時 dir。socket の path は `sun_path`（108 byte）に収まる必要があり、
/// run の `TMPDIR` のような深い path の下では bind できないので、そのときは `/tmp` に作る。
fn short_tempdir() -> std::io::Result<tempfile::TempDir> {
    let base = std::env::temp_dir();
    let base = if base.as_os_str().len() > 40 {
        std::path::PathBuf::from("/tmp")
    } else {
        base
    };
    tempfile::Builder::new().prefix("cz").tempdir_in(base)
}
