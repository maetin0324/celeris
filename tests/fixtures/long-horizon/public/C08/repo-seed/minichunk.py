import hashlib, json, os, re, subprocess, sys


def write_chunk(path, data):
    with open(path, 'wb') as f:
        f.write(data)
        f.flush()
        os.fsync(f.fileno())


def read_chunk(path):
    with open(path, 'rb') as f:
        return f.read()


def evidence_of(case, outcome):
    return hashlib.sha256(('minichunk %s %s chunk' % (case, outcome)).encode()).hexdigest()


def verify(case, outcome):
    """chunk を書いて fsync し、別の process（再起動の代わり）で読み戻して照合する。

    一致すれば out/<outcome>.txt に契約の 4 欄（README.md）を書く。一致しなければ何も書かずに 1 で終わる。
    """
    os.makedirs('out', exist_ok=True)
    chunk = os.path.join('out', '.chunk-' + outcome)
    data = ('minichunk %s %s chunk' % (case, outcome)).encode()
    write_chunk(chunk, data)
    back = subprocess.run([sys.executable, os.path.abspath(__file__), 'read', chunk],
                          check=True, stdout=subprocess.PIPE).stdout
    os.remove(chunk)
    if back != data:
        print('minichunk %s %s: read-back mismatch' % (case, outcome), file=sys.stderr)
        return 1
    with open(os.path.join('out', outcome + '.txt'), 'w') as f:
        f.write('case: %s\noutcome: %s\nstatus: verified\nevidence: %s\n'
                % (case, outcome, hashlib.sha256(back).hexdigest()))
    return 0


def fields_of(text):
    """README.md の「成果の契約」の 2 つの形を、欄の名前から値の一覧へ読む。"""
    if text.strip().startswith('{'):
        try:
            pairs = json.loads(text, object_pairs_hook=list)
            return {k.lower(): [v for x, v in pairs if x.lower() == k.lower()] for k, _ in pairs}
        except ValueError:
            pass
    found = {}
    for line in text.splitlines():
        line = re.sub(r'^\s*[-*]\s+', '', line).strip()
        m = re.match(r'([A-Za-z_]+)\s*[:\uff1a=]\s*(.*)$', line)
        if m:
            value = m.group(2).strip()
            if len(value) >= 2 and value[0] == value[-1] and value[0] in '`"\'':
                value = value[1:-1].strip()
            found.setdefault(m.group(1).lower(), []).append(value)
    return found


def check(case, outcome):
    """今の out/<outcome>.txt が README.md の契約を満たすかを確かめる。満たさない欄を stderr に書いて 1 で終わる。"""
    path = os.path.join('out', outcome + '.txt')
    if os.path.islink(path) or not os.path.isfile(path):
        print('%s: not a regular file' % path, file=sys.stderr)
        return 1
    if not 1 <= os.path.getsize(path) <= 65536:
        print('%s: size must be 1 byte to 64 KiB' % path, file=sys.stderr)
        return 1
    try:
        with open(path, encoding='utf-8') as f:
            found = fields_of(f.read().lstrip('\ufeff'))
    except UnicodeDecodeError:
        print('%s: not UTF-8' % path, file=sys.stderr)
        return 1
    want = {'case': case, 'outcome': outcome, 'status': 'verified', 'evidence': evidence_of(case, outcome)}
    bad = []
    for key, value in want.items():
        got = found.get(key, [])
        norm = [v.lower() if key in ('status', 'evidence') and isinstance(v, str) else v for v in got]
        if len(set(map(str, got))) != 1 or norm[0] != value:
            bad.append(key)
    if bad:
        print('%s: %s do not meet the contract' % (path, ', '.join(bad)), file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    if sys.argv[1:2] == ['verify'] and len(sys.argv) == 4:
        sys.exit(verify(sys.argv[2], sys.argv[3]))
    if sys.argv[1:2] == ['check'] and len(sys.argv) == 4:
        sys.exit(check(sys.argv[2], sys.argv[3]))
    if sys.argv[1:2] == ['read'] and len(sys.argv) == 3:
        sys.stdout.buffer.write(read_chunk(sys.argv[2]))
        sys.exit(0)
    write_chunk(sys.argv[1], sys.argv[2].encode())
    print(read_chunk(sys.argv[1]).decode())
