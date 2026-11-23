#!/usr/bin/env python3
"""Camera simulator for testing without the hardware.

Listens on port 5000, speaks the protocol in docs/PROTOCOL.md, logs every
message and saves each received print as a PNG.

Usage:
  python3 tools/fake_camera.py [--port 5000] [--fail-first] [--out DIR]
  KAMINONEKO_IP=127.0.0.1 cargo run --release -- print some_image.png
"""
import argparse
import socket
import struct
import threading
from datetime import datetime
from pathlib import Path

from PIL import Image


def frame(cmd, value):
    return struct.pack(">IBI", 5, cmd, value)


# Greeting observed from the real camera (status 0x02 omitted).
GREETING = (frame(0x03, 1) + frame(0x06, 0) + frame(0x04, 0) + frame(0x5d, 80)
            + struct.pack(">IB", 9, 0x0b) + b"\x00didicam"
            + frame(0x0c, 2) + frame(0x0d, 2) + frame(0x0f, 80) + frame(0x1d, 0x5601))


class Session(threading.Thread):
    count = 0
    lock = threading.Lock()

    def __init__(self, conn, args):
        super().__init__(daemon=True)
        self.conn, self.args = conn, args
        with Session.lock:
            Session.count += 1
            self.n = Session.count

    def log(self, msg):
        print(f"[{datetime.now():%H:%M:%S} #{self.n}] {msg}", flush=True)

    def run(self):
        c = self.conn
        try:
            c.sendall(GREETING)
        except OSError:
            return
        buf, pending, pixels, header, mode = b"", 0, b"", None, 2
        try:
            while True:
                d = c.recv(65536)
                if not d:
                    self.log("client disconnected")
                    break
                buf += d
                while buf:
                    if pending:
                        take = min(pending, len(buf))
                        pixels += buf[:take]
                        buf, pending = buf[take:], pending - take
                        if pending == 0:
                            self.finish_print(header, pixels)
                            pixels = b""
                        continue
                    if len(buf) < 4:
                        break
                    n = struct.unpack(">I", buf[:4])[0]
                    if len(buf) < 4 + n:
                        break
                    p, buf = buf[4:4 + n], buf[4 + n:]
                    if p == b"\x00\x00\x00\x00":
                        self.log("<- heartbeat")
                        continue
                    self.log(f"<- {p.hex(' ')[:110]}")
                    if p[0] == 0x0d:
                        if len(p) == 5:
                            mode = p[4]
                        c.sendall(frame(0x0d, mode))
                    elif p[0] == 0x1c and len(p) >= 18:
                        header = struct.unpack(">IIII", p[2:18])
                        pending = header[3]
                        self.log(f"   print header: field=0x{header[0]:x} {header[1]}x{header[2]}, {pending} bytes")
        except OSError as e:
            self.log(f"error: {e}")
        c.close()

    def finish_print(self, header, pixels):
        _, w, h, size = header
        gray = bytes(max(0, min(255, round((v - 16) * 255 / 219))) for v in pixels)
        out = Path(self.args.out)
        out.mkdir(parents=True, exist_ok=True)
        png = out / f"fake_print_{datetime.now():%H%M%S_%f}.png"
        Image.frombytes("L", (w, h), gray).save(png)
        result = 1 if self.args.fail_first and not getattr(Session, "failed", False) else 0
        Session.failed = True
        self.log(f"   received {len(pixels)} px (luma {min(pixels)}..{max(pixels)}) -> {png}; answering 1c={result}")
        self.conn.sendall(frame(0x1c, result))


def main():
    ap = argparse.ArgumentParser(description="Fake camera for testing.")
    ap.add_argument("--host", default="127.0.0.1")
    ap.add_argument("--port", type=int, default=5000)
    ap.add_argument("--fail-first", action="store_true", help="answer 1 to the first print (tests the retry)")
    ap.add_argument("--out", default=str(Path(__file__).resolve().parent.parent / "captures" / "fake_prints"))
    args = ap.parse_args()

    srv = socket.socket()
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind((args.host, args.port))
    srv.listen()
    print(f"Fake camera listening on {args.host}:{args.port} (Ctrl+C to stop)", flush=True)
    try:
        while True:
            conn, _ = srv.accept()
            Session(conn, args).start()
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    main()
