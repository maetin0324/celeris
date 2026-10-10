# Gateway node test hang repair

---
tasks: [gateway-tests]
---

## Cause

`web/server/browser-live.test.mjs` was the file that stopped the sequential `node --test` run. Its fixture built the owner and trusted-device Unix sockets under `os.tmpdir()`. In this run that made `owner.sock` 132 bytes and a device socket 143 bytes, exceeding Linux `sockaddr_un.sun_path` (108 bytes including its limit). `startSocket()` therefore failed to bind; the fixture awaited only `listening`, never observed `error`, and node reported a pending promise until externally interrupted. The same test file passed with `TMPDIR=/tmp`, confirming the path-length cause.

## Fix

The fixture now allocates sockets in a private short directory under `/tmp` when the normal temp path would exceed the Unix socket limit; otherwise it stays under `os.tmpdir()`. It uses that directory for the owner and trusted-device sockets and removes it in teardown. Socket server startup waits now reject on `error`, including the common TCP fixture helper, so bind failures surface immediately instead of hanging.

## Verification

- `timeout 90 node --test server/browser-live.test.mjs` with the run TMPDIR: 19 passed, exit 0.
- Ran each `server/*.test.mjs` file individually with a 120-second timeout under the run TMPDIR; all files exited 0 after the fix. The previous hang was isolated to `browser-live.test.mjs` (19 tests).
- `timeout 900 node --test server/*.test.mjs` with the run TMPDIR: 78 passed, exit 0.
- `TMPDIR=/tmp timeout 900 node --test server/*.test.mjs`: 78 passed, exit 0.
- `corepack pnpm@12.6.0 -C web run lint`: exit 0, checked 459 files; 4 existing `!important` warnings.
