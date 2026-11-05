import os, subprocess, sys


def write_chunk(path, data):
    with open(path, 'wb') as f:
        f.write(data)
        f.flush()
        os.fsync(f.fileno())


def read_chunk(path):
    with open(path, 'rb') as f:
        return f.read()


def verify(case, outcome):
    """chunk を書いて fsync し、別の process（再起動の代わり）で読み戻して照合する。

    一致すれば out/<outcome>.txt に検証の 1 行を書く。一致しなければ何も書かずに 1 で終わる。
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
        f.write('minichunk %s %s verified: write+fsync+restart read-back ok\n' % (case, outcome))
    return 0


if __name__ == '__main__':
    if sys.argv[1:2] == ['verify'] and len(sys.argv) == 4:
        sys.exit(verify(sys.argv[2], sys.argv[3]))
    if sys.argv[1:2] == ['read'] and len(sys.argv) == 3:
        sys.stdout.buffer.write(read_chunk(sys.argv[2]))
        sys.exit(0)
    write_chunk(sys.argv[1], sys.argv[2].encode())
    print(read_chunk(sys.argv[1]).decode())
