use anyhow::Result;
use std::path::Path;

pub struct Options<'a> {
    pub monitor: Option<usize>,
    pub seconds: u64,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub fps: u32,
    pub mbps: f64,
    pub output: &'a Path,
}

#[cfg(windows)]
pub fn run(o: Options) -> Result<()> {
    use anyhow::{bail, Context};
    use gamennetto_capture::{monitors, Capturer};
    use gamennetto_encode::{even, Converter, EncoderConfig, H264Encoder};
    use std::io::Write;
    use std::time::{Duration, Instant};

    let list = monitors()?;
    let mon = match o.monitor {
        Some(i) => list.get(i).with_context(|| format!("no monitor {i}"))?,
        None => list.iter().find(|m| m.primary).or(list.first()).context("no monitors")?,
    };
    let mut cap = Capturer::new(mon, true)?;

    // Output size: keep the aspect ratio if only one side was given.
    let (in_w, in_h) = (mon.width, mon.height);
    let (out_w, out_h) = match (o.width, o.height) {
        (Some(w), Some(h)) => (w, h),
        (Some(w), None) => (w, w * in_h / in_w),
        (None, Some(h)) => (h * in_w / in_h, h),
        (None, None) => (in_w, in_h),
    };
    let (out_w, out_h) = (even(out_w), even(out_h));
    if out_w == 0 || out_h == 0 {
        bail!("output size too small");
    }

    let mut conv = Converter::new(cap.device(), in_w, in_h, out_w, out_h)?;
    let cfg = EncoderConfig {
        width: out_w,
        height: out_h,
        fps: o.fps,
        bitrate: (o.mbps * 1_000_000.0) as u32,
        keyframe_interval: o.fps * 2,
    };
    let mut enc = H264Encoder::new(cap.device(), &cfg)?;

    println!("Monitor:  [{}] {}x{} on {}", mon.index, in_w, in_h, mon.adapter_name);
    println!("Encoder:  {}", enc.name());
    println!("Output:   {out_w}x{out_h}, {} fps target, {} Mbit/s", o.fps, o.mbps);
    println!("\nEncoding for {} s. Move a window or play a video so the screen changes.\n", o.seconds);

    let mut file = std::io::BufWriter::new(std::fs::File::create(o.output)?);
    let start = Instant::now();
    let end = start + Duration::from_secs(o.seconds);
    let mut t0 = None;
    let (mut frames_in, mut frames_out, mut keyframes, mut bytes) = (0u64, 0u64, 0u64, 0u64);
    let mut convert_ms = Vec::new();
    let mut encode_ms = Vec::new();

    while Instant::now() < end {
        let Some(frame) = cap.next_frame(Duration::from_millis(100))? else {
            continue;
        };
        frames_in += 1;
        let base = *t0.get_or_insert(frame.time);
        let pts = frame.time.saturating_sub(base);

        let t = Instant::now();
        let nv12 = conv.convert(&frame.texture)?.clone();
        convert_ms.push(t.elapsed().as_secs_f64() * 1000.0);
        drop(frame); // give the buffer back to Windows as early as possible

        let t = Instant::now();
        let out = enc.encode(&nv12, pts, Duration::from_millis(500))?;
        encode_ms.push(t.elapsed().as_secs_f64() * 1000.0);
        for f in out {
            frames_out += 1;
            keyframes += f.keyframe as u64;
            bytes += f.data.len() as u64;
            file.write_all(&f.data)?;
        }
    }
    file.flush()?;

    let secs = start.elapsed().as_secs_f64();
    let stats = |v: &mut Vec<f64>| -> String {
        if v.is_empty() {
            return "n/a".into();
        }
        v.sort_by(|a, b| a.total_cmp(b));
        let avg = v.iter().sum::<f64>() / v.len() as f64;
        let p95 = v[(v.len() * 95 / 100).min(v.len() - 1)];
        format!("average {avg:.1} ms, 95% under {p95:.1} ms, worst {:.1} ms", v[v.len() - 1])
    };
    println!("Results");
    println!("  Frames captured: {frames_in}   encoded: {frames_out}   keyframes: {keyframes}");
    println!("  Colour convert:  {}", stats(&mut convert_ms));
    println!("  Encode:          {}", stats(&mut encode_ms));
    println!(
        "  Data:            {:.1} MB total, {:.1} Mbit/s actual, {:.0} KB per frame",
        bytes as f64 / 1e6,
        bytes as f64 * 8.0 / secs / 1e6,
        if frames_out > 0 { bytes as f64 / frames_out as f64 / 1e3 } else { 0.0 }
    );
    println!("  Saved:           {}  (play it with: ffplay {})", o.output.display(), o.output.display());
    Ok(())
}

#[cfg(not(windows))]
pub fn run(_: Options) -> Result<()> {
    anyhow::bail!("encoding is only implemented on Windows for now")
}
