#!/usr/bin/env python3
"""Prints images or text on the kids' camera ("Children's camera" / DiDi Cam).

Minimal reference implementation of the protocol in docs/PROTOCOL.md; the Rust
app (`kaminoneko print IMAGE`) does the same thing.

Usage:
  python3 tools/print_image.py photo.jpg                 # one or more images
  python3 tools/print_image.py --text "Hello world"      # text (\\n allowed)
  python3 tools/print_image.py photo.jpg --save view.png # only render the preview

By default it switches to the camera's Wi-Fi on its own and returns to your network when done.
"""
import argparse
import os
import queue
import socket
import struct
import subprocess
import sys
import threading
import time

from PIL import Image, ImageDraw, ImageFont, ImageOps

PAPER_WIDTH = 384
PORT = 5000


# -------------------------------------------------------------------- image
def load_image(path):
    img = ImageOps.exif_transpose(Image.open(path))
    if img.mode in ("RGBA", "LA", "P"):
        img = img.convert("RGBA")
        background = Image.new("RGBA", img.size, "white")
        img = Image.alpha_composite(background, img)
    return img.convert("L")


def font(size):
    for f in ("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
              "/usr/share/fonts/TTF/DejaVuSans.ttf",
              "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf"):
        if os.path.exists(f):
            return ImageFont.truetype(f, size)
    return ImageFont.load_default(size=size)


def text_image(text, size=36, margin=12):
    f = font(size)
    usable = PAPER_WIDTH - 2 * margin
    lines = []
    for paragraph in text.replace("\\n", "\n").split("\n"):
        current = ""
        for word in paragraph.split(" "):
            attempt = f"{current} {word}".strip()
            if f.getlength(attempt) <= usable or not current:
                current = attempt
            else:
                lines.append(current)
                current = word
        lines.append(current)
    line_height = int(size * 1.25)
    img = Image.new("L", (PAPER_WIDTH, 2 * margin + line_height * len(lines)), 255)
    d = ImageDraw.Draw(img)
    for i, l in enumerate(lines):
        d.text((margin, margin + i * line_height), l, fill=0, font=f)
    return img


def prepare(img, contrast=True, rotate=False):
    if rotate or (rotate is None and img.width > img.height):
        img = img.rotate(90, expand=True)
    height = max(1, round(img.height * PAPER_WIDTH / img.width))
    img = img.resize((PAPER_WIDTH, height), Image.LANCZOS)
    if contrast:
        img = ImageOps.autocontrast(img, cutoff=1)
    return img


def to_luma(img):
    """0..255 -> 16..235, same as the app sends."""
    return img.point([16 + round(v * 219 / 255) for v in range(256)]).tobytes()


# ------------------------------------------------------------------- Wi-Fi
def nmcli(*args, timeout=45):
    r = subprocess.run(["nmcli", *args], capture_output=True, text=True, timeout=timeout)
    return r.returncode, (r.stdout + r.stderr).strip()


def reachable(ip, seconds=1.5):
    try:
        socket.create_connection((ip, PORT), timeout=seconds).close()
        return True
    except OSError:
        return False


def go_to_camera(ip, profile):
    """Returns the previous Wi-Fi profile if the network had to be switched, else None."""
    if reachable(ip):
        return None
    _, active = nmcli("-t", "-f", "NAME,TYPE", "con", "show", "--active")
    previous = next((l.rsplit(":", 1)[0] for l in active.splitlines() if l.endswith(":802-11-wireless")), None)
    print(f"Connecting to Wi-Fi '{profile}'...")
    rc, out = nmcli("con", "up", "id", profile)
    if rc != 0:
        sys.exit(f"Could not connect to '{profile}': {out}\nIs the camera on with Wi-Fi enabled?")
    for _ in range(20):
        if reachable(ip):
            return previous
        time.sleep(1)
    go_back(previous)
    sys.exit(f"Connected to '{profile}' but {ip}:{PORT} does not answer.")


def go_back(previous):
    if previous:
        print(f"Switching back to '{previous}'...")
        nmcli("con", "up", "id", previous)


# ----------------------------------------------------------------- protocol
class Camera:
    def __init__(self, ip, verbose=False):
        self.s = socket.create_connection((ip, PORT), timeout=10)
        self.verbose = verbose
        self.seq = 0x10
        self.queue = queue.Queue()
        self.state = {}
        self.last_heartbeat = 0
        self.lock = threading.Lock()
        threading.Thread(target=self._read, daemon=True).start()

    def _read(self):
        buf = b""
        self.s.settimeout(None)
        while True:
            try:
                d = self.s.recv(4096)
            except OSError:
                break
            if not d:
                break
            buf += d
            while len(buf) >= 4:
                n = struct.unpack(">I", buf[:4])[0]
                if len(buf) < 4 + n:
                    break
                fr, buf = buf[4:4 + n], buf[4 + n:]
                if not fr:
                    continue
                value = int.from_bytes(fr[1:5], "big") if len(fr) == 5 else fr[1:]
                self.state[fr[0]] = value
                if self.verbose:
                    print(f"  <- cmd 0x{fr[0]:02x} = {value}")
                self.queue.put((fr[0], value))
        self.queue.put((None, None))

    def _send(self, payload):
        with self.lock:
            self.s.sendall(struct.pack(">I", len(payload)) + payload)

    def heartbeat(self, force=False):
        if force or time.time() - self.last_heartbeat >= 5:
            self._send(b"\x00\x00\x00\x00")
            self.last_heartbeat = time.time()

    def command(self, cmd, rest=b"\x00\x00"):
        self.seq = (self.seq + 1) & 0xFF
        self._send(bytes([cmd, self.seq]) + rest)

    def wait_for(self, cmd, seconds):
        end = time.time() + seconds
        while time.time() < end:
            self.heartbeat()
            try:
                c, v = self.queue.get(timeout=0.5)
            except queue.Empty:
                continue
            if c is None:
                raise ConnectionError("the camera closed the connection")
            if c == cmd:
                return v
        return None

    def print(self, img, retries=1):
        data = to_luma(img)
        w, h = img.size
        for attempt in range(1 + retries):
            self.command(0x0d)                    # query mode
            self.wait_for(0x0d, 3)
            self.command(0x0d, b"\x00\x00\x03")   # print mode
            self.wait_for(0x0d, 3)
            header = struct.pack(">IIII", 0xF200, w, h, len(data)) + bytes(16)
            self.command(0x1c, header)
            with self.lock:
                self.s.sendall(data)
            r = self.wait_for(0x1c, 90)
            if r == 0:
                return True
            print(f"  the camera answered {r!r} (attempt {attempt + 1})")
            time.sleep(4)
        return False

    def close(self):
        try:
            self.s.close()
        except OSError:
            pass


# ---------------------------------------------------------------------- main
def main():
    ap = argparse.ArgumentParser(description="Prints on the kids' camera over Wi-Fi.")
    ap.add_argument("images", nargs="*", help="image files to print")
    ap.add_argument("--text", action="append", default=[], help="text to print (repeatable)")
    ap.add_argument("--size", type=int, default=36, help="text font size (36)")
    ap.add_argument("--no-contrast", action="store_true", help="do not auto-adjust contrast")
    ap.add_argument("--rotate", choices=["auto", "yes", "no"], default="auto",
                    help="rotate landscape photos 90° so they print larger (auto)")
    ap.add_argument("--save", metavar="PNG", help="only save the preview, do not print")
    ap.add_argument("--ip", default="192.168.1.1")
    ap.add_argument("--wifi", default="Children's camera d109", help="camera Wi-Fi profile")
    ap.add_argument("--no-wifi", action="store_true", help="do not switch networks automatically")
    ap.add_argument("-v", "--verbose", action="store_true", help="show the camera's messages")
    a = ap.parse_args()

    rotate = {"auto": None, "yes": True, "no": False}[a.rotate]
    jobs = [(f"text {i + 1}", prepare(text_image(t, a.size), not a.no_contrast, False))
            for i, t in enumerate(a.text)]
    jobs += [(p, prepare(load_image(p), not a.no_contrast, rotate)) for p in a.images]
    if not jobs:
        ap.error("give at least one image or --text")

    if a.save:
        imgs = [img for _, img in jobs]
        sheet = Image.new("L", (PAPER_WIDTH, sum(i.height for i in imgs) + 10 * (len(imgs) - 1)), 200)
        y = 0
        for i in imgs:
            sheet.paste(i, (0, y))
            y += i.height + 10
        sheet.save(a.save)
        print(f"Preview saved to {a.save}")
        return

    previous = None if a.no_wifi else go_to_camera(a.ip, a.wifi)
    try:
        cam = Camera(a.ip, a.verbose)
        cam.wait_for(0x1d, 3)  # last message of the initial greeting
        battery = cam.state.get(0x5d)
        if isinstance(battery, int):
            print(f"Connected. Battery: {battery}%")
        ok = 0
        for name, img in jobs:
            print(f"Printing {name} ({img.width}x{img.height})...", flush=True)
            if cam.print(img):
                ok += 1
                print("  done")
            else:
                print("  could not print")
        cam.close()
        print(f"{ok}/{len(jobs)} prints succeeded")
    finally:
        go_back(previous)


if __name__ == "__main__":
    main()
