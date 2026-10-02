//! ADR-0095: worker の run から本番 DB は読み取り専用（D1〜D5）。
//!
//! celeris が task のために起動するプロセス（adapter・check。コンテナ実行を除く）は、exec の直前（fork 後の
//! 子、`pre_exec`）に unprivileged な user + mount namespace に入り、**DB のディレクトリを読み取り専用**に
//! する。DB 本体と `-wal` / `-shm` / `-journal` 以外の直下の項目は自分自身へ bind し直すので従来どおり書ける。
//! プロセスの継承で効くので、エージェント CLI の sandbox の有無・escalation・子プロセスに関係なく掛かる。
//!
//! - 入口は [`launch`]（`container::wrap` の代わり）。コンテナ実行ならそのまま `container::wrap`、そうでなければ
//!   [`install`] されたガードを [`apply`] する（無ければ何もしない）。
//! - 準備（直下の列挙・`statvfs`・uid/gid・cwd の絶対化・ssh 設定の写し）は fork の**前**に親で行い、子では
//!   用意した C 文字列で `unshare` / `mount` / `chdir` を呼ぶだけ（割り当てをしない）。
//! - 付記 D-a: user systemd bus を tmpfs / 空ファイルの bind で覆い、`$HOME` の `.config/systemd`・
//!   `.local/celeris/releases`・`.config/celeris` を（存在すれば）読み取り専用にする（[`DbGuard::host_config_read_only_paths`]）。
//! - 準備に失敗したら、その spawn を失敗させる（黙って保護なしで起動しない）。

use std::ffi::{CString, OsString};
use std::io;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use nix::libc;

/// ssh の `Include` 先（D4）。root 所有のファイルが namespace の中では nobody に見え、ssh が拒否する。
const SSH_CONFIG_D: &str = "/etc/ssh/ssh_config.d";

/// 付記 D-a の 3: namespace の中で読み取り専用にする `$HOME` 下の host の設定（存在するものだけ）。
const HOST_CONFIG_DIRS: [&str; 3] = [
    ".config/systemd",
    ".local/celeris/releases",
    ".config/celeris",
];

#[derive(Debug, thiserror::Error)]
pub enum DbGuardError {
    #[error("db {path}: {source}")]
    Db { path: PathBuf, source: io::Error },
    #[error("db {0} has no parent directory")]
    NoParent(PathBuf),
    #[error("probe failed: {0}")]
    Probe(String),
}

/// 保護する DB（`[db] path`）。`DbGuard::new` は DB ファイルが存在することを要求する（daemon が開いた後に作る）。
#[derive(Debug, Clone)]
pub struct DbGuard {
    db: PathBuf,
    dir: PathBuf,
    /// 読み取り専用のまま残す直下の名前（DB 本体と `-wal` / `-shm` / `-journal`）。
    family: Vec<OsString>,
    /// ssh 設定の写しを置く所（D4）。`None` なら写さない。
    ssh_shadow_root: Option<PathBuf>,
    /// 付記 D-a の 3 の基準の `$HOME`（`new` で解決。`None` なら何もしない）。
    home: Option<PathBuf>,
}

impl DbGuard {
    pub fn new(db_path: &Path) -> Result<Self, DbGuardError> {
        let db = std::fs::canonicalize(db_path).map_err(|source| DbGuardError::Db {
            path: db_path.to_path_buf(),
            source,
        })?;
        let dir = db
            .parent()
            .ok_or_else(|| DbGuardError::NoParent(db.clone()))?
            .to_path_buf();
        let name = db
            .file_name()
            .ok_or_else(|| DbGuardError::NoParent(db.clone()))?
            .to_os_string();
        let family = ["", "-wal", "-shm", "-journal"]
            .iter()
            .map(|suffix| {
                let mut n = name.clone();
                n.push(suffix);
                n
            })
            .collect();
        Ok(Self {
            db,
            dir,
            family,
            ssh_shadow_root: Some(default_ssh_shadow_root()),
            home: std::env::var_os("HOME")
                .filter(|h| !h.is_empty())
                .map(PathBuf::from)
                .filter(|h| h.is_absolute()),
        })
    }

    /// 試験用: 付記 D-a の 3 の基準の `$HOME` を変える（`None` で読み取り専用にしない）。
    pub fn with_home(mut self, home: Option<PathBuf>) -> Self {
        self.home = home;
        self
    }

    /// 付記 D-a の 3: `$HOME` 下の host の設定のうち、存在するもの（canonicalize 済み）。存在するかを
    /// 確かめられない（`EACCES` など）ときは失敗にする（D5: 保護なしで起動しない）。
    pub fn host_config_read_only_paths(&self) -> io::Result<Vec<PathBuf>> {
        let Some(home) = &self.home else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        for rel in HOST_CONFIG_DIRS {
            let path = home.join(rel);
            if path.try_exists()? {
                out.push(std::fs::canonicalize(&path)?);
            }
        }
        Ok(out)
    }

    /// 試験用: ssh 設定の写しの置き場所を変える（`None` で写さない）。
    pub fn with_ssh_shadow_root(mut self, root: Option<PathBuf>) -> Self {
        self.ssh_shadow_root = root;
        self
    }

    /// canonicalize 済みの DB のパス。
    pub fn db_path(&self) -> &Path {
        &self.db
    }

    /// 読み取り専用にするディレクトリ（DB の親）。
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// 直下のうち、書けるように bind し直す項目（ディレクトリと通常ファイル。symlink は辿らない）。
    /// spawn のたびに列挙する（daemon の起動後に増えた兄弟も書ける）。
    pub fn writable_children(&self) -> io::Result<Vec<PathBuf>> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(&self.dir)? {
            let entry = entry?;
            let name = entry.file_name();
            if self.family.contains(&name) {
                continue;
            }
            let ty = entry.file_type()?;
            if ty.is_dir() || ty.is_file() {
                out.push(self.dir.join(name));
            }
        }
        out.sort();
        Ok(out)
    }

    fn plan(&self, cwd: Option<&Path>, runtime_dir: Option<PathBuf>) -> io::Result<Plan> {
        let keep = self
            .writable_children()?
            .into_iter()
            .map(|p| cstring(p.into_os_string()))
            .collect::<io::Result<Vec<_>>>()?;
        let stat = nix::sys::statvfs::statvfs(&self.dir).map_err(io::Error::from)?;
        let cwd = match cwd {
            Some(c) if c.is_absolute() => Some(c.to_path_buf()),
            Some(c) => std::env::current_dir().ok().map(|d| d.join(c)),
            None => std::env::current_dir().ok(),
        };
        let ssh = match &self.ssh_shadow_root {
            Some(root) => sync_ssh_shadow(Path::new(SSH_CONFIG_D), root)
                .ok()
                .flatten()
                .map(|shadow| -> io::Result<(CString, CString)> {
                    Ok((
                        cstring(shadow.into_os_string())?,
                        cstring(OsString::from(SSH_CONFIG_D))?,
                    ))
                })
                .transpose()?,
            None => None,
        };
        let read_only = self
            .host_config_read_only_paths()?
            .into_iter()
            .map(|p| -> io::Result<(CString, libc::c_ulong)> {
                let stat = nix::sys::statvfs::statvfs(&p).map_err(io::Error::from)?;
                Ok((
                    cstring(p.into_os_string())?,
                    libc::MS_REMOUNT
                        | libc::MS_BIND
                        | libc::MS_RDONLY
                        | locked_mount_flags(stat.flags()),
                ))
            })
            .collect::<io::Result<Vec<_>>>()?;
        let uid = nix::unistd::getuid().as_raw();
        let gid = nix::unistd::getgid().as_raw();
        let mut bus_paths = Vec::new();
        let host_bus = PathBuf::from(format!("/run/user/{uid}/bus"));
        if host_bus.exists() {
            bus_paths.push(cstring(host_bus.into_os_string())?);
        }
        let mut systemd_dir = None;
        if let Some(runtime) = runtime_dir.filter(|p| !p.as_os_str().is_empty()) {
            if !runtime.is_absolute() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "XDG_RUNTIME_DIR is not absolute",
                ));
            }
            let bus = runtime.join("bus");
            if bus.exists()
                && !bus_paths
                    .iter()
                    .any(|p| p.as_bytes() == bus.as_os_str().as_bytes())
            {
                bus_paths.push(cstring(bus.into_os_string())?);
            }
            let systemd = runtime.join("systemd");
            if systemd.exists() {
                systemd_dir = Some(cstring(systemd.into_os_string())?);
            }
        }
        // Bind mounts from anonymous fds (memfd/O_TMPFILE) fail on socket targets.
        // NamedTempFile keeps a randomly named, empty regular file alive through pre_exec.
        let empty_bus = if bus_paths.is_empty() {
            None
        } else {
            Some(tempfile::NamedTempFile::new()?)
        };
        let empty_bus_path = empty_bus
            .as_ref()
            .map(|file| cstring(file.path().as_os_str().to_os_string()))
            .transpose()?;
        Ok(Plan {
            dir: cstring(self.dir.clone().into_os_string())?,
            keep,
            remount_flags: libc::MS_REMOUNT
                | libc::MS_BIND
                | libc::MS_RDONLY
                | locked_mount_flags(stat.flags()),
            cwd: cwd.map(|c| cstring(c.into_os_string())).transpose()?,
            uid_map: format!("{uid} {uid} 1").into_bytes(),
            gid_map: format!("{gid} {gid} 1").into_bytes(),
            ssh,
            read_only,
            bus_paths,
            systemd_dir,
            _empty_bus: empty_bus,
            empty_bus_path,
        })
    }
}

/// fork の前に作る、子で使う値（C 文字列と数値だけ）。
struct Plan {
    dir: CString,
    keep: Vec<CString>,
    remount_flags: libc::c_ulong,
    cwd: Option<CString>,
    uid_map: Vec<u8>,
    gid_map: Vec<u8>,
    /// (写し, `/etc/ssh/ssh_config.d`)
    ssh: Option<(CString, CString)>,
    /// 付記 D-a の 3: 自分自身へ bind して読み取り専用にする path と、その remount の flag。
    read_only: Vec<(CString, libc::c_ulong)>,
    bus_paths: Vec<CString>,
    systemd_dir: Option<CString>,
    /// Retains the source file until all bus socket paths have been overmounted.
    _empty_bus: Option<tempfile::NamedTempFile>,
    empty_bus_path: Option<CString>,
}

impl Plan {
    /// 子（fork 後・exec 前）で呼ぶ。割り当てをせず、libc の呼び出しだけを行う。
    fn enter(&self) -> io::Result<()> {
        // SAFETY: fork 後の子で、親が用意した NUL 終端の文字列だけを渡す。どれも async-signal-safe な
        // システムコールの薄い包み。
        unsafe {
            check(libc::unshare(libc::CLONE_NEWUSER | libc::CLONE_NEWNS))?;
            write_proc(c"/proc/self/setgroups", b"deny")?;
            write_proc(c"/proc/self/uid_map", &self.uid_map)?;
            write_proc(c"/proc/self/gid_map", &self.gid_map)?;
            check(libc::mount(
                std::ptr::null(),
                c"/".as_ptr(),
                std::ptr::null(),
                libc::MS_REC | libc::MS_PRIVATE,
                std::ptr::null(),
            ))?;
            for child in &self.keep {
                bind(child, child, libc::MS_REC)?;
            }
            if let Some((shadow, target)) = &self.ssh {
                // D4 は best-effort（失敗しても DB の保護には関係しない）。
                let _ = bind(shadow, target, 0);
            }
            if let Some(systemd) = &self.systemd_dir {
                check(libc::mount(
                    c"tmpfs".as_ptr(),
                    systemd.as_ptr(),
                    c"tmpfs".as_ptr(),
                    libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC,
                    c"size=4096,mode=0700".as_ptr().cast(),
                ))?;
            }
            if let Some(source) = &self.empty_bus_path {
                for target in &self.bus_paths {
                    bind(source, target, 0)?;
                }
            }
            for (path, flags) in &self.read_only {
                bind(path, path, 0)?;
                check(libc::mount(
                    std::ptr::null(),
                    path.as_ptr(),
                    std::ptr::null(),
                    *flags,
                    std::ptr::null(),
                ))?;
            }
            bind(&self.dir, &self.dir, libc::MS_REC)?;
            check(libc::mount(
                std::ptr::null(),
                self.dir.as_ptr(),
                std::ptr::null(),
                self.remount_flags,
                std::ptr::null(),
            ))?;
            // std は pre_exec より前に cwd へ chdir するので、cwd は mount の下の古い dentry を指している。
            // 絶対パスで入り直し、`../celeris.sqlite3` が元の（書ける）mount に届かないようにする。
            if let Some(cwd) = &self.cwd {
                check(libc::chdir(cwd.as_ptr()))?;
            }
        }
        Ok(())
    }
}

unsafe fn bind(src: &CString, dst: &CString, extra: libc::c_ulong) -> io::Result<()> {
    // SAFETY: 呼び出し側（`Plan::enter`）と同じ。
    check(unsafe {
        libc::mount(
            src.as_ptr(),
            dst.as_ptr(),
            std::ptr::null(),
            libc::MS_BIND | extra,
            std::ptr::null(),
        )
    })
}

unsafe fn write_proc(path: &std::ffi::CStr, data: &[u8]) -> io::Result<()> {
    // SAFETY: 呼び出し側（`Plan::enter`）と同じ。
    unsafe {
        let fd = libc::open(path.as_ptr(), libc::O_WRONLY | libc::O_CLOEXEC);
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let n = libc::write(fd, data.as_ptr().cast(), data.len());
        let err = io::Error::last_os_error();
        libc::close(fd);
        if n < 0 || n as usize != data.len() {
            return Err(err);
        }
    }
    Ok(())
}

fn check(rc: libc::c_int) -> io::Result<()> {
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn cstring(s: OsString) -> io::Result<CString> {
    CString::new(s.into_vec()).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))
}

fn runtime_dir(command: &std::process::Command) -> Option<PathBuf> {
    command
        .get_envs()
        .find(|(name, _)| *name == "XDG_RUNTIME_DIR")
        .map(|(_, value)| value.map(PathBuf::from))
        .unwrap_or_else(|| std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from))
}

/// user namespace では元の mount の `nosuid` / `nodev` / `noexec` / atime 系は外せない（locked）ので、
/// 読み取り専用にする remount でも引き継ぐ。
fn locked_mount_flags(flags: nix::sys::statvfs::FsFlags) -> libc::c_ulong {
    use nix::sys::statvfs::FsFlags;
    let mut out = 0;
    for (st, ms) in [
        (FsFlags::ST_NOSUID, libc::MS_NOSUID),
        (FsFlags::ST_NODEV, libc::MS_NODEV),
        (FsFlags::ST_NOEXEC, libc::MS_NOEXEC),
        (FsFlags::ST_NOATIME, libc::MS_NOATIME),
        (FsFlags::ST_NODIRATIME, libc::MS_NODIRATIME),
        (FsFlags::ST_RELATIME, libc::MS_RELATIME),
    ] {
        if flags.contains(st) {
            out |= ms;
        }
    }
    out
}

fn default_ssh_shadow_root() -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR").filter(|v| !v.is_empty()) {
        Some(dir) => PathBuf::from(dir).join("celeris-db-guard"),
        None => std::env::temp_dir().join(format!(
            "celeris-db-guard-{}",
            nix::unistd::getuid().as_raw()
        )),
    }
}

/// D4: `src`（`/etc/ssh/ssh_config.d`）の通常ファイル（symlink は辿って中身を読む）を `root/ssh_config.d` へ
/// 同じ内容で写す（この uid の所有になる）。`src` が無ければ `Ok(None)`。内容が同じファイルは書かない。
/// 写しにだけある名前は消す。書き込みは一時ファイル + rename（並行する spawn が半端な中身を読まない）。
pub fn sync_ssh_shadow(src: &Path, root: &Path) -> io::Result<Option<PathBuf>> {
    if !src.is_dir() {
        return Ok(None);
    }
    let shadow = root.join("ssh_config.d");
    std::fs::create_dir_all(&shadow)?;
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700));
    }
    let mut names: Vec<OsString> = Vec::new();
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = entry.file_name();
        let Ok(content) = std::fs::read(&path) else {
            continue;
        };
        let dest = shadow.join(&name);
        if std::fs::read(&dest).ok().as_deref() != Some(content.as_slice()) {
            let mut tmp_name = OsString::from(".tmp-");
            tmp_name.push(format!(
                "{}-{}-",
                std::process::id(),
                TMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            tmp_name.push(&name);
            let tmp = shadow.join(tmp_name);
            std::fs::write(&tmp, &content)?;
            std::fs::rename(&tmp, &dest)?;
        }
        names.push(name);
    }
    for entry in std::fs::read_dir(&shadow)? {
        let entry = entry?;
        let name = entry.file_name();
        if !names.contains(&name) && !name.as_bytes().starts_with(b".tmp-") {
            let _ = std::fs::remove_file(entry.path());
        }
    }
    Ok(Some(shadow))
}

static TMP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// `command` に D1 の `pre_exec` を付ける。準備に失敗したら、その理由を log に残し、spawn が同じ errno で
/// 失敗するようにする（保護なしで起動しない）。
pub fn apply(command: &mut tokio::process::Command, guard: &DbGuard) {
    // 守る DB がもう無い（同じプロセスで先に終わった daemon〈in-process の試験〉が入れたガードの残り）なら
    // 守るものが無いので掛けない。本番の daemon の DB は動いている間に消えない（namespace の中からは消せない）。
    if !guard.db.exists() {
        tracing::warn!(
            db = %guard.db.display(),
            "worker db guard (ADR-0095): the guarded db no longer exists; spawning without the namespace"
        );
        return;
    }
    let cwd = command.as_std().get_current_dir().map(Path::to_path_buf);
    match guard.plan(cwd.as_deref(), runtime_dir(command.as_std())) {
        Ok(plan) => {
            // SAFETY: `Plan::enter` は fork 後の子で libc の呼び出しだけを行い、割り当てをしない。
            unsafe {
                command.pre_exec(move || plan.enter());
            }
        }
        Err(e) => {
            tracing::error!(
                db = %guard.db.display(),
                "worker db guard (ADR-0095): cannot prepare the read-only namespace, refusing to spawn: {e}"
            );
            let errno = e.raw_os_error().unwrap_or(libc::EPERM);
            // SAFETY: 何も呼ばずに errno を返すだけ。
            unsafe {
                command.pre_exec(move || Err(io::Error::from_raw_os_error(errno)));
            }
        }
    }
}

/// `apply` の std 版（起動時の probe と試験用）。
pub fn apply_std(command: &mut std::process::Command, guard: &DbGuard) -> io::Result<()> {
    use std::os::unix::process::CommandExt;
    let plan = guard.plan(command.get_current_dir(), runtime_dir(command))?;
    // SAFETY: `apply` と同じ。
    unsafe {
        command.pre_exec(move || plan.enter());
    }
    Ok(())
}

/// D5/D-a: 起動時に namespace 付きプロセスで DB の読み取り専用化と user bus の遮断を確かめる。
pub fn probe(guard: &DbGuard) -> Result<(), DbGuardError> {
    let mut command = std::process::Command::new("sh");
    command
        .arg("-c")
        .arg("test ! -w \"$1\" && test ! -S \"/run/user/$(id -u)/bus\" && { test -z \"$XDG_RUNTIME_DIR\" || { test ! -S \"$XDG_RUNTIME_DIR/bus\" && test ! -e \"$XDG_RUNTIME_DIR/systemd/private\"; }; }")
        .arg("sh")
        .arg(guard.db_path())
        .env_remove("DBUS_SESSION_BUS_ADDRESS")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());
    apply_std(&mut command, guard).map_err(|e| DbGuardError::Probe(e.to_string()))?;
    let out = command
        .output()
        .map_err(|e| DbGuardError::Probe(format!("spawn in the namespace failed: {e}")))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(DbGuardError::Probe(format!(
            "{} is writable or the user systemd bus is visible inside the namespace ({}; {})",
            guard.db_path().display(),
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        )))
    }
}

static INSTALLED: RwLock<Option<Arc<DbGuard>>> = RwLock::new(None);

/// D5: daemon の `run` が入れるプロセス全体のガード（`None` で外す）。後から入れたものが勝つ。
pub fn install(guard: Option<DbGuard>) {
    let guard = guard.map(Arc::new);
    match INSTALLED.write() {
        Ok(mut slot) => *slot = guard,
        Err(poisoned) => *poisoned.into_inner() = guard,
    }
}

/// いま入っているガード。
pub fn installed() -> Option<Arc<DbGuard>> {
    match INSTALLED.read() {
        Ok(slot) => slot.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    }
}

/// D2: worker の run のプロセスを spawn する所で `container::wrap` の代わりに呼ぶ。コンテナ実行なら
/// `container::wrap`、そうでなければ入っているガードを付ける（無ければそのまま）。
pub fn launch(
    command: tokio::process::Command,
    container: Option<&crate::container::ContainerPlan>,
) -> tokio::process::Command {
    launch_with(command, container, installed().as_deref())
}

/// [`launch`] の本体。ガードを引数で受ける（試験はプロセス全体の [`install`] を使わずにここを呼ぶ。
/// install すると同じ test binary で並行する他の試験の spawn にも試験用の DB のガードが掛かり、その tempdir が
/// 消えた瞬間に `pre_exec` の bind が ENOENT になる）。
fn launch_with(
    mut command: tokio::process::Command,
    container: Option<&crate::container::ContainerPlan>,
    guard: Option<&DbGuard>,
) -> tokio::process::Command {
    command.env_remove("DBUS_SESSION_BUS_ADDRESS");
    if container.is_some() {
        return crate::container::wrap(command, container);
    }
    if let Some(guard) = guard {
        apply(&mut command, guard);
    }
    command
}

#[cfg(test)]
#[path = "db_guard_tests.rs"]
mod tests;
