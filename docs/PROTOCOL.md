# Camera Print Protocol

Reverse-engineered from a PCAPdroid capture of the DiDi Cam Android app printing
two images (see `tools/decode_pcap.py`). Everything here was observed on one
device; fields marked *unknown* are copied verbatim by Kaminoneko.

## Device

| Item | Value |
|---|---|
| Product | Kids' instant print camera, Wi-Fi SSID `Children's camera <id>` |
| Companion app | DiDi Cam (iOS / Android) |
| SoC | AppoTech AX3292 (firmware `AX3292.BIN`, compressed) |
| Camera IP | `192.168.1.1` (it is the access point and DHCP server) |
| Printer | Thermal, **384 px wide** (≈ 48 mm at 8 px/mm) |

### Open ports

| Port | Service | Notes |
|---|---|---|
| 21 | FTP | Anonymous login. Folder `0` holds the firmware and `CONFIG.DAT`; `1`/`2` are probably photos and videos |
| 80 | HTTP | Only answers `URL is err` |
| **5000** | **Control + print** | Everything in this document |
| 5001, 5002 | ? | Silent; direct image sends do nothing |
| 7070 | RTSP | `CONFIG.DAT` lists `/webcam` (live) and `/file` (playback) and a custom `SSID/` protocol; live view has not worked yet |

A fast full `nmap -p-` scan overwhelms the device and every port shows as filtered.

## Framing

A single TCP connection to port 5000. Every message in both directions is:

```text
[length: u32 big-endian][payload: length bytes]
```

Right after connecting, the camera sends its status messages (see below).

## Camera → client

Payload: `[cmd: u8][value: u32 BE]` (5 bytes), except where noted.

| cmd | Meaning | Observed values |
|---|---|---|
| `0x03` | *unknown* | `1` |
| `0x02` | *unknown*, 31-byte blob | `00 00 00 09 00 08 00 19 01 51 …` |
| `0x06` | *unknown* | `0` |
| `0x04` | *unknown* | `0` |
| `0x5d` | Battery level? Repeated every 5 s | `80`, `100` |
| `0x0b` | Device name (`00` + ASCII) | `didicam` |
| `0x0c` | *unknown* | `2` |
| `0x0d` | Mode | `2` (idle, before the app), `3` (print mode) |
| `0x0f` | Battery level? | `80`, `100` |
| `0x1d` | *unknown*; last message of the greeting | `0x5601` |
| `0x1c` | **Print result** | `0` = done; `1` = rejected (the app retried ~4 s later and then got `0`) |

## Client → camera

Payload: `[cmd: u8][seq: u8][...]`, where `seq` increases by one on every command
(the app started around `0x0e`; any start value works in the simulator).

| Message | Payload bytes |
|---|---|
| Heartbeat, every 5 s | `00 00 00 00` |
| Query mode | `0d <seq> 00 00` |
| Set print mode | `0d <seq> 00 00 03` |
| Print | `1c <seq>` + header (below), then the raw pixels **outside** the framing |

The camera answers each `0x0d` with its `0x0d` status.

### Print header

32 bytes after `1c <seq>`, all big-endian `u32`:

| Offset | Field | Observed |
|---|---|---|
| 0 | *unknown* | always `0x0000f200` |
| 4 | Width | `384` |
| 8 | Height | `401`, `384` (variable) |
| 12 | Pixel byte count | `width × height` |
| 16 | 16 zero bytes | |

### Pixel data

Sent right after the print message, without a length prefix:

- 1 byte per pixel, row by row, top to bottom.
- **Luma in limited range: 16 = black … 235 = white** (`printer-yuv` in the firmware config).
  Convert a 0–255 gray value with `16 + v × 219 / 255`.
- No dithering: the camera handles grayscale itself.

## Print sequence

```text
client                                   camera
  | ---- TCP connect :5000 --------------> |
  | <--- status greeting (… 0x1d) -------- |
  | ---- heartbeat (every 5 s) ----------> |
  | ---- 0d seq 00 00 -------------------> |
  | <--- 0x0d = 3 ------------------------ |
  | ---- 0d seq 00 00 03 ----------------> |
  | <--- 0x0d = 3 ------------------------ |
  | ---- 1c seq header ------------------> |
  | ---- width×height luma bytes --------> |
  |          (≈ 11 s of printing)          |
  | <--- 0x1c = 0 (done) / 1 (rejected) -- |
```

## Open questions

- What `0x1c = 1` means exactly (busy, overheated, out of paper, …).
- Maximum image height.
- Meaning of the `0xf200` header field and of the unknown status messages.
- How to start the RTSP live view on port 7070.
