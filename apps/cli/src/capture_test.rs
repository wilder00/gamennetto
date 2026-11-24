use anyhow::{bail, Result};
use std::path::Path;
use std::time::{Duration, Instant};

#[cfg(windows)]
pub fn run(monitor: Option<usize>, seconds: u64, screenshot: &Path, cursor: bool) -> Result<()> {
    use gamennetto_capture::{monitors, Capturer};

    let list = monitors()?;
    if list.is_empty() {
        bail!("no monitors found");
    }
    println!("Monitors");
    for m in &list {
        println!(
            "  [{}] {}  {}x{}{}  on {}",
            m.index,
            m.name,
            m.width,
            m.height,
            if m.primary { "  (main)" } else { "" },
            m.adapter_name
        );
    }
    let chosen = match monitor {
        Some(i) => list.get(i).ok_or_else(|| anyhow::anyhow!("no monitor {i}"))?,
        None => list.iter().find(|m| m.primary).unwrap_or(&list[0]),
    };

    println!("\nCapturing monitor [{}] for {seconds} s.", chosen.index);
    println!("Move a window or play a video: Windows only sends a frame when the screen changes.\n");

    let mut cap = Capturer::new(chosen, cursor)?;
    let start = Instant::now();
    let end = start + Duration::from_secs(seconds);
    let mut intervals: Vec<Duration> = Vec::new();
    let mut last: Option<Duration> = None;
    let mut first_frame_after = None;
    let mut frames = 0u64;
    let mut saved = false;
    let mut size = (0, 0);

    while Instant::now() < end {
        let Some(frame) = cap.next_frame(Duration::from_millis(200))? else {
            continue;
        };
        frames += 1;
        size = (frame.width, frame.height);
        first_frame_after.get_or_insert_with(|| start.elapsed());
        if let Some(prev) = last {
            intervals.push(frame.time.saturating_sub(prev));
        }
        last = Some(frame.time);
        if !saved {
            cap.save_png(&frame, screenshot)?;
            saved = true;
        }
    }

    let secs = start.elapsed().as_secs_f64();
    println!("Results");
    println!("  Resolution:        {}x{}", size.0, size.1);
    println!("  Frames received:   {frames}  ({:.1} per second on average)", frames as f64 / secs);
    if let Some(t) = first_frame_after {
        println!("  First frame after: {:.0} ms", t.as_secs_f64() * 1000.0);
    }
    if !intervals.is_empty() {
        let ms = |d: Duration| d.as_secs_f64() * 1000.0;
        let min = intervals.iter().min().copied().unwrap_or_default();
        let fastest_fps = if min.is_zero() { 0.0 } else { 1000.0 / ms(min) };
        let mut sorted = intervals.clone();
        sorted.sort();
        let median = sorted[sorted.len() / 2];
        println!(
            "  Time between frames: fastest {:.1} ms (~{:.0} fps max), median {:.1} ms",
            ms(min),
            fastest_fps,
            ms(median)
        );
    }
    println!("  Frames skipped (a newer one arrived first): {}", cap.dropped_frames());
    if saved {
        println!("  Screenshot saved:  {}", screenshot.display());
    } else {
        println!("  No frame arrived, so no screenshot. Try moving the mouse during the test.");
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn run(_: Option<usize>, _: u64, _: &Path, _: bool) -> Result<()> {
    bail!("screen capture is only implemented on Windows for now")
}
