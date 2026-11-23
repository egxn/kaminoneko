//! Print protocol of the kids' camera (TCP port 5000).
//!
//! Every message is `[length u32 BE][payload]`.
//! - Camera -> PC: `[cmd][value u32]` (0x5d battery, 0x0d mode, 0x1c print result: 0 = done).
//! - PC -> camera: heartbeat `00 00 00 00` every 5 s; `0d seq 00 00` queries the mode;
//!   `0d seq 00 00 03` sets print mode; `1c seq [0xf200][width][height][size] + 16 zero bytes`
//!   followed by width*height luma bytes (16 = black .. 235 = white).

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::process::Command;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use image::GrayImage;

/// Dots per printed line: the width of every image sent to the camera.
pub const PAPER_WIDTH: u32 = 384;
/// Width of the paper roll, in mm (measured).
pub const PAPER_WIDTH_MM: f32 = 66.0;
/// Print head resolution. 8 dots/mm (203 dpi) is the usual for 384-dot heads, so the
/// print is ≈ 48 mm wide and centered on the roll. Not measured yet on a real print.
pub const DOTS_PER_MM: f32 = 8.0;
const PORT: u16 = 5000;

#[derive(Clone)]
pub struct Config {
    pub ip: String,
    pub wifi_profile: String,
}

impl Config {
    pub fn from_env() -> Self {
        Self {
            ip: std::env::var("KAMINONEKO_IP").unwrap_or_else(|_| "192.168.1.1".into()),
            wifi_profile: std::env::var("KAMINONEKO_WIFI")
                .unwrap_or_else(|_| "Children's camera d109".into()),
        }
    }
}

fn to_string<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

// ------------------------------------------------------------------ Wi-Fi
fn reachable(addr: &SocketAddr, timeout: Duration) -> bool {
    TcpStream::connect_timeout(addr, timeout).is_ok()
}

fn nmcli(args: &[&str]) -> Result<String, String> {
    let out = Command::new("nmcli")
        .args(args)
        .output()
        .map_err(|e| format!("could not run nmcli: {e}"))?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    if out.status.success() {
        Ok(text)
    } else {
        Err(text.trim().to_string())
    }
}

/// Switches to the camera's Wi-Fi when needed and knows how to go back to the previous network.
struct Network {
    previous: Option<String>,
    switched: bool,
}

impl Network {
    fn connect(cfg: &Config, addr: &SocketAddr, notify: &dyn Fn(String)) -> Result<Network, String> {
        let mut net = Network { previous: None, switched: false };
        if reachable(addr, Duration::from_millis(1500)) {
            return Ok(net);
        }
        let active = nmcli(&["-t", "-f", "NAME,TYPE", "con", "show", "--active"]).unwrap_or_default();
        net.previous = active
            .lines()
            .find_map(|l| l.strip_suffix(":802-11-wireless"))
            .map(|s| s.replace("\\:", ":"));
        notify(format!("Connecting to Wi-Fi “{}”…", cfg.wifi_profile));
        nmcli(&["con", "up", "id", &cfg.wifi_profile]).map_err(|e| {
            format!("Could not connect to the camera's Wi-Fi (is it on with Wi-Fi enabled?): {e}")
        })?;
        net.switched = true;
        for _ in 0..20 {
            if reachable(addr, Duration::from_millis(1500)) {
                return Ok(net);
            }
            thread::sleep(Duration::from_secs(1));
        }
        Err(format!("Connected to the Wi-Fi, but the camera does not answer at {addr}"))
    }

    fn restore(&mut self) {
        if self.switched {
            self.switched = false;
            if let Some(p) = &self.previous {
                let _ = nmcli(&["con", "up", "id", p]);
            }
        }
    }
}

impl Drop for Network {
    fn drop(&mut self) {
        self.restore();
    }
}

// --------------------------------------------------------------- protocol
struct Camera {
    stream: TcpStream,
    rx: Receiver<(u8, u32)>,
    seq: u8,
    last_heartbeat: Instant,
    battery: Option<u32>,
}

impl Camera {
    fn connect(addr: &SocketAddr) -> Result<Self, String> {
        let stream = TcpStream::connect_timeout(addr, Duration::from_secs(5)).map_err(to_string)?;
        stream.set_nodelay(true).ok();
        stream.set_write_timeout(Some(Duration::from_secs(30))).ok();
        let mut reader = stream.try_clone().map_err(to_string)?;
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let mut buf: Vec<u8> = Vec::new();
            let mut chunk = [0u8; 4096];
            loop {
                let n = match reader.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                buf.extend_from_slice(&chunk[..n]);
                while buf.len() >= 4 {
                    let len = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
                    if buf.len() < 4 + len {
                        break;
                    }
                    let frame: Vec<u8> = buf.drain(..4 + len).skip(4).collect();
                    let msg = match frame.as_slice() {
                        [c, a, b, d, e] => (*c, u32::from_be_bytes([*a, *b, *d, *e])),
                        [c, ..] => (*c, u32::MAX),
                        [] => continue,
                    };
                    if tx.send(msg).is_err() {
                        return;
                    }
                }
            }
        });
        Ok(Self {
            stream,
            rx,
            seq: 0x10,
            last_heartbeat: Instant::now() - Duration::from_secs(10),
            battery: None,
        })
    }

    fn send(&mut self, payload: &[u8]) -> Result<(), String> {
        let mut msg = (payload.len() as u32).to_be_bytes().to_vec();
        msg.extend_from_slice(payload);
        self.stream.write_all(&msg).map_err(to_string)
    }

    fn heartbeat(&mut self) -> Result<(), String> {
        if self.last_heartbeat.elapsed() >= Duration::from_secs(5) {
            self.send(&[0, 0, 0, 0])?;
            self.last_heartbeat = Instant::now();
        }
        Ok(())
    }

    fn command(&mut self, cmd: u8, rest: &[u8]) -> Result<(), String> {
        self.seq = self.seq.wrapping_add(1);
        let mut payload = vec![cmd, self.seq];
        payload.extend_from_slice(rest);
        self.send(&payload)
    }

    fn wait_for(&mut self, cmd: u8, timeout: Duration) -> Result<Option<u32>, String> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            self.heartbeat()?;
            match self.rx.recv_timeout(Duration::from_millis(500)) {
                Ok((c, v)) => {
                    if c == 0x5d {
                        self.battery = Some(v);
                    }
                    if c == cmd {
                        return Ok(Some(v));
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return Err("the camera closed the connection".into()),
            }
        }
        Ok(None)
    }

    fn print(&mut self, img: &GrayImage) -> Result<Option<u32>, String> {
        let luma: Vec<u8> = img.as_raw().iter().map(|&v| (16 + (v as u32 * 219 + 127) / 255) as u8).collect();
        self.command(0x0d, &[0, 0])?;
        self.wait_for(0x0d, Duration::from_secs(3))?;
        self.command(0x0d, &[0, 0, 3])?;
        self.wait_for(0x0d, Duration::from_secs(3))?;
        let mut header = Vec::with_capacity(32);
        for v in [0xF200, img.width(), img.height(), luma.len() as u32] {
            header.extend_from_slice(&v.to_be_bytes());
        }
        header.extend_from_slice(&[0; 16]);
        self.command(0x1c, &header)?;
        self.stream.write_all(&luma).map_err(to_string)?;
        self.wait_for(0x1c, Duration::from_secs(90))
    }
}

/// Prints `img` (384 px wide, grayscale 0 = black .. 255 = white) `copies` times.
pub fn print(cfg: &Config, img: &GrayImage, copies: u32, notify: &dyn Fn(String)) -> Result<(), String> {
    let addr: SocketAddr = format!("{}:{}", cfg.ip, PORT).parse().map_err(|e| format!("Invalid IP: {e}"))?;
    let mut net = Network::connect(cfg, &addr, notify)?;

    let result = (|| {
        notify("Connecting to the camera…".into());
        let mut cam = Camera::connect(&addr)?;
        cam.wait_for(0x1d, Duration::from_secs(3))?; // end of the initial greeting
        for n in 1..=copies {
            let mut done = false;
            for attempt in 0..2 {
                let copy = if copies > 1 { format!(" copy {n} of {copies}") } else { String::new() };
                let battery = cam.battery.map(|b| format!(" (battery {b}%)")).unwrap_or_default();
                notify(format!("Printing{copy}…{battery}"));
                match cam.print(img)? {
                    Some(0) => {
                        done = true;
                        break;
                    }
                    Some(code) if attempt == 0 => {
                        notify(format!("The camera answered {code}; retrying…"));
                        thread::sleep(Duration::from_secs(4));
                    }
                    Some(code) => return Err(format!("The camera rejected the print (code {code})")),
                    None => return Err("The camera did not confirm the print within 90 s".into()),
                }
            }
            if !done {
                return Err("The camera rejected the print".into());
            }
        }
        Ok(())
    })();

    if net.switched {
        notify("Switching back to your Wi-Fi…".into());
    }
    net.restore();
    result
}
