//! Kaminoneko: top bar icon to take a screenshot and print it
//! on the kids' camera with a thermal printer.

mod editor;
mod imaging;
mod printer;
mod theme;
mod tray;

use std::path::PathBuf;

const HELP: &str = "\
kaminoneko               top bar icon (screenshot and print); if it is already running,
                         takes a screenshot instead
kaminoneko capture       takes a screenshot and opens the editor (for a keyboard shortcut)
kaminoneko open          picks an image file and opens the editor
kaminoneko edit IMAGE    opens the editor with an image
kaminoneko print IMAGE   prints right away, without the editor (full width)

Install with scripts/install.sh (app menu entry, optional autostart).

Environment: KAMINONEKO_IP (192.168.1.1), KAMINONEKO_WIFI (\"Children's camera d109\")";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        None => match tray::lock_instance() {
            Some(_lock) => runtime().and_then(|rt| rt.block_on(tray::run())),
            // Already in the top bar: opening the app again takes a screenshot.
            None => pick_and_edit(tray::capture()),
        },
        Some("capture") => pick_and_edit(tray::capture()),
        Some("open") => pick_and_edit(tray::pick_image()),
        Some("--version" | "-V") => {
            println!("kaminoneko {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("edit") => match args.get(1) {
            Some(p) => editor::run(PathBuf::from(p)),
            None => Err("Missing the image path".into()),
        },
        Some("print") => match args.get(1) {
            Some(p) => print_now(PathBuf::from(p)),
            None => Err("Missing the image path".into()),
        },
        _ => {
            println!("{HELP}");
            Ok(())
        }
    };
    if let Err(e) = result {
        tray::notify(&e);
        std::process::exit(1);
    }
}

fn print_now(path: PathBuf) -> Result<(), String> {
    let img = image::open(&path).map_err(|e| format!("Could not open {}: {e}", path.display()))?;
    let sheet = imaging::render(&img.to_luma8(), &imaging::Settings::default(), &imaging::Placement::default());
    printer::print(&printer::Config::from_env(), &sheet, 1, &|m| println!("{m}"))?;
    println!("Done!");
    Ok(())
}

/// Runs a screenshot or file picker and opens the editor with the result (nothing if cancelled).
fn pick_and_edit(pick: impl Future<Output = Result<Option<PathBuf>, String>>) -> Result<(), String> {
    // The runtime is dropped before the editor opens.
    let picked = runtime()?.block_on(pick)?;
    picked.map_or(Ok(()), editor::run)
}

fn runtime() -> Result<tokio::runtime::Runtime, String> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|e| e.to_string())
}
