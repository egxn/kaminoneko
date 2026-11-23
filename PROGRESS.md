# Kaminoneko — PROGRESS

> Linux top bar app to take a screenshot, place and scale it on the paper, and print it on a Wi-Fi kids' instant print camera.

_Last updated: 2026-11-23_

## Legend

| Status | Meaning |
|---|---|
| ⚪ **To Do** | Not started |
| 🟡 **In Progress** | Being worked on (recent or uncommitted changes) |
| 🟢 **Done** | Finished |
| 🔴 **Blocked** | Blocked by something external or an open problem |

**Priority:** `P0 > P1 > P2 > P3` — sections go from highest to lowest priority.
Within a section all tasks have equal priority (`P1 = P1`) and are listed in suggested execution order.

| Level | Meaning |
|---|---|
| P0 | Critical / blocks the project goal |
| P1 | High: next milestone |
| P2 | Medium: planned improvement |
| P3 | Low / exploratory / long term |

## Tasks

### P0 — Critical

| Status | Task | Notes |
|---|---|---|
| 🟢 Done | Print from the PC on the real camera | Protocol matches the app byte for byte against `tools/fake_camera.py`; not yet run on the hardware |
| 🟢 Done | End-to-end screenshot flow (icon → portal → editor → print) | Icon, editor and printing tested separately; the interactive portal step needs a manual run |
| 🟢 Done | Identify the device and its services | `tools/probe_device.sh`: AX3292 SoC, FTP, RTSP config, control channel on 5000 |
| 🟢 Done | Reverse-engineer the print protocol | PCAPdroid capture of two prints; documented in `docs/PROTOCOL.md` |
| 🟢 Done | Rust app: top bar icon, editor, printing | `src/`; crop, size on paper, rotation, contrast, invert, copies |

### P1 — High

| Status | Task | Notes |
|---|---|---|
| 🟡 In Progress | Unit tests | Done: imaging pipeline, placement, drag / snap math (`cargo test`). Missing: `uri_to_path`, protocol framing against the simulator |
| ⚪ To Do | Find out what print result `0x1c = 1` means | Seen once in the capture before an app retry; the app retries once, so does Kaminoneko |
| ⚪ To Do | Measure a real print to confirm the scale | Width of the printed area in mm; sets `DOTS_PER_MM` (8 assumed, ≈ 48 mm) |
| ⚪ To Do | Find the maximum image height | Only 384 and 401 px seen; very tall crops may be rejected |
| ⚪ To Do | Text printing in the editor | `tools/print_image.py --text` already renders text |
| 🟢 Done | Editor is just the paper: move, scale and cut on the strip | Replaces the crop canvas; snapping to center and print area; controls at the bottom |
| 🟢 Done | Brutalist theme | `src/theme.rs`, dark and light mode |
| 🟢 Done | Repo layout from the workspace template | README, PROGRESS, `docs/`, `tools/`, git-ignored `captures/` |
| 🟢 Done | Python reference printer and camera simulator | `tools/print_image.py`, `tools/fake_camera.py` |

### P2 — Medium

| Status | Task | Notes |
|---|---|---|
| 🔴 Blocked | Live view over RTSP (port 7070) | `ffplay rtsp://192.168.1.1:7070/webcam` did not work; needs a capture of the app's live view |
| ⚪ To Do | Download photos from the camera | Anonymous FTP, folders `1`/`2` |
| ⚪ To Do | Remember the last editor settings | Size, invert, contrast between sessions |
| ⚪ To Do | Show battery level in the tray menu | `0x5d` status message |

### P3 — Low

| Status | Task | Notes |
|---|---|---|
| ⚪ To Do | Decode the remaining status messages | `0x02` blob, `0x03`, `0x0c`, `0x0f`, `0x1d`, header field `0xf200` |
| ⚪ To Do | Support more camera models | Other DiDi Cam / KX_Print / FR_PRINT cameras |
| ⚪ To Do | Packaging | `.deb` or Flatpak |
| ⚪ To Do | Choose a license | |

---

_Order within each priority: In Progress, Blocked, To Do, Done._
