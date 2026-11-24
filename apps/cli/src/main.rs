use clap::{Parser, Subcommand};
use gamennetto_probe::{probe, Report};

#[derive(Parser)]
#[command(name = "gamennetto", version, about = "Use your phone or another PC as a second screen")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Share this PC's screen. Shows a pairing code for the viewer.
    Host {
        /// Monitor number. Default: ask, or the main monitor.
        #[arg(long)]
        monitor: Option<usize>,
        /// Delay vs sharpness. Default: ask, or balanced.
        #[arg(long, value_enum)]
        quality: Option<quality::Quality>,
        /// Override the preset's frames per second.
        #[arg(long)]
        fps: Option<u32>,
        /// Override the preset's bitrate (Mbit/s).
        #[arg(long)]
        mbps: Option<f64>,
        /// Let the viewer use this PC's mouse and keyboard.
        #[arg(long)]
        allow_control: bool,
        /// Network address to listen on.
        #[arg(long, default_value = "0.0.0.0:48100")]
        bind: std::net::SocketAddr,
        /// Don't announce this host on the network.
        #[arg(long)]
        no_discovery: bool,
        /// Extend: share a NEW virtual monitor instead of copying one.
        /// Needs `gamennetto display install` once.
        #[arg(long)]
        extend: bool,
        /// Size of the virtual monitor, e.g. 1920x1080. Default: the viewer's screen.
        #[arg(long, requires = "extend", value_parser = display::parse_size)]
        size: Option<(u32, u32)>,
    },
    /// Install, check or turn off the virtual monitor used by `host --extend`.
    Display {
        #[command(subcommand)]
        action: display::Action,
    },
    /// Emergency: take a stuck virtual monitor off the desktop.
    ResetDisplays,
    /// Download and install the newest version.
    Update {
        /// Only say whether a newer version exists.
        #[arg(long)]
        check: bool,
    },
    /// Watch (and control, if allowed) another PC's screen.
    View {
        /// Host address, e.g. 192.168.1.20. Default: search the network.
        address: Option<String>,
        /// Pairing code shown by the host. Default: ask.
        #[arg(long)]
        code: Option<String>,
        /// Close after this many seconds (for testing).
        #[arg(long)]
        seconds: Option<u64>,
    },
    /// Check what this machine can do (GPU, encoders, network, Android tools).
    Doctor {
        /// Print machine-readable JSON instead of text.
        #[arg(long)]
        json: bool,
    },
    /// Capture a monitor for a few seconds and report speed. Saves a screenshot.
    CaptureTest {
        /// Monitor number (see the list printed). Default: the main monitor.
        #[arg(long)]
        monitor: Option<usize>,
        /// How long to capture, in seconds.
        #[arg(long, default_value_t = 5)]
        seconds: u64,
        /// Where to save one captured frame.
        #[arg(long, default_value = "capture-test.png")]
        screenshot: std::path::PathBuf,
        /// Hide the mouse pointer in the capture.
        #[arg(long)]
        no_cursor: bool,
    },
    /// Capture and encode the screen to an H.264 file, and report speed.
    EncodeTest {
        /// Monitor number. Default: the main monitor.
        #[arg(long)]
        monitor: Option<usize>,
        /// How long to record, in seconds.
        #[arg(long, default_value_t = 5)]
        seconds: u64,
        /// Output width (default: same as the monitor). Height follows the aspect ratio.
        #[arg(long)]
        width: Option<u32>,
        /// Output height (default: same as the monitor).
        #[arg(long)]
        height: Option<u32>,
        /// Target frames per second.
        #[arg(long, default_value_t = 60)]
        fps: u32,
        /// Target bitrate in Mbit/s.
        #[arg(long, default_value_t = 20.0)]
        mbps: f64,
        /// Output file.
        #[arg(long, default_value = "encode-test.h264")]
        output: std::path::PathBuf,
    },
}

mod capture_test;
mod display;
mod encode_test;
#[cfg(windows)]
mod grab;
mod host;
mod quality;
mod ui;
mod update;
mod view;

fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::Host { monitor, quality, fps, mbps, allow_control, bind, no_discovery, extend, size } => {
            update::notify_in_background();
            host::run(host::Options {
                monitor,
                quality,
                fps,
                mbps,
                bind,
                allow_control,
                discovery: !no_discovery,
                extend,
                size,
            })?;
        }
        Command::Display { action } => display::run(action)?,
        Command::ResetDisplays => display::run(display::Action::Off)?,
        Command::Update { check } => update::run(check)?,
        Command::View { address, code, seconds } => {
            update::notify_in_background();
            view::run(view::Options { address, code, seconds })?;
        }
        Command::Doctor { json } => {
            let report = probe();
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                print_report(&report);
            }
        }
        Command::CaptureTest { monitor, seconds, screenshot, no_cursor } => {
            capture_test::run(monitor, seconds, &screenshot, !no_cursor)?;
        }
        Command::EncodeTest { monitor, seconds, width, height, fps, mbps, output } => {
            encode_test::run(encode_test::Options { monitor, seconds, width, height, fps, mbps, output: &output })?;
        }
    }
    Ok(())
}

fn print_report(r: &Report) {
    println!("Gamennetto doctor ({})\n", r.os);

    println!("Graphics");
    if r.gpus.is_empty() {
        println!("  (none detected)");
    }
    for g in &r.gpus {
        let kind = if g.is_software { " [software]" } else { "" };
        let display = if g.drives_display { " [draws the screen]" } else { "" };
        println!("  - {} ({:?}, {} MB){}{}", g.name, g.vendor, g.vram_mb, kind, display);
    }

    println!("\nVideo encoders");
    if r.encoders.is_empty() {
        println!("  (none detected)");
    }
    for e in &r.encoders {
        let kind = if e.hardware { "hardware" } else { "software" };
        let status = if e.works { "ok" } else { "FAILED to start" };
        println!("  - {:?}  {}  [{}, {}]", e.codec, e.name, kind, status);
    }

    println!("\nNetwork");
    if r.interfaces.is_empty() {
        println!("  (no usable interface)");
    }
    for i in &r.interfaces {
        let lan = match (i.is_virtual, i.is_private_lan) {
            (true, _) => "virtual, ignored",
            (false, true) => "LAN",
            (false, false) => "other",
        };
        println!("  - {}  {}  [{}]", i.name, i.addr, lan);
    }

    println!("\nAndroid tools (optional)");
    println!("  Wi-Fi and USB tethering need nothing extra.");
    println!("  adb is only for USB-debugging mode and building the Android app.");
    println!("  adb:     {}", r.android.adb.as_deref().unwrap_or("not found"));
    println!("  SDK dir: {}", r.android.sdk_dir.as_deref().unwrap_or("not found"));

    let rec = &r.recommendation;
    println!("\nWhat Gamennetto would use");
    match &rec.h264_encoder {
        Some(name) => {
            println!("  H.264 encoder: {}{}", name, if rec.hardware_encoding { "" } else { "  (software, slower)" })
        }
        None => println!("  H.264 encoder: none available"),
    }
    println!("  LAN address:   {}", rec.lan_address.as_deref().unwrap_or("none"));
    for n in &rec.notes {
        println!("  ! {n}");
    }
}
