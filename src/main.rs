#![cfg_attr(all(windows, not(test)), windows_subsystem = "windows")]
#![cfg_attr(not(windows), allow(dead_code))]

mod display;
mod hidpp;
mod layout;
mod logfile;
mod preview;
mod render;
mod scan;
mod settings;
#[cfg(windows)]
mod win;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(i) = args.iter().position(|a| a == "--preview") {
        let dir = args.get(i + 1).cloned().unwrap_or_else(|| "preview".into());
        match preview::run(&dir) {
            Ok(files) => {
                use std::io::Write;
                for f in files {
                    let _ = writeln!(std::io::stdout(), "wrote {}", f);
                }
            }
            Err(e) => {
                use std::io::Write;
                let _ = writeln!(std::io::stderr(), "preview failed: {}", e);
                std::process::exit(1);
            }
        }
        return;
    }
    #[cfg(windows)]
    std::process::exit(win::main(&args));
    #[cfg(not(windows))]
    {
        eprintln!("Battery Strip runs on Windows only (except --preview <dir>).");
        std::process::exit(2);
    }
}
