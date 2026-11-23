#!/usr/bin/env python3
"""Second-round probe of the kids' print camera ("Children's camera" / DiDi Cam).

Research history: this script was written *before* the app traffic was captured.
Its direct-print tests on ports 5001/5002 were guesses and do not work; the real
print protocol runs on port 5000 (see docs/PROTOCOL.md and tools/print_image.py).
It is still useful for the RTSP (7070), control-channel (5000) and FTP checks.

Before running it:
  1. Take one or two photos with the camera (used as a sample of its format).
  2. Put the camera on its "WiFi connection" screen.

Usage:
  sudo python3 tools/probe_camera.py    # recommended: also saves capture.pcap
  python3 tools/probe_camera.py         # without a traffic capture

Optional environment:
  IP=192.168.1.1                        camera IP
  CAMERA_WIFI="Children's camera d109"  Wi-Fi profile used when the camera is not reachable
  NO_PRINT=1                            do not send images to ports 5001/5002
"""
import ftplib
import io
import os
import re
import socket
import struct
import subprocess
import threading
import time
from datetime import datetime
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont, ImageOps

IP = os.environ.get("IP", "192.168.1.1")
CAMERA_WIFI = os.environ.get("CAMERA_WIFI", "Children's camera d109")
NO_PRINT = os.environ.get("NO_PRINT") == "1"
OUT = Path(__file__).resolve().parent.parent / "captures" / f"camera_test_{datetime.now():%Y%m%d_%H%M%S}"
OUT.mkdir(parents=True)
LOG = open(OUT / "summary.log", "a", buffering=1)
T0 = time.time()


# ------------------------------------------------------------------ helpers
def log(*a):
    text = " ".join(str(x) for x in a)
    for line in text.splitlines() or [""]:
        s = f"[{time.time() - T0:6.1f}s] {line}"
        print(s, flush=True)
        LOG.write(s + "\n")


def section(t):
    log("")
    log(f"===== {t} =====")


def hexdump(b, maxlen=256):
    b = b[:maxlen]
    rows = []
    for i in range(0, len(b), 16):
        ch = b[i:i + 16]
        asc = "".join(chr(c) if 32 <= c < 127 else "." for c in ch)
        rows.append(f"  {i:04x}: {ch.hex(' '):<47}  {asc}")
    return "\n".join(rows) or "  (empty)"


def _s(x):
    return x.decode(errors="replace") if isinstance(x, bytes) else (x or "")


def run(cmd, timeout=30):
    try:
        r = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout)
        return r.returncode, (r.stdout + r.stderr).strip()
    except subprocess.TimeoutExpired as e:
        return -1, f"(cut off after {timeout}s) {_s(e.stdout)}{_s(e.stderr)}".strip()
    except FileNotFoundError:
        return -2, f"({cmd[0]} is not installed)"


def read_for(sock, seconds):
    """Reads everything that arrives during `seconds`."""
    sock.settimeout(0.5)
    data, end = b"", time.time() + seconds
    while time.time() < end:
        try:
            d = sock.recv(65536)
        except socket.timeout:
            continue
        except OSError as e:
            data += f"\n(error: {e})".encode()
            break
        if not d:
            data += b"\n(the camera closed the connection)"
            break
        data += d
    return data


def reachable(port=5000, timeout=2):
    try:
        socket.create_connection((IP, port), timeout=timeout).close()
        return True
    except OSError:
        return False


def go_to_camera():
    """Switches to the camera Wi-Fi if needed. Returns the previous profile, or None."""
    if reachable():
        return None
    active = run(["nmcli", "-t", "-f", "NAME,TYPE", "con", "show", "--active"])[1]
    previous = next((l.rsplit(":", 1)[0] for l in active.splitlines() if l.endswith(":802-11-wireless")), None)
    log(f"Switching to Wi-Fi '{CAMERA_WIFI}'...")
    run(["nmcli", "con", "up", "id", CAMERA_WIFI], timeout=45)
    for _ in range(20):
        if reachable():
            break
        time.sleep(1)
    return previous


# --------------------------------------------------- control channel (5000)
class Control(threading.Thread):
    """Keeps the connection to port 5000 open and logs every message.

    Observed format: [length u32 big-endian][command 1 byte][payload].
    """

    def __init__(self):
        super().__init__(daemon=True)
        self.sock = socket.create_connection((IP, 5000), timeout=5)
        self.frames = []
        self.active = True
        self.file = open(OUT / "control_5000.log", "a", buffering=1)

    def _record(self, direction, fr):
        payload = fr[1:]
        text = payload.strip(b"\x00")
        asc = f"  '{text.decode(errors='replace')}'" if text and all(32 <= c < 127 for c in text) and len(text) > 2 else ""
        line = f"[5000] {direction} cmd=0x{fr[0]:02x} payload={payload.hex(' ')}{asc}"
        self.file.write(f"{time.time() - T0:7.1f}s {line}\n")
        log(line)

    def run(self):
        buf = b""
        self.sock.settimeout(1)
        while self.active:
            try:
                d = self.sock.recv(4096)
            except socket.timeout:
                continue
            except OSError as e:
                if self.active:
                    log(f"[5000] error: {e}")
                break
            if not d:
                log("[5000] the camera closed the control connection")
                break
            buf += d
            while len(buf) >= 4:
                n = struct.unpack(">I", buf[:4])[0]
                if n == 0 or n > 1_000_000:
                    log("[5000] data that does not follow the format:\n" + hexdump(buf))
                    buf = b""
                    break
                if len(buf) < 4 + n:
                    break
                fr, buf = buf[4:4 + n], buf[4 + n:]
                self.frames.append(fr)
                self._record("RX", fr)
        self.active = False

    def send(self, fr):
        self.sock.sendall(struct.pack(">I", len(fr)) + fr)
        self._record("TX", fr)

    def stop(self):
        self.active = False
        try:
            self.sock.close()
        except OSError:
            pass


# -------------------------------------------------------------- RTSP (7070)
def raw_rtsp(method, url, proto="RTSP/1.0", extra=""):
    req = f"{method} {url} {proto}\r\nCSeq: 1\r\nUser-Agent: LibVLC/3.0.20\r\n{extra}\r\n"
    try:
        with socket.create_connection((IP, 7070), timeout=5) as s:
            s.sendall(req.encode())
            data = read_for(s, 4)
    except OSError as e:
        data = f"(error: {e})".encode()
    log(f">>> {req.splitlines()[0]}")
    log(data.decode(errors="replace").strip() or "(no answer)")
    return data


def rtsp_tests(label):
    section(f"RTSP on 7070 ({label})")
    for path in ("/webcam", "/file", "/"):
        url = f"rtsp://{IP}:7070{path}"
        raw_rtsp("OPTIONS", url)
        raw_rtsp("DESCRIBE", url, extra="Accept: application/sdp\r\n")
    # The 7070 parser also accepts a custom "SSID/" protocol
    raw_rtsp("OPTIONS", "*", proto="SSID/1.0")
    raw_rtsp("GET", "/webcam", proto="SSID/1.0")
    raw_rtsp("DESCRIBE", f"rtsp://{IP}:7070/webcam", proto="SSID/1.0")

    ok = False
    for path in ("/webcam", "/file"):
        for transport in ("tcp", "udp"):
            url = f"rtsp://{IP}:7070{path}"
            rc, out = run(["ffprobe", "-hide_banner", "-rtsp_transport", transport, "-i", url], timeout=20)
            log(f">>> ffprobe {transport} {url} -> exit {rc}")
            log(out[-1500:])
            if rc == 0 and path == "/webcam":
                ok = True
                frame = OUT / f"webcam_{label}.jpg"
                run(["ffmpeg", "-hide_banner", "-y", "-rtsp_transport", transport, "-i", url,
                     "-frames:v", "1", str(frame)], timeout=25)
                run(["ffmpeg", "-hide_banner", "-y", "-rtsp_transport", transport, "-i", url,
                     "-t", "8", "-c", "copy", str(OUT / f"webcam_{label}.mkv")], timeout=30)
                log(f"VIDEO OK over {transport}. Frame: {frame}")
                break
        if ok:
            break
    return ok


# --------------------------------------------------------------- test images
def font(size):
    try:
        return ImageFont.load_default(size=size)
    except Exception:
        return ImageFont.load_default()


def test_image(label, w, h):
    img = Image.new("L", (w, h), 255)
    d = ImageDraw.Draw(img)
    d.rectangle([2, 2, w - 3, h - 3], outline=0, width=max(3, w // 120))
    d.text((w // 2, int(h * 0.30)), f"TEST {label}", fill=0, font=font(max(28, h // 5)), anchor="mm")
    d.text((w // 2, int(h * 0.55)), "Hello from the PC", fill=0, font=font(max(16, h // 12)), anchor="mm")
    # gray bars to see how tones print
    n, y0, y1 = 8, int(h * 0.72), int(h * 0.90)
    for i in range(n):
        x0 = int(w * 0.06 + i * w * 0.88 / n)
        x1 = int(w * 0.06 + (i + 1) * w * 0.88 / n)
        d.rectangle([x0, y0, x1, y1], fill=int(255 * i / (n - 1)))
    return img


def to_jpeg(img):
    buf = io.BytesIO()
    img.convert("RGB").save(buf, "JPEG", quality=90, subsampling=2, progressive=False)
    return buf.getvalue()


def to_bits(img):
    """1-bit raster, 1 = black dot (like thermal printers)."""
    return ImageOps.invert(img.convert("L")).convert("1").tobytes()


# ----------------------------------------------------------------------- FTP
def ftp_tests():
    section("FTP: listing, camera photos and write access")
    try:
        ftp = ftplib.FTP(IP, timeout=15)
        log(ftp.getwelcome())
        ftp.login()
    except Exception as e:
        log(f"FTP not available: {e}")
        return None, (640, 480)
    for c in ("SYST", "FEAT", "PWD"):
        try:
            log(f"{c}: {ftp.sendcmd(c)}")
        except Exception as e:
            log(f"{c}: {e}")

    files = []

    def walk(path, depth):
        lines = []
        try:
            ftp.cwd(path)
            ftp.dir(lines.append)
        except Exception as e:
            log(f"LIST {path}: {e}")
            return
        log(f"LIST {path}: {len(lines)} entries")
        for l in lines:
            log("   " + l)
            parts = l.split(None, 8)
            if len(parts) < 9 or parts[8] in (".", ".."):
                continue
            full = path.rstrip("/") + "/" + parts[8]
            if l.startswith("d"):
                if depth < 3:
                    walk(full, depth + 1)
            else:
                files.append(full)

    walk("/", 0)
    jpgs = [f for f in files if f.lower().endswith(".jpg") and not f.startswith("/0/")]
    log(f"Photos found: {len(jpgs)}  |  other files: {len(files) - len(jpgs)}")

    size = (640, 480)
    photo_dir = OUT / "camera_photos"
    photo_dir.mkdir()
    for path in jpgs[:15]:
        target = photo_dir / path.strip("/").replace("/", "_")
        try:
            with open(target, "wb") as f:
                ftp.retrbinary(f"RETR {path}", f.write)
            im = Image.open(target)
            size = im.size
            log(f"{path}: {im.format} {im.size} {im.mode} progressive={bool(im.info.get('progressive'))} "
                f"bytes={target.stat().st_size}")
            log(hexdump(target.read_bytes()[:32]))
        except Exception as e:
            log(f"{path}: could not download/analyze ({e})")

    # Is it writable?
    folder = jpgs[-1].rsplit("/", 1)[0] if jpgs else "/1"
    try:
        ftp.cwd(folder)
        ftp.storbinary("STOR PCTEST.TXT", io.BytesIO(b"test from the PC\n"))
        log(f"Write to {folder}: OK")
        try:
            ftp.delete("PCTEST.TXT")
            log("Deleting the test file: OK")
        except Exception as e:
            log(f"Could not delete PCTEST.TXT: {e}")
    except Exception as e:
        log(f"Write to {folder}: FAILED ({e})")

    # F1: upload a photo with the same format and naming as the camera's own
    name = None
    for path in jpgs:
        m = re.match(r"(.*)/(.*?)(\d+)(\.jpg)$", path, re.I)
        if m:
            num = int(m.group(3)) + 1
            cand = (m.group(1), f"{m.group(2)}{num:0{len(m.group(3))}d}{m.group(4)}")
            if name is None or cand[1] > name[1]:
                name = cand
    if name is None:
        name = (folder, "PICT0001.JPG")
    try:
        ftp.cwd(name[0])
        ftp.storbinary(f"STOR {name[1]}", io.BytesIO(to_jpeg(test_image("F1", *size))))
        log(f"F1 uploaded as {name[0]}/{name[1]} ({size[0]}x{size[1]})")
    except Exception as e:
        log(f"F1 could not be uploaded: {e}")
    try:
        ftp.quit()
    except Exception:
        pass
    return name, size


# ------------------------------------------------------- direct image send
def send_image(port, label, data, desc):
    log(f"--- {label}: port {port}, {desc}, {len(data)} bytes ---")
    try:
        with socket.create_connection((IP, port), timeout=5) as s:
            greeting = read_for(s, 2)
            if greeting:
                log("Sent on connect:\n" + hexdump(greeting))
            s.settimeout(10)
            s.sendall(data)
            reply = read_for(s, 5)
            log("Reply:\n" + hexdump(reply) if reply else "No reply")
    except OSError as e:
        log(f"Error: {e}")
    time.sleep(8)  # time to print


# ---------------------------------------------------------------------- main
def main():
    previous = go_to_camera()
    section("Network")
    log(run(["nmcli", "-t", "-f", "ACTIVE,SSID,BSSID,SIGNAL", "dev", "wifi"])[1].split("\n")[0])
    if not reachable(timeout=4):
        log(f"!! {IP} does not answer on 5000. Are you on the camera's Wi-Fi?")
        return

    tcpdump = None
    if os.geteuid() == 0:
        m = re.search(r"dev (\S+)", run(["ip", "route", "get", IP])[1])
        if m:
            pcap = OUT / "capture.pcap"
            tcpdump = subprocess.Popen(["tcpdump", "-i", m.group(1), "-U", "-Z", "root", "-w", str(pcap),
                                        "host", IP], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            log(f"Capturing traffic to {pcap}")
    else:
        log("(no sudo: no traffic capture)")

    ctrl = None
    try:
        video_without = rtsp_tests("without_control")

        section("Control channel 5000: listen for 15 s")
        ctrl = Control()
        ctrl.start()
        time.sleep(15)
        log(f"Messages received: {len(ctrl.frames)}. Still open? {ctrl.active}")

        video_with = rtsp_tests("with_control") if not video_without else True

        section("Control channel 5000: echo every status back unchanged")
        if ctrl.active:
            seen = {}
            for fr in ctrl.frames:
                seen.setdefault(fr[0], fr)
            for fr in seen.values():
                if not ctrl.active:
                    break
                ctrl.send(fr)
                time.sleep(1.5)
            time.sleep(4)
        else:
            log("The control connection was already closed; skipped.")

        f1_name, size = ftp_tests()

        if not NO_PRINT:
            section("Direct image sends (watch for paper)")
            for port, letter in ((5001, "A"), (5002, "B")):
                jpg = to_jpeg(test_image(f"{letter}1", 384, 240))
                send_image(port, f"{letter}1", jpg, "JPEG 384x240")
                send_image(port, f"{letter}2", struct.pack(">I", len(jpg)) + to_jpeg(test_image(f"{letter}2", 384, 240)),
                           "JPEG 384x240 prefixed with its u32 length")
                send_image(port, f"{letter}3", to_bits(test_image(f"{letter}3", 384, 240)),
                           "1-bit raster 384 px (48 bytes/line)")
                send_image(port, f"{letter}4", to_jpeg(test_image(f"{letter}4", *size)),
                           f"JPEG {size[0]}x{size[1]} (camera size)")

        section("Summary")
        log(f"Video without control: {'OK' if video_without else 'no'} | with control: {'OK' if video_with else 'no'}")
        if f1_name:
            log(f"F1: look for '{f1_name[1]}' in the camera gallery and print it from there.")
        log("A1-A4 = port 5001, B1-B4 = port 5002. Note which label got printed, if any.")
    finally:
        if ctrl:
            ctrl.stop()
        if tcpdump:
            time.sleep(1)
            tcpdump.terminate()
        uid, gid = os.environ.get("SUDO_UID"), os.environ.get("SUDO_GID")
        if uid and gid:
            for p in [OUT, *OUT.rglob("*")]:
                os.chown(p, int(uid), int(gid))
        if previous:
            section(f"Reconnecting to '{previous}'")
            log(run(["nmcli", "con", "up", "id", previous], timeout=45)[1])
        log(f"Results in: {OUT}")


if __name__ == "__main__":
    main()
