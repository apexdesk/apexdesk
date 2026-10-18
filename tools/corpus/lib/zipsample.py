#!/usr/bin/env python3
"""Extracts a sample of the members of a remote ZIP file without
downloading all of it: the central directory and each chosen member are
read with HTTP range requests. Standard library only.

    zipsample.py URL OUTDIR --ext xls,xlsx [--max N] [--prefix P] [--list]

Members are chosen deterministically: those whose extension matches, in
name order, evenly spread when there are more than --max. Files already
in OUTDIR are skipped, so the script can be run again after a failure.
"""

import argparse
import io
import os
import sys
import time
import urllib.request
import zipfile

BLOCK = 1 << 20


class HttpFile(io.RawIOBase):
    """A seekable read-only file over HTTP range requests, with a small
    cache of 1 MiB blocks."""

    def __init__(self, url):
        self.url = url
        self.pos = 0
        self.cache = {}
        req = urllib.request.Request(url, method="HEAD")
        with self._open(req) as r:
            self.url = r.geturl()
            self.size = int(r.headers["Content-Length"])

    def _open(self, req):
        for attempt in range(6):
            try:
                return urllib.request.urlopen(req, timeout=120)
            except Exception as e:  # network hiccups: retry with a pause
                if attempt == 5:
                    raise
                print(f"retry after {e}", file=sys.stderr)
                time.sleep(5 * (attempt + 1))

    def readable(self):
        return True

    def seekable(self):
        return True

    def tell(self):
        return self.pos

    def seek(self, off, whence=0):
        if whence == 0:
            self.pos = off
        elif whence == 1:
            self.pos += off
        else:
            self.pos = self.size + off
        return self.pos

    def _block(self, i):
        if i not in self.cache:
            if len(self.cache) > 64:
                self.cache.clear()
            start = i * BLOCK
            end = min(self.size, start + BLOCK) - 1
            req = urllib.request.Request(self.url, headers={"Range": f"bytes={start}-{end}"})
            with self._open(req) as r:
                self.cache[i] = r.read()
        return self.cache[i]

    def read(self, n=-1):
        if n is None or n < 0:
            n = self.size - self.pos
        n = max(0, min(n, self.size - self.pos))
        out = bytearray()
        while n > 0:
            b = self._block(self.pos // BLOCK)
            off = self.pos % BLOCK
            chunk = b[off:off + n]
            if not chunk:
                break
            out += chunk
            self.pos += len(chunk)
            n -= len(chunk)
        return bytes(out)

    def readinto(self, buf):
        data = self.read(len(buf))
        buf[:len(data)] = data
        return len(data)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("url")
    ap.add_argument("outdir")
    ap.add_argument("--ext", required=True, help="comma-separated extensions")
    ap.add_argument("--max", type=int, default=0, help="at most N members (0: all)")
    ap.add_argument("--prefix", default="", help="prefix for the extracted file names")
    ap.add_argument("--max-size", type=int, default=50 << 20, help="skip members bigger than this")
    args = ap.parse_args()

    exts = tuple("." + e.strip().lower() for e in args.ext.split(","))
    zf = zipfile.ZipFile(HttpFile(args.url))
    members = sorted(
        (i for i in zf.infolist() if not i.is_dir() and i.filename.lower().endswith(exts) and 0 < i.file_size <= args.max_size),
        key=lambda i: i.filename,
    )
    if args.max and len(members) > args.max:
        step = len(members) / args.max
        members = [members[int(k * step)] for k in range(args.max)]
    os.makedirs(args.outdir, exist_ok=True)
    got = 0
    for i in members:
        name = args.prefix + i.filename.replace("/", "_").replace("\\", "_")
        out = os.path.join(args.outdir, name)
        if os.path.exists(out):
            continue
        try:
            data = zf.read(i)
        except Exception as e:
            print(f"skip {i.filename}: {e}", file=sys.stderr)
            continue
        with open(out + ".part", "wb") as f:
            f.write(data)
        os.replace(out + ".part", out)
        got += 1
        if got % 100 == 0:
            print(f"{got} files from {args.url}", file=sys.stderr)
    print(f"{len(members)} chosen, {got} new from {args.url}", file=sys.stderr)


if __name__ == "__main__":
    main()
