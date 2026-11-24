//! `gamennetto display ...`: the virtual monitor used by extend mode.

#[derive(clap::Subcommand)]
pub enum Action {
    /// Show whether the virtual monitor driver is installed and in use.
    Status,
    /// Download and install the virtual monitor driver (one time; asks for administrator permission).
    Install,
    /// Remove the virtual monitor driver.
    Uninstall,
    /// Take the virtual monitor off the desktop. Use this if it ever gets stuck on.
    Off,
    /// Put the virtual monitor on the desktop (for testing; `display off` removes it).
    On {
        /// Size, e.g. 1920x1080.
        #[arg(long, default_value = "1920x1080", value_parser = parse_size)]
        size: (u32, u32),
    },
    #[command(hide = true)]
    InstallElevated {
        #[arg(long)]
        dir: std::path::PathBuf,
        #[arg(long)]
        gpu: String,
        #[arg(long)]
        log: std::path::PathBuf,
    },
    #[command(hide = true)]
    UninstallElevated {
        #[arg(long)]
        log: std::path::PathBuf,
    },
}

/// Parses "1920x1080".
pub fn parse_size(s: &str) -> std::result::Result<(u32, u32), String> {
    let (w, h) = s.split_once(['x', 'X']).ok_or("expected a size like 1920x1080")?;
    let w: u32 = w.trim().parse().map_err(|_| "bad width")?;
    let h: u32 = h.trim().parse().map_err(|_| "bad height")?;
    if !(640..=7680).contains(&w) || !(480..=4320).contains(&h) {
        return Err("size must be between 640x480 and 7680x4320".into());
    }
    Ok((w & !1, h & !1))
}

#[cfg(not(windows))]
pub fn run(_: Action) -> anyhow::Result<()> {
    anyhow::bail!("the virtual monitor is only available on Windows for now")
}

#[cfg(windows)]
pub use imp::*;

#[cfg(windows)]
mod imp {
    use super::Action;
    use anyhow::{bail, Context, Result};
    use gamennetto_vdisplay::settings::{self, Settings};
    use gamennetto_vdisplay::*;
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::{Duration, Instant};

    pub fn run(action: Action) -> Result<()> {
        match action {
            Action::Status => status(),
            Action::Install => install(),
            Action::Uninstall => uninstall(),
            Action::Off => off(),
            Action::On { size } => on(size),
            Action::InstallElevated { dir, gpu, log } => logged(&log, |out| install_elevated(&dir, &gpu, out)),
            Action::UninstallElevated { log } => logged(&log, uninstall_elevated),
        }
    }

    /// True if the driver is installed and working.
    pub fn driver_ready() -> bool {
        matches!(find_device(), Ok(Some(d)) if d.problem.is_none())
    }

    /// If the virtual monitor was left on (crash, closed window), and no
    /// Gamennetto host is using it, turn it off.
    pub fn cleanup_leftover() {
        if SessionLock::held_elsewhere() {
            return;
        }
        if let Ok(Some(v)) = virtual_display() {
            if v.active && detach().unwrap_or(false) {
                println!("  Turned off a virtual monitor left on by an earlier session.");
            }
        }
    }

    /// Makes sure the driver offers `w`x`h`, reloading it if a size was added.
    pub fn ensure_size(w: u32, h: u32) -> Result<()> {
        let dir = configured_settings_dir()
            .context("the virtual monitor driver is not set up; run `gamennetto display install`")?;
        let mut s = read_settings().unwrap_or_else(|| Settings::new(&display_gpu()));
        if !s.add_size(w, h) {
            return Ok(());
        }
        write_settings(&dir, &s)?;
        send_command("RELOAD_DRIVER", Duration::from_secs(3))?;
        // The monitor disappears while the driver restarts, then comes back.
        // Wait for both, or we'd use the old monitor just before it goes.
        let _ = wait_for(Duration::from_secs(5), || matches!(virtual_display(), Ok(None)));
        wait_for(Duration::from_secs(15), || matches!(virtual_display(), Ok(Some(_))))
            .context("the virtual monitor did not come back after adding a new size")?;
        // Windows needs a moment to finish setting up the new monitor.
        std::thread::sleep(Duration::from_secs(1));
        Ok(())
    }

    fn wait_for(limit: Duration, mut ok: impl FnMut() -> bool) -> Result<()> {
        let end = Instant::now() + limit;
        while Instant::now() < end {
            if ok() {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        bail!("timed out")
    }

    /// Name of the GPU that draws the screen: the virtual monitor should be
    /// drawn there too, so capture and the hardware encoder stay on one GPU.
    fn display_gpu() -> String {
        gamennetto_probe::probe()
            .gpus
            .into_iter()
            .find(|g| g.drives_display && !g.is_software)
            .map(|g| g.name)
            .unwrap_or_else(|| "default".into())
    }

    fn status() -> Result<()> {
        println!("Virtual monitor (for extend mode)\n");
        match find_device()? {
            None => {
                println!("  Driver:   not installed. Run `gamennetto display install` (one time).");
                return Ok(());
            }
            Some(d) => match d.problem {
                None => println!("  Driver:   installed and working ({})", d.inf.as_deref().unwrap_or("?")),
                Some(22) => println!("  Driver:   installed but DISABLED in Device Manager"),
                Some(p) => {
                    println!("  Driver:   installed but not working (Windows problem code {p}). A restart may help.")
                }
            },
        }
        println!("  Running:  {}", if driver_running() { "yes" } else { "no" });
        match configured_settings_dir() {
            Some(dir) => {
                println!("  Settings: {}", settings_file(&dir).display());
                if let Some(s) = read_settings() {
                    println!("  GPU:      {}", s.gpu);
                    let sizes: Vec<String> = s.sizes.iter().map(|(w, h)| format!("{w}x{h}")).collect();
                    println!("  Sizes:    {} (at {} Hz)", sizes.join(", "), s.refresh);
                }
            }
            None => println!("  Settings: not configured (reinstall with `gamennetto display install`)"),
        }
        match virtual_display()? {
            None => println!("  Monitor:  not detected"),
            Some(v) if v.active => println!(
                "  Monitor:  ON as {} ({}x{} at {},{}){}",
                v.gdi_name.as_deref().unwrap_or("?"),
                v.width,
                v.height,
                v.left,
                v.top,
                if SessionLock::held_elsewhere() {
                    ", used by a Gamennetto host"
                } else {
                    ", not in use: `gamennetto display off` removes it"
                }
            ),
            Some(_) => println!("  Monitor:  off (not part of the desktop)"),
        }
        if std::env::var_os("GAMENNETTO_DEBUG").is_some() {
            println!("\n  Monitors Windows knows:");
            for m in known_monitors() {
                println!("    {m}");
            }
        }
        Ok(())
    }

    fn install() -> Result<()> {
        if let Some(d) = find_device()? {
            if d.problem.is_none() && configured_settings_dir().is_some() {
                println!("The virtual monitor driver is already installed.");
                cleanup_leftover();
                return Ok(());
            }
        }
        println!("This installs the open-source Virtual Display Driver {} (MIT licence,", settings::DRIVER_VERSION);
        println!("signed by SignPath Foundation). Windows asks for administrator permission,");
        println!("and may ask whether to trust the publisher.\n");

        let dir = download_driver()?;
        let gpu = display_gpu();
        let log = std::env::temp_dir().join(format!("gamennetto-install-{}.log", std::process::id()));
        let args = format!(
            "display install-elevated --dir \"{}\" --gpu \"{}\" --log \"{}\"",
            dir.display(),
            gpu.replace('"', ""),
            log.display()
        );
        println!("Installing (accept the Windows prompt)...");
        let code = run_elevated(&args);
        print_log(&log);
        if code? != 0 {
            bail!("the driver was not installed");
        }

        // Windows turns a new monitor on by itself; take it off the desktop.
        print!("Waiting for the virtual monitor...");
        let _ = std::io::stdout().flush();
        if wait_for(Duration::from_secs(20), || matches!(virtual_display(), Ok(Some(_)))).is_err() {
            println!(" not seen yet.");
            println!("If `gamennetto display status` doesn't show it, restart Windows.");
            return Ok(());
        }
        std::thread::sleep(Duration::from_secs(1));
        detach()?;
        println!(" ok.\n");
        println!("Done. The virtual monitor stays off until you share with `gamennetto host --extend`.");
        println!("If it ever stays on by mistake: `gamennetto display off`, or press Win+P -> PC screen only.");
        Ok(())
    }

    /// Downloads the pinned driver release and checks its SHA-256.
    /// `%LOCALAPPDATA%\Gamennetto\driver`
    fn download_dir() -> PathBuf {
        let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        base.join("Gamennetto").join("driver")
    }

    fn download_driver() -> Result<PathBuf> {
        let dir = download_dir().join(settings::DRIVER_VERSION);
        let driver_dir = dir.join("VirtualDisplayDriver");
        let zip = dir.join("driver.zip");
        std::fs::create_dir_all(&dir)?;
        let good = |p: &Path| std::fs::read(p).is_ok_and(|b| settings::sha256_hex(&b) == settings::DRIVER_SHA256);
        if !good(&zip) {
            println!("Downloading {} ...", settings::DRIVER_URL);
            let ok = Command::new("curl.exe")
                .args(["-L", "--fail", "--silent", "--show-error", "-o"])
                .arg(&zip)
                .arg(settings::DRIVER_URL)
                .status()
                .context("running curl.exe")?
                .success();
            if !ok {
                bail!("downloading the driver failed");
            }
            if !good(&zip) {
                let _ = std::fs::remove_file(&zip);
                bail!("the downloaded driver does not match the expected file (SHA-256); not installing it");
            }
        }
        let _ = std::fs::remove_dir_all(&driver_dir);
        let ok = Command::new("tar.exe").arg("-xf").arg(&zip).arg("-C").arg(&dir).status()?.success();
        if !ok || !driver_dir.join("MttVDD.inf").exists() {
            bail!("unpacking the driver failed");
        }
        Ok(driver_dir)
    }

    fn print_log(log: &Path) {
        if let Ok(text) = std::fs::read_to_string(log) {
            for line in text.lines() {
                println!("  {line}");
            }
        }
        let _ = std::fs::remove_file(log);
    }

    /// Runs an administrator step, writing its messages to `log` (its own
    /// console window closes when it ends, so the caller prints the log).
    fn logged(log: &Path, f: impl FnOnce(&mut dyn FnMut(&str)) -> Result<()>) -> Result<()> {
        let mut file = std::fs::File::create(log)?;
        let mut out = |s: &str| {
            let _ = writeln!(file, "{s}");
        };
        let r = f(&mut out);
        if let Err(e) = &r {
            out(&format!("Error: {e:#}"));
        }
        r
    }

    fn install_elevated(driver: &Path, gpu: &str, out: &mut dyn FnMut(&str)) -> Result<()> {
        if !is_admin() {
            bail!("administrator rights are needed");
        }
        let dir = settings_dir();
        write_settings(&dir, &Settings::new(gpu))?;
        // Let normal users update the sizes later without administrator rights.
        let _ = Command::new("icacls").arg(&dir).args(["/grant", "*S-1-5-32-545:(OI)(CI)M", "/T", "/Q"]).output();
        let ok = Command::new("reg")
            .args(["add", &format!("HKLM\\{REG_KEY}"), "/v", "VDDPATH", "/t", "REG_SZ", "/d"])
            .arg(&dir)
            .arg("/f")
            .output()?
            .status
            .success();
        if !ok {
            bail!("could not register the settings folder");
        }
        out(&format!("Settings folder: {}", dir.display()));
        install_driver(driver)?;
        out("Driver installed.");
        Ok(())
    }

    fn uninstall() -> Result<()> {
        if find_device()?.is_none() && configured_settings_dir().is_none() {
            println!("The virtual monitor driver is not installed.");
            return Ok(());
        }
        if SessionLock::held_elsewhere() {
            bail!("a Gamennetto host is using the virtual monitor; stop it first");
        }
        let _ = detach();
        let log = std::env::temp_dir().join(format!("gamennetto-uninstall-{}.log", std::process::id()));
        println!("Removing (accept the Windows prompt)...");
        let code = run_elevated(&format!("display uninstall-elevated --log \"{}\"", log.display()));
        print_log(&log);
        if code? != 0 {
            bail!("the driver was not removed");
        }
        // The downloaded copy of the driver isn't needed any more.
        let _ = std::fs::remove_dir_all(download_dir());
        let local = download_dir();
        if let Some(gn) = local.parent() {
            let _ = std::fs::remove_dir(gn); // only if empty
        }
        println!("Done.");
        Ok(())
    }

    fn uninstall_elevated(out: &mut dyn FnMut(&str)) -> Result<()> {
        if !is_admin() {
            bail!("administrator rights are needed");
        }
        let removed = uninstall_driver()?;
        out(if removed { "Driver removed." } else { "No driver device was found." });
        let ours = configured_settings_dir().is_some_and(|d| d == settings_dir());
        if ours {
            let _ = Command::new("reg").args(["delete", &format!("HKLM\\{REG_KEY}"), "/f"]).output();
        }
        let _ = std::fs::remove_dir_all(settings_dir());
        if let Some(parent) = settings_dir().parent() {
            let _ = std::fs::remove_dir(parent); // only if empty
        }
        Ok(())
    }

    fn off() -> Result<()> {
        if SessionLock::held_elsewhere() {
            println!("Note: a Gamennetto host is using the virtual monitor; its viewer will stop.");
        }
        match virtual_display()? {
            None => println!("No virtual monitor found."),
            Some(_) => {
                if detach()? {
                    println!("The virtual monitor is off. Your monitors are back as before.");
                } else {
                    println!("The virtual monitor was already off.");
                }
            }
        }
        Ok(())
    }

    fn on((w, h): (u32, u32)) -> Result<()> {
        if !driver_ready() {
            bail!("the virtual monitor driver is not installed; run `gamennetto display install`");
        }
        ensure_size(w, h)?;
        let v = attach(w, h)?;
        println!(
            "The virtual monitor is ON as {} ({}x{}). Turn it off with `gamennetto display off`.",
            v.gdi_name.as_deref().unwrap_or("?"),
            v.width,
            v.height
        );
        Ok(())
    }
}
