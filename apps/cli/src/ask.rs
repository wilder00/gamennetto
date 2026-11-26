//! `--ask` / `-i`: asks, one step at a time, for every option that wasn't
//! given on the command line. The quality and the monitor are asked by the
//! host itself (also without `--ask`).

use anyhow::Result;

use crate::{host, ui, Scale};

const SIZES: &[(u32, u32)] = &[(1920, 1080), (2560, 1440), (1280, 720), (1920, 1200), (3840, 2160)];

pub fn host_options(o: &mut host::Options) -> Result<()> {
    if !ui::interactive() {
        return Ok(());
    }
    println!();
    if !o.extend {
        let modes = [
            "Mirror  - the other PC shows a copy of this screen".to_string(),
            "Extend  - the other PC becomes an extra screen for this one".to_string(),
        ];
        o.extend = ui::select("How do you want to share?", &modes, 0)? == 1;
    }
    if o.extend {
        #[cfg(windows)]
        if !crate::display::driver_ready() {
            println!("\nExtend needs the virtual monitor driver (installed once, asks for administrator permission).");
            if ui::confirm("Install it now?", true)? {
                crate::display::run(crate::display::Action::Install)?;
            } else {
                println!("Using mirror instead.");
                o.extend = false;
            }
        }
    }
    if o.extend && o.size.is_none() {
        let mut labels = vec!["Same as the other PC's screen (recommended)".to_string()];
        labels.extend(SIZES.iter().map(|(w, h)| format!("{w}x{h}")));
        labels.push("Other size...".to_string());
        let i = ui::select("Size of the extra screen", &labels, 0)?;
        o.size = match i {
            0 => None,
            i if i <= SIZES.len() => Some(SIZES[i - 1]),
            _ => loop {
                match crate::display::parse_size(&ui::input("Size, e.g. 1600x900")?) {
                    Ok(s) => break Some(s),
                    Err(e) => println!("  {e}"),
                }
            },
        };
    }
    if !o.allow_control {
        o.allow_control = ui::confirm("Let the other PC use this PC's mouse and keyboard?", false)?;
    }
    Ok(())
}

pub fn view_scale() -> Result<Scale> {
    if !ui::interactive() {
        return Ok(Scale::Fit);
    }
    let labels = [
        "Fit     - as big as possible, whole picture (black bars if the screens' shapes differ)".to_string(),
        "Fill    - covers your whole screen, trims the edges that don't fit".to_string(),
        "Actual  - pixel for pixel, like on the other PC".to_string(),
    ];
    println!("(Ctrl+Alt+S changes it while viewing.)");
    Ok(match ui::select("Picture size", &labels, 0)? {
        1 => Scale::Fill,
        2 => Scale::Actual,
        _ => Scale::Fit,
    })
}
