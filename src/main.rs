//! Kaminoneko: top bar icon to take a screenshot and print it
//! on the kids' camera with a thermal printer.

mod editor;
mod imaging;
mod printer;
mod theme;
mod tray;

use std::path::PathBuf;

const HELP: &str = "\
kaminoneko               top bar icon (screenshot and print)
kaminoneko capture       takes a screenshot and opens the editor (for a keyboard shortcut)
kaminoneko edit IMAGE    opens the editor with an image
kaminoneko print IMAGE   prints right away, without the editor (full width)
kaminoneko install       copies itself to ~/.local/bin, adds a menu entry and starts on login

Environment: KAMINONEKO_IP (192.168.1.1), KAMINONEKO_WIFI (\"Children's camera d109\")";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        None => runtime().and_then(|rt| rt.block_on(tray::run())),
        Some("capture") => match runtime().and_then(|rt| rt.block_on(tray::capture())) {
            Ok(Some(path)) => editor::run(path),
            Ok(None) => Ok(()),
            Err(e) => Err(e),
        },
        Some("edit") => match args.get(1) {
            Some(p) => editor::run(PathBuf::from(p)),
            None => Err("Missing the image path".into()),
        },
        Some("print") => match args.get(1) {
            Some(p) => print_now(PathBuf::from(p)),
            None => Err("Missing the image path".into()),
        },
        Some("install") => install(),
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

fn runtime() -> Result<tokio::runtime::Runtime, String> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|e| e.to_string())
}

fn install() -> Result<(), String> {
    let home = PathBuf::from(std::env::var("HOME").map_err(|_| "HOME is not set")?);
    let bin = home.join(".local/bin/kaminoneko");
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(bin.parent().unwrap()).map_err(|e| e.to_string())?;
    if exe != bin {
        // Copy to a temp file and rename, so reinstalling works even while it is running.
        let tmp = bin.with_extension("new");
        std::fs::copy(&exe, &tmp).map_err(|e| format!("Could not copy the binary: {e}"))?;
        std::fs::rename(&tmp, &bin).map_err(|e| e.to_string())?;
    }
    let desktop = format!(
        "[Desktop Entry]\nType=Application\nName=Kaminoneko\nComment=Take a screenshot and print it on the camera\n\
         Exec={}\nIcon=printer\nTerminal=false\nCategories=Utility;Graphics;\n",
        bin.display()
    );
    for dir in [".local/share/applications", ".config/autostart"] {
        let d = home.join(dir);
        std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
        std::fs::write(d.join("kaminoneko.desktop"), &desktop).map_err(|e| e.to_string())?;
    }
    println!("Installed to {}.\nIt will start on login and also shows up in the app menu.", bin.display());
    Ok(())
}
