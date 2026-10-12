#!/usr/bin/env python3
"""Takes a sample of the FUSE spreadsheet corpus without storing it.

Zenodo's fuse.zip holds FUSE.7z.001..140, a split 7z archive of LZMA2
streams (mostly uncompressed chunks, one after the other) holding the
analysis JSON files and the corpus, each as a gzip file. This script reads the volumes in order with
HTTP range requests, decodes the LZMA2 stream, skips every gzip member
that isn't a tar file, and reads the tar stream, keeping every STEP-th file (named by its hash in the corpus;
the extension comes from its content).

    fuse_stream.py URL OUTDIR [--step N]
"""

import argparse
import io
import lzma
import os
import sys
import tarfile
import zipfile
import zlib

sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import zipsample  # noqa: E402

zipsample.BLOCK = 16 << 20


class Chain(io.RawIOBase):
    """The volumes one after the other, as one stream."""

    def __init__(self, zf, names):
        self.zf = zf
        self.names = list(names)
        self.cur = None

    def readable(self):
        return True

    def readinto(self, buf):
        while True:
            if self.cur is None:
                if not self.names:
                    return 0
                name = self.names.pop(0)
                print(f"volume {name}", file=sys.stderr, flush=True)
                self.cur = self.zf.open(name)
            n = self.cur.readinto(buf)
            if n:
                return n
            self.cur = None


class Lzma2(io.RawIOBase):
    """The raw LZMA2 streams of a 7z archive (one per folder, back to
    back), decoded."""

    def __init__(self, stream, dict_size):
        self.stream = stream
        self.dict_size = dict_size
        self.d = self._new()
        self.out = b""
        self.left = b""

    def _new(self):
        return lzma.LZMADecompressor(lzma.FORMAT_RAW, filters=[{"id": lzma.FILTER_LZMA2, "dict_size": self.dict_size}])

    def readable(self):
        return True

    def readinto(self, buf):
        while not self.out:
            if self.d.eof:
                # The next folder's stream starts right after.
                self.left = self.d.unused_data
                self.d = self._new()
            if self.left:
                chunk, self.left = self.left, b""
            elif self.d.needs_input:
                chunk = self.stream.read(1 << 20)
                if not chunk:
                    return 0
            else:
                chunk = b""
            self.out = self.d.decompress(chunk, 4 << 20)
        n = min(len(buf), len(self.out))
        buf[:n] = self.out[:n]
        self.out = self.out[n:]
        return n


class Prefixed(io.RawIOBase):
    """`head` bytes, then the rest of `stream`."""

    def __init__(self, head, stream):
        self.head = head
        self.stream = stream

    def readable(self):
        return True

    def readinto(self, buf):
        if self.head:
            n = min(len(buf), len(self.head))
            buf[:n] = self.head[:n]
            self.head = self.head[n:]
            return n
        return self.stream.readinto(buf)


def next_gzip(stream):
    """Moves to the next gzip member: returns its stored file name and a
    stream that starts at it, or None at the end."""
    data = b""
    while True:
        i = data.find(b"\x1f\x8b\x08")
        if i >= 0 and len(data) - i >= 1024:
            break
        chunk = stream.read(1 << 16)
        if not chunk:
            if i < 0:
                return None
            break
        data += chunk
        if i < 0 and len(data) > (4 << 20):
            data = data[-4:]
    data = data[i:]
    name = b""
    if data[3] & 0x08:  # FNAME
        name = data[10:data.index(b"\0", 10)]
    return name.decode("latin-1"), io.BufferedReader(Prefixed(data, stream), 1 << 20)


def skip_gzip(stream):
    """Reads a gzip member to its end; returns the stream after it."""
    d = zlib.decompressobj(31)
    while not d.eof:
        chunk = stream.read(1 << 20)
        if not chunk:
            return stream
        d.decompress(chunk)
        while d.unconsumed_tail:
            d.decompress(d.unconsumed_tail)
    return io.BufferedReader(Prefixed(d.unused_data, stream), 1 << 20)


def is_tar(stream):
    """Whether the gzip member at the start of `stream` holds a tar file."""
    head = stream.peek(1 << 16)
    try:
        data = zlib.decompressobj(31).decompress(head, 1024)
    except zlib.error:
        return False
    return data[257:262] == b"ustar"


def kind(data):
    if data.startswith(b"\xd0\xcf\x11\xe0"):
        return "xls"
    if data.startswith(b"PK"):
        return "xlsx"
    return None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("url")
    ap.add_argument("outdir")
    ap.add_argument("--step", type=int, default=60)
    args = ap.parse_args()
    os.makedirs(args.outdir, exist_ok=True)
    zf = zipfile.ZipFile(zipsample.HttpFile(args.url))
    vols = sorted(n for n in zf.namelist() if n.startswith("FUSE.7z."))
    stream = io.BufferedReader(Chain(zf, vols), 1 << 20)
    stream.read(32)  # 7z signature header
    stream = io.BufferedReader(Lzma2(stream, 256 << 20), 1 << 20)
    while True:
        got = next_gzip(stream)
        if got is None:
            sys.exit("no tar file in the archive")
        name, stream = got
        tar = is_tar(stream)
        print(f"gzip member {name} (tar: {tar})", file=sys.stderr, flush=True)
        if tar:
            break
        stream = skip_gzip(stream)
    n = kept = 0
    with tarfile.open(fileobj=stream, mode="r|gz") as tar:
        for m in tar:
            if not m.isfile():
                continue
            n += 1
            if n % args.step != 1:
                continue
            data = tar.extractfile(m).read()
            ext = kind(data)
            if ext is None:
                continue
            name = os.path.basename(m.name)
            out = os.path.join(args.outdir, f"{name}.{ext}")
            if not os.path.exists(out):
                with open(out + ".part", "wb") as f:
                    f.write(data)
                os.replace(out + ".part", out)
            kept += 1
            if kept % 200 == 0:
                print(f"{kept} kept of {n}", file=sys.stderr, flush=True)
    print(f"{kept} kept of {n} files", file=sys.stderr)


if __name__ == "__main__":
    main()
