//! Top bar icon and screenshots through the desktop portal.

use std::path::{Path, PathBuf};
use std::process::Command;

use ashpd::desktop::file_chooser::{FileFilter, SelectedFiles};
use ashpd::desktop::screenshot::Screenshot;
use ksni::TrayMethods;
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

enum Action {
    Capture,
    Open,
}

struct Tray {
    tx: UnboundedSender<Action>,
}

impl ksni::Tray for Tray {
    fn id(&self) -> String {
        "kaminoneko".into()
    }

    fn title(&self) -> String {
        "Kaminoneko".into()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        vec![icon()]
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        let _ = self.tx.send(Action::Capture);
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::*;
        vec![
            StandardItem {
                label: "Take screenshot…".into(),
                icon_name: "camera-photo".into(),
                activate: Box::new(|t: &mut Self| {
                    let _ = t.tx.send(Action::Capture);
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Print an image…".into(),
                icon_name: "document-open".into(),
                activate: Box::new(|t: &mut Self| {
                    let _ = t.tx.send(Action::Open);
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Quit".into(),
                icon_name: "application-exit".into(),
                activate: Box::new(|_| std::process::exit(0)),
                ..Default::default()
            }
            .into(),
        ]
    }
}

/// Held for as long as the top bar icon runs, so a second launch can tell.
pub struct InstanceLock {
    _file: Option<std::fs::File>,
}

/// `None` when another top bar icon is already running. If the lock file cannot be
/// created, runs anyway.
pub fn lock_instance() -> Option<InstanceLock> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    lock_at(&dir.join("kaminoneko.lock"))
}

fn lock_at(path: &Path) -> Option<InstanceLock> {
    let file = std::fs::OpenOptions::new().create(true).write(true).truncate(false).open(path);
    let Ok(file) = file else { return Some(InstanceLock { _file: None }) };
    match file.try_lock() {
        Err(std::fs::TryLockError::WouldBlock) => None,
        _ => Some(InstanceLock { _file: Some(file) }),
    }
}

pub async fn run() -> Result<(), String> {
    let (tx, mut rx) = unbounded_channel();
    let _handle = Tray { tx }
        .spawn()
        .await
        .map_err(|e| format!("Could not create the top bar icon: {e}"))?;
    while let Some(action) = rx.recv().await {
        let result = match action {
            Action::Capture => capture().await,
            Action::Open => pick_image().await,
        };
        match result {
            Ok(Some(path)) => open_editor(&path),
            Ok(None) => {}
            Err(e) => notify(&e),
        }
    }
    Ok(())
}

/// Opens GNOME's screenshot tool (area, window or whole screen).
pub async fn capture() -> Result<Option<PathBuf>, String> {
    let request = Screenshot::request()
        .interactive(true)
        .modal(true)
        .send()
        .await
        .map_err(|e| format!("Could not request the screenshot: {e}"))?;
    match request.response() {
        Ok(r) => uri_to_path(r.uri().as_str()).map(Some),
        Err(ashpd::Error::Response(_)) => Ok(None), // cancelled
        Err(e) => Err(format!("The screenshot failed: {e}")),
    }
}

pub async fn pick_image() -> Result<Option<PathBuf>, String> {
    let filter = ["image/png", "image/jpeg", "image/webp", "image/bmp", "image/gif"]
        .into_iter()
        .fold(FileFilter::new("Images"), |f, m| f.mimetype(m));
    let request = SelectedFiles::open_file()
        .title("Choose an image to print")
        .accept_label("Open")
        .modal(true)
        .filter(filter)
        .send()
        .await
        .map_err(|e| format!("Could not open the file chooser: {e}"))?;
    match request.response() {
        Ok(selection) => match selection.uris().first() {
            Some(u) => uri_to_path(u.as_str()).map(Some),
            None => Ok(None),
        },
        Err(ashpd::Error::Response(_)) => Ok(None),
        Err(e) => Err(format!("The file chooser failed: {e}")),
    }
}

fn uri_to_path(uri: &str) -> Result<PathBuf, String> {
    let rest = uri.strip_prefix("file://").ok_or_else(|| format!("Unsupported URI: {uri}"))?;
    let b = rest.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) = u8::from_str_radix(&rest[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    Ok(PathBuf::from(String::from_utf8_lossy(&out).into_owned()))
}

fn open_editor(path: &Path) {
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("kaminoneko"));
    match Command::new(exe).arg("edit").arg(path).spawn() {
        Ok(mut child) => {
            std::thread::spawn(move || child.wait());
        }
        Err(e) => notify(&format!("Could not open the editor: {e}")),
    }
}

pub fn notify(msg: &str) {
    eprintln!("{msg}");
    let _ = Command::new("notify-send").args(["-a", "Kaminoneko", "Kaminoneko", msg]).spawn();
}

/// 64×64 white cat icon (ARGB) drawn with supersampling.
fn icon() -> ksni::Icon {
    const N: usize = 64;
    const SS: usize = 4;
    let mut data = Vec::with_capacity(N * N * 4);
    for y in 0..N {
        for x in 0..N {
            let mut covered = 0;
            for sy in 0..SS {
                for sx in 0..SS {
                    let px = (x as f32 + (sx as f32 + 0.5) / SS as f32) / N as f32;
                    let py = (y as f32 + (sy as f32 + 0.5) / SS as f32) / N as f32;
                    if cat(px, py) {
                        covered += 1;
                    }
                }
            }
            data.extend_from_slice(&[(covered * 255 / (SS * SS)) as u8, 255, 255, 255]);
        }
    }
    ksni::Icon { width: N as i32, height: N as i32, data }
}

fn cat(x: f32, y: f32) -> bool {
    fn in_triangle(p: (f32, f32), a: (f32, f32), b: (f32, f32), c: (f32, f32)) -> bool {
        let s = |p1: (f32, f32), p2: (f32, f32), p3: (f32, f32)| (p1.0 - p3.0) * (p2.1 - p3.1) - (p2.0 - p3.0) * (p1.1 - p3.1);
        let (d1, d2, d3) = (s(p, a, b), s(p, b, c), s(p, c, a));
        !((d1 < 0.0 || d2 < 0.0 || d3 < 0.0) && (d1 > 0.0 || d2 > 0.0 || d3 > 0.0))
    }
    let head = ((x - 0.5) / 0.42).powi(2) + ((y - 0.6) / 0.34).powi(2) <= 1.0;
    let ears = in_triangle((x, y), (0.10, 0.50), (0.16, 0.06), (0.46, 0.32))
        || in_triangle((x, y), (0.90, 0.50), (0.84, 0.06), (0.54, 0.32));
    let eye = |cx: f32| ((x - cx) / 0.065).powi(2) + ((y - 0.57) / 0.09).powi(2) <= 1.0;
    let nose = in_triangle((x, y), (0.45, 0.69), (0.55, 0.69), (0.5, 0.75));
    (head || ears) && !eye(0.34) && !eye(0.66) && !nose
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_instance_is_detected_until_the_first_exits() {
        let path = std::env::temp_dir().join(format!("kaminoneko-test-{}.lock", std::process::id()));
        let first = lock_at(&path);
        assert!(first.is_some());
        assert!(lock_at(&path).is_none(), "second launch must see the first one");
        drop(first);
        assert!(lock_at(&path).is_some(), "lock is released when the first one exits");
        let _ = std::fs::remove_file(path);
    }
}
