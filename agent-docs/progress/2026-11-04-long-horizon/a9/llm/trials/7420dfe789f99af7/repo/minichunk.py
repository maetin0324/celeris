import os, sys


def fsync_dir(path):
    """Flush a directory entry to disk (needed so a newly created file survives a crash)."""
    d = os.path.dirname(os.path.abspath(path))
    fd = os.open(d, os.O_RDONLY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def write_chunk(path, data):
    with open(path, 'wb') as f:
        f.write(data)
        f.flush()
        os.fsync(f.fileno())
    fsync_dir(path)


def read_chunk(path):
    with open(path, 'rb') as f:
        return f.read()


if __name__ == '__main__':
    if len(sys.argv) >= 3:
        write_chunk(sys.argv[1], sys.argv[2].encode())
    print(read_chunk(sys.argv[1]).decode())
