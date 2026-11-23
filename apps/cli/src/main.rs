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
    /// Check what this machine can do (GPU, encoders, network, Android tools).
    Doctor {
        /// Print machine-readable JSON instead of text.
        #[arg(long)]
        json: bool,
    },
}

fn main() {
    match Cli::parse().command {
        Command::Doctor { json } => {
            let report = probe();
            if json {
                println!("{}", serde_json::to_string_pretty(&report).unwrap());
            } else {
                print_report(&report);
            }
        }
    }
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
        Some(name) => println!(
            "  H.264 encoder: {}{}",
            name,
            if rec.hardware_encoding { "" } else { "  (software, slower)" }
        ),
        None => println!("  H.264 encoder: none available"),
    }
    println!(
        "  LAN address:   {}",
        rec.lan_address.as_deref().unwrap_or("none")
    );
    for n in &rec.notes {
        println!("  ! {n}");
    }
}
