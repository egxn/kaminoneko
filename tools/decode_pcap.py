#!/usr/bin/env python3
"""Decodes a traffic capture of the DiDi Cam app talking to the camera.

Lists every TCP/UDP flow, decodes the port 5000 messages in both directions and
rebuilds every printed image as a PNG. Standard library + Pillow only (no tshark).

Usage:
  python3 tools/decode_pcap.py captures/PCAPdroid_xxx.pcap [--out DIR] [--all-flows]

Supports pcap files with raw IP (PCAPdroid), Ethernet and Linux cooked (SLL) link types.
"""
import argparse
import struct
from collections import defaultdict
from pathlib import Path

from PIL import Image

CAMERA_PORT = 5000


def read_pcap(path):
    d = Path(path).read_bytes()
    magic = d[:4]
    if magic in (b"\xd4\xc3\xb2\xa1", b"\x4d\x3c\xb2\xa1"):
        end = "<"
    elif magic in (b"\xa1\xb2\xc3\xd4", b"\xa1\xb2\x3c\x4d"):
        end = ">"
    else:
        raise SystemExit("Not a pcap file (pcapng is not supported; convert it with editcap -F pcap).")
    nano = magic in (b"\x4d\x3c\xb2\xa1", b"\xa1\xb2\x3c\x4d")
    linktype = struct.unpack(end + "I", d[20:24])[0]
    skip = {101: 0, 228: 0, 1: 14, 113: 16}.get(linktype)
    if skip is None:
        raise SystemExit(f"Unsupported link type {linktype}")
    off = 24
    while off + 16 <= len(d):
        ts, frac, incl, _ = struct.unpack(end + "IIII", d[off:off + 16])
        off += 16
        yield ts + frac / (1e9 if nano else 1e6), d[off + skip:off + incl]
        off += incl


def parse_packet(pkt):
    if not pkt:
        return None
    version = pkt[0] >> 4
    if version == 4:
        ihl = (pkt[0] & 15) * 4
        proto = pkt[9]
        src, dst = ".".join(map(str, pkt[12:16])), ".".join(map(str, pkt[16:20]))
        l4 = pkt[ihl:struct.unpack(">H", pkt[2:4])[0]]
    elif version == 6:
        proto = pkt[6]
        src, dst = pkt[8:24].hex(), pkt[24:40].hex()
        l4 = pkt[40:]
    else:
        return None
    if proto == 6 and len(l4) >= 20:
        sport, dport, seq = struct.unpack(">HHI", l4[:8])
        return "TCP", src, sport, dst, dport, seq, l4[13], l4[(l4[12] >> 4) * 4:]
    if proto == 17 and len(l4) >= 8:
        sport, dport = struct.unpack(">HH", l4[:4])
        return "UDP", src, sport, dst, dport, 0, 0, l4[8:]
    return None


def build_flows(path):
    flows = defaultdict(lambda: {"packets": 0, "segments": [], "t0": None, "t1": None, "isn": 0})
    for t, pkt in read_pcap(path):
        p = parse_packet(pkt)
        if not p:
            continue
        proto, src, sport, dst, dport, seq, flags, data = p
        f = flows[(proto, src, sport, dst, dport)]
        f["packets"] += 1
        f["t0"] = f["t0"] if f["t0"] is not None else t
        f["t1"] = t
        if flags & 0x02:  # SYN
            f["isn"] = seq
        if data:
            f["segments"].append((t, seq, data))
    start = min(f["t0"] for f in flows.values())
    result = {}
    for key, f in flows.items():
        if key[0] == "TCP":
            # Reassemble by sequence number, dropping retransmissions.
            seen, parts = set(), []
            for t, seq, data in sorted(f["segments"], key=lambda x: (x[1] - f["isn"]) & 0xFFFFFFFF):
                if seq not in seen:
                    seen.add(seq)
                    parts.append((t, data))
        else:
            parts = [(t, data) for t, _, data in f["segments"]]
        times = []
        for t, data in parts:
            times += [t - start] * len(data)
        result[key] = {"packets": f["packets"], "t0": f["t0"] - start, "t1": f["t1"] - start,
                       "stream": b"".join(d for _, d in parts), "times": times}
    return result


def decode_to_camera(stream, times, out_dir):
    """Phone -> camera: [len u32][cmd][seq][...]; a 0x1c print header is followed by raw luma."""
    off, images = 0, 0
    while off + 4 <= len(stream):
        n = struct.unpack(">I", stream[off:off + 4])[0]
        if n == 0 or n > 100_000 or off + 4 + n > len(stream):
            print(f"{times[off]:8.2f}s  UNPARSED @{off}: {stream[off:off + 48].hex(' ')}")
            return
        payload, t = stream[off + 4:off + 4 + n], times[off]
        off += 4 + n
        if payload == b"\x00\x00\x00\x00":
            label = "heartbeat"
        elif payload[0] == 0x0d:
            label = "query mode" if len(payload) == 4 else f"set mode {payload[-1]}"
        elif payload[0] == 0x1c:
            label = "print"
        else:
            label = ""
        print(f"{t:8.2f}s  -> {payload.hex(' ')[:110]}  {label}")
        if payload[0] == 0x1c and len(payload) >= 18:
            magic, w, h, size = struct.unpack(">IIII", payload[2:18])
            data = stream[off:off + size]
            off += size
            images += 1
            # luma 16..235 -> gray 0..255
            gray = bytes(max(0, min(255, round((v - 16) * 255 / 219))) for v in data)
            png = out_dir / f"print_{images}.png"
            Image.frombytes("L", (w, h), gray).save(png)
            print(f"            print #{images}: {w}x{h}, {len(data)} bytes, luma {min(data)}..{max(data)} "
                  f"(field 0x{magic:x}) -> {png}")


def decode_from_camera(stream, times):
    """Camera -> phone: [len u32][cmd][value u32]."""
    names = {0x5d: "battery?", 0x0d: "mode", 0x1c: "print result", 0x0b: "name"}
    off = 0
    while off + 4 <= len(stream):
        n = struct.unpack(">I", stream[off:off + 4])[0]
        if off + 4 + n > len(stream):
            break
        fr, t = stream[off + 4:off + 4 + n], times[off]
        off += 4 + n
        value = int.from_bytes(fr[1:5], "big") if len(fr) == 5 else fr[1:].hex(" ")
        print(f"{t:8.2f}s  <- cmd 0x{fr[0]:02x} = {value}  {names.get(fr[0], '')}")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("pcap")
    ap.add_argument("--out", help="folder for the rebuilt prints (default: captures/decoded_<name>)")
    ap.add_argument("--all-flows", action="store_true", help="list every flow, not only the camera's")
    a = ap.parse_args()

    flows = build_flows(a.pcap)
    print("===== Flows")
    for key in sorted(flows, key=lambda k: flows[k]["t0"]):
        f = flows[key]
        if a.all_flows or CAMERA_PORT in (key[2], key[4]):
            print(f"{f['t0']:8.2f}-{f['t1']:8.2f}s  {key[0]} {key[1]}:{key[2]} -> {key[3]}:{key[4]}  "
                  f"packets={f['packets']}  bytes={len(f['stream'])}")

    out_dir = Path(a.out) if a.out else Path(__file__).resolve().parent.parent / "captures" / f"decoded_{Path(a.pcap).stem}"
    out_dir.mkdir(parents=True, exist_ok=True)
    for key, f in sorted(flows.items(), key=lambda kv: kv[1]["t0"]):
        if key[0] != "TCP" or not f["stream"]:
            continue
        if key[4] == CAMERA_PORT:
            print(f"\n===== Phone -> camera ({key[1]}:{key[2]})")
            decode_to_camera(f["stream"], f["times"], out_dir)
        elif key[2] == CAMERA_PORT:
            print(f"\n===== Camera -> phone ({key[3]}:{key[4]})")
            decode_from_camera(f["stream"], f["times"])


if __name__ == "__main__":
    main()
