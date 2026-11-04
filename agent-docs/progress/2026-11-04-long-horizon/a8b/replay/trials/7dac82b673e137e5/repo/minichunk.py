import os, sys


def write_chunk(path, data):
    with open(path, 'wb') as f:
        f.write(data)
        f.flush()
        os.fsync(f.fileno())


def read_chunk(path):
    with open(path, 'rb') as f:
        return f.read()


if __name__ == '__main__':
    write_chunk(sys.argv[1], sys.argv[2].encode())
    print(read_chunk(sys.argv[1]).decode())
