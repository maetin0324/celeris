# Source after defining root/repo and before committing the fake repository.
mkdir -p "$repo/web/server"
printf '{"name":"celeris-web","version":"0.1.0","packageManager":"pnpm@12.6.0"}\n' >"$repo/web/package.json"
printf 'lockfileVersion: 9.0\n' >"$repo/web/pnpm-lock.yaml"
cat >"$root/bin/web-pnpm" <<'WEB_PNPM'
#!/usr/bin/env bash
case "$1" in
  install) mkdir -p node_modules ;;
  release)
    name="celeris-web-0.1.0-$CELERIS_WEB_RELEASE"
    stage="$(mktemp -d)"
    mkdir -p "$stage/$name/server" release
    echo '// fake app' >"$stage/$name/server/index.js"
    echo '// fake app' >"$stage/$name/server/app.js"
    echo '{}' >"$stage/$name/package.json"
    tar -czf "release/$name.tar.gz" -C "$stage" "$name"
    rm -rf "$stage" ;;
esac
WEB_PNPM
chmod +x "$root/bin/web-pnpm"
export SD_WEB_PNPM="$root/bin/web-pnpm"
