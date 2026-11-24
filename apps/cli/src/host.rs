//! `gamennetto host`: share this PC's screen, or an extra virtual screen.

use anyhow::Result;
use std::net::SocketAddr;

use crate::quality::Quality;

pub struct Options {
    pub monitor: Option<usize>,
    pub quality: Option<Quality>,
    pub fps: Option<u32>,
    pub mbps: Option<f64>,
    pub bind: SocketAddr,
    pub allow_control: bool,
    pub discovery: bool,
    /// Share a new virtual monitor (extend) instead of copying one (mirror).
    pub extend: bool,
    /// Virtual monitor size; default: the viewer's screen size.
    pub size: Option<(u32, u32)>,
}

#[cfg(windows)]
pub fn run(o: Options) -> Result<()> {
    imp::run(o)
}

#[cfg(not(windows))]
pub fn run(_: Options) -> Result<()> {
    anyhow::bail!("sharing a screen is only implemented on Windows for now")
}

#[cfg(windows)]
mod imp {
    use super::Options;
    use crate::display;
    use crate::quality::output_size;
    use crate::ui;
    use anyhow::{anyhow, bail, Context, Result};
    use gamennetto_capture::{monitors, now, Capturer, Monitor};
    use gamennetto_encode::{Converter, EncoderConfig, H264Encoder};
    use gamennetto_input::{Injector, ScreenRect};
    use gamennetto_net::*;
    use gamennetto_vdisplay::SessionLock;
    use std::net::Ipv4Addr;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Condvar, Mutex};
    use std::time::{Duration, Instant};
    use tokio::sync::{mpsc, oneshot};

    /// In extend mode the virtual monitor stays on this long after the
    /// viewer leaves, so a quick reconnect finds the windows where they were.
    const EXTEND_GRACE: Duration = Duration::from_secs(10);

    /// What to share.
    #[derive(Clone, Copy)]
    enum Source {
        Mirror(usize),
        Extend(Option<(u32, u32)>),
    }

    #[derive(Clone, Copy)]
    struct Encoding {
        max_height: Option<u32>,
        fps: u32,
        mbps: f64,
    }

    /// A viewer asking the capture thread to get ready.
    struct Request {
        screen: Option<(u32, u32)>,
        reply: oneshot::Sender<Result<Setup, String>>,
    }

    /// What the capture thread is sending.
    #[derive(Clone)]
    struct Setup {
        width: u32,
        height: u32,
        fps: u32,
        screen: ScreenRect,
        extend: bool,
    }

    /// State shared between the network tasks and the capture/encode thread.
    struct Shared {
        /// Where encoded frames go; None while nobody is watching.
        sender: Mutex<Option<mpsc::Sender<VideoPacket>>>,
        keyframe: AtomicBool,
        /// A viewer is connected.
        busy: AtomicBool,
        quit: AtomicBool,
        request: Mutex<Option<Request>>,
        wake: Condvar,
    }

    pub fn run(o: Options) -> Result<()> {
        let source = if o.extend {
            if !display::driver_ready() {
                bail!(
                    "extend mode needs the virtual monitor driver.\n\
                     Install it once with: gamennetto display install"
                );
            }
            Source::Extend(o.size)
        } else {
            let list = monitors()?;
            Source::Mirror(choose_monitor(&list, o.monitor)?.index)
        };
        // Only one host may drive the virtual monitor.
        let _lock = match source {
            Source::Extend(_) => Some(SessionLock::acquire().context("another Gamennetto host is already extending")?),
            Source::Mirror(_) => None,
        };
        if display::driver_ready() {
            display::cleanup_leftover();
            if o.extend {
                // We hold the lock, so anything still on is a leftover.
                let _ = gamennetto_vdisplay::detach();
            }
        }

        let quality = match o.quality {
            Some(q) => q,
            None => ui::choose_quality()?,
        };
        let s = quality.settings();
        let enc = Encoding { max_height: s.max_height, fps: o.fps.unwrap_or(s.fps), mbps: o.mbps.unwrap_or(s.mbps) };
        let name = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "Gamennetto host".into());

        let id = HostIdentity::generate()?;
        let rt = tokio::runtime::Runtime::new()?;
        let endpoint = rt.block_on(async { server_endpoint(&id, o.bind) })?;
        let port = endpoint.local_addr()?.port();

        let lan_ip = gamennetto_probe::probe().recommendation.lan_address.and_then(|a| a.parse::<Ipv4Addr>().ok());
        let _advert = match (o.discovery, lan_ip) {
            (true, Some(ip)) => discovery::advertise(&name, ip, port).ok(),
            _ => None,
        };

        println!();
        match source {
            Source::Mirror(i) => {
                let m = monitors()?.into_iter().nth(i).context("the monitor disappeared")?;
                let (w, h) = output_size(m.width, m.height, enc.max_height);
                println!("  Sharing:       [{i}] {}x{} -> sent as {w}x{h} (mirror)", m.width, m.height);
            }
            Source::Extend(size) => {
                let size = size.map(|(w, h)| format!("{w}x{h}")).unwrap_or_else(|| "the viewer's screen size".into());
                println!("  Sharing:       a new virtual monitor ({size}), to the right of your screens");
                println!("                 It turns on when a viewer connects and off 10 s after it leaves.");
            }
        }
        println!("  Quality:       {}", quality.title());
        println!("                 {}", quality.explain());
        println!("                 ({} fps, {} Mbit/s)", enc.fps, enc.mbps);
        match lan_ip {
            _ if !o.bind.ip().is_unspecified() => println!("  Address:       {} (only this address)", o.bind),
            Some(ip) => println!("  Address:       {ip}:{port}"),
            None => println!("  Address:       port {port} (no LAN address found)"),
        }
        println!("  Pairing code:  {}", id.code);
        println!(
            "  Control:       {}",
            if o.allow_control {
                "the viewer CAN use this PC's mouse and keyboard"
            } else {
                "view only (start with --allow-control to let the viewer use mouse and keyboard)"
            }
        );
        println!("\n  Waiting for a viewer. Press Ctrl+C to stop.\n");

        let shared = Arc::new(Shared {
            sender: Mutex::new(None),
            keyframe: AtomicBool::new(false),
            busy: AtomicBool::new(false),
            quit: AtomicBool::new(false),
            request: Mutex::new(None),
            wake: Condvar::new(),
        });
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let sh = shared.clone();
        std::thread::Builder::new().name("capture-encode".into()).spawn(move || {
            stream_thread(source, enc, &sh);
            let _ = done_tx.send(());
        })?;

        let allow_control = o.allow_control;
        rt.block_on(async {
            tokio::select! {
                _ = accept_loop(endpoint, shared.clone(), name, allow_control) => {}
                _ = stop_signal() => println!("\n  Stopping..."),
            }
        });

        // Let the capture thread close cleanly (it turns the virtual monitor off).
        shared.quit.store(true, Ordering::SeqCst);
        shared.wake.notify_all();
        *shared.sender.lock().unwrap() = None;
        if done_rx.recv_timeout(Duration::from_secs(5)).is_err() {
            eprintln!("  The capture thread did not stop in time.");
        }
        Ok(())
    }

    /// Ctrl+C, Ctrl+Break, or the console window being closed.
    async fn stop_signal() {
        use tokio::signal::windows::{ctrl_break, ctrl_c, ctrl_close};
        let (Ok(mut c), Ok(mut b), Ok(mut x)) = (ctrl_c(), ctrl_break(), ctrl_close()) else {
            return std::future::pending().await;
        };
        tokio::select! {
            _ = c.recv() => {}
            _ = b.recv() => {}
            _ = x.recv() => {}
        }
    }

    fn choose_monitor(list: &[Monitor], wanted: Option<usize>) -> Result<Monitor> {
        if let Some(i) = wanted {
            return list.get(i).cloned().with_context(|| format!("there is no monitor {i}"));
        }
        if list.len() > 1 && ui::interactive() {
            let labels: Vec<String> = list
                .iter()
                .map(|m| format!("{} {}x{}{}", m.name, m.width, m.height, if m.primary { " (main)" } else { "" }))
                .collect();
            let i = ui::select("Which screen do you want to share?", &labels, 0)?;
            return Ok(list[i].clone());
        }
        list.iter().find(|m| m.primary).or(list.first()).cloned().context("no monitors found")
    }

    /// Turns the virtual monitor off when dropped.
    struct VirtualOn;
    impl Drop for VirtualOn {
        fn drop(&mut self) {
            match gamennetto_vdisplay::detach() {
                Ok(_) => println!("  Virtual monitor off."),
                Err(e) => eprintln!(
                    "  Could not turn the virtual monitor off ({e:#}).\n  \
                     Run `gamennetto display off`, or press Win+P -> PC screen only."
                ),
            }
        }
    }

    /// A running capture -> convert -> encode pipeline. Field order matters:
    /// capture stops before the virtual monitor goes away.
    struct Active {
        cap: Capturer,
        conv: Converter,
        enc: H264Encoder,
        setup: Setup,
        /// For extend: the size asked for, to reuse the monitor on reconnect.
        size: Option<(u32, u32)>,
        have_picture: bool,
        seq: u32,
        idle_since: Option<Instant>,
        _virtual: Option<VirtualOn>,
    }

    impl Active {
        fn start(source: Source, screen: Option<(u32, u32)>, e: Encoding) -> Result<Self> {
            let (mon, size, virt) = match source {
                Source::Mirror(i) => (monitors()?.into_iter().nth(i).context("the monitor disappeared")?, None, None),
                Source::Extend(fixed) => {
                    let (w, h) = fixed.or(screen).unwrap_or((1920, 1080));
                    let (w, h) = (w & !1, h & !1);
                    display::ensure_size(w, h)?;
                    let v = gamennetto_vdisplay::attach(w, h)?;
                    let guard = VirtualOn;
                    let gdi = v.gdi_name.context("the virtual monitor has no name")?;
                    let mon = wait_for_monitor(&gdi)?;
                    println!("  Virtual monitor on: {gdi} {}x{} (at {},{}).", mon.width, mon.height, mon.left, mon.top);
                    (mon, Some((w, h)), Some(guard))
                }
            };
            let (w, h) = output_size(mon.width, mon.height, e.max_height);
            let cfg = EncoderConfig {
                width: w,
                height: h,
                fps: e.fps,
                bitrate: (e.mbps * 1_000_000.0) as u32,
                keyframe_interval: e.fps * 2,
            };
            let cap = Capturer::new(&mon, true)?;
            let conv = Converter::new(cap.device(), mon.width, mon.height, w, h)?;
            let enc = H264Encoder::new(cap.device(), &cfg)?;
            println!("  Encoder:       {}", enc.name());
            Ok(Self {
                cap,
                conv,
                enc,
                setup: Setup {
                    width: w,
                    height: h,
                    fps: e.fps,
                    screen: ScreenRect { left: mon.left, top: mon.top, width: mon.width, height: mon.height },
                    extend: virt.is_some(),
                },
                size,
                have_picture: false,
                seq: 0,
                idle_since: None,
                _virtual: virt,
            })
        }

        /// Can a new viewer asking for `screen` use this as it is?
        fn fits(&self, source: Source, screen: Option<(u32, u32)>) -> bool {
            match source {
                Source::Mirror(_) => true,
                Source::Extend(Some(_)) => true,
                Source::Extend(None) => {
                    let want = screen.unwrap_or((1920, 1080));
                    self.size == Some((want.0 & !1, want.1 & !1))
                }
            }
        }

        /// One round: wait briefly for a frame, encode it, hand it to the network.
        fn step(&mut self, shared: &Shared) -> Result<()> {
            let frame = self.cap.next_frame(Duration::from_millis(50))?;
            // Keep the latest picture converted (cheap), so a still screen
            // can be resent at once when a full picture is needed.
            if let Some(f) = &frame {
                self.conv.convert(&f.texture)?;
                self.have_picture = true;
            }
            let tx = shared.sender.lock().unwrap().clone();
            let Some(tx) = tx else { return Ok(()) }; // nobody watching

            let want_key = shared.keyframe.swap(false, Ordering::Relaxed);
            let pts = match frame {
                Some(f) => f.time,
                None if want_key && self.have_picture => now(),
                None => {
                    if want_key {
                        shared.keyframe.store(true, Ordering::Relaxed);
                    }
                    return Ok(());
                }
            };
            // The network is behind: skip this frame rather than queue it.
            // Skipping before encoding never corrupts the picture.
            if tx.capacity() == 0 {
                if want_key {
                    shared.keyframe.store(true, Ordering::Relaxed);
                }
                return Ok(());
            }
            if want_key {
                self.enc.request_keyframe();
            }
            let nv12 = self.conv.convert_output().clone();
            for out in self.enc.encode(&nv12, pts, Duration::from_millis(500))? {
                self.seq = self.seq.wrapping_add(1);
                let _ = tx.try_send(VideoPacket {
                    seq: self.seq,
                    capture_us: pts.as_micros() as u64,
                    keyframe: out.keyframe,
                    data: out.data,
                });
            }
            Ok(())
        }
    }

    /// Waits until the capture side sees the new monitor.
    fn wait_for_monitor(gdi: &str) -> Result<Monitor> {
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(m) = monitors()?.into_iter().find(|m| m.name.eq_ignore_ascii_case(gdi)) {
                return Ok(m);
            }
            if Instant::now() > end {
                bail!("the virtual monitor {gdi} can't be captured (it was not found among the GPU's screens)");
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    /// Runs on its own thread: starts the pipeline when a viewer asks,
    /// streams while it watches, and stops (turning the virtual monitor
    /// off) after it leaves.
    fn stream_thread(source: Source, e: Encoding, shared: &Shared) {
        let mut active: Option<Active> = None;
        let grace = match source {
            Source::Mirror(_) => Duration::ZERO,
            Source::Extend(_) => EXTEND_GRACE,
        };
        loop {
            if shared.quit.load(Ordering::SeqCst) {
                return; // dropping `active` turns the virtual monitor off
            }
            let req = shared.request.lock().unwrap().take();
            if let Some(req) = req {
                if !active.as_ref().is_some_and(|a| a.fits(source, req.screen)) {
                    active = None;
                    match Active::start(source, req.screen, e) {
                        Ok(a) => active = Some(a),
                        Err(err) => {
                            eprintln!("  Could not start sharing: {err:#}");
                            let _ = req.reply.send(Err(format!("{err:#}")));
                            continue;
                        }
                    }
                }
                if let Some(a) = active.as_mut() {
                    a.idle_since = None;
                    let _ = req.reply.send(Ok(a.setup.clone()));
                }
            }

            let Some(a) = active.as_mut() else {
                // Idle: sleep until a viewer asks (or we're told to quit).
                let guard = shared.request.lock().unwrap();
                if guard.is_none() {
                    let _ = shared.wake.wait_timeout(guard, Duration::from_millis(500));
                }
                continue;
            };
            if shared.busy.load(Ordering::SeqCst) {
                a.idle_since = None;
            } else if a.idle_since.get_or_insert_with(Instant::now).elapsed() >= grace {
                active = None;
                continue;
            }
            if let Err(err) = a.step(shared) {
                eprintln!("  Capture/encode stopped: {err:#}");
                // Hang up on the viewer; it can reconnect to restart.
                *shared.sender.lock().unwrap() = None;
                active = None;
            }
        }
    }

    async fn accept_loop(endpoint: quinn::Endpoint, shared: Arc<Shared>, name: String, allow_control: bool) {
        let name = Arc::new(name);
        while let Some(incoming) = endpoint.accept().await {
            let (shared, name) = (shared.clone(), name.clone());
            tokio::spawn(async move {
                let addr = incoming.remote_address();
                match incoming.await {
                    Ok(conn) => {
                        if let Err(e) = serve(conn, &shared, &name, allow_control).await {
                            println!("  Viewer {addr} disconnected: {e:#}");
                        }
                    }
                    Err(e) => println!("  Connection from {addr} failed: {e}"),
                }
            });
        }
    }

    async fn serve(conn: quinn::Connection, shared: &Shared, host_name: &str, allow_control: bool) -> Result<()> {
        let addr = conn.remote_address();
        let (mut send, mut recv) = conn.accept_bi().await?;
        let ViewerMsg::Hello { name, screen, .. } = read_msg(&mut recv).await? else {
            bail!("viewer did not say hello");
        };

        if shared.busy.swap(true, Ordering::SeqCst) {
            write_msg(&mut send, &HostMsg::Busy).await?;
            let _ = send.finish();
            println!("  Refused {name} ({addr}): another viewer is connected.");
            // Let the viewer read the message and hang up first.
            let _ = tokio::time::timeout(Duration::from_secs(2), conn.closed()).await;
            return Ok(());
        }
        // From here on, always release `busy` when leaving.
        struct Release<'a>(&'a Shared);
        impl Drop for Release<'_> {
            fn drop(&mut self) {
                *self.0.sender.lock().unwrap() = None;
                self.0.busy.store(false, Ordering::SeqCst);
            }
        }
        let _release = Release(shared);

        // Ask the capture thread to get ready (in extend mode this turns
        // the virtual monitor on, which takes a moment).
        let (reply_tx, reply_rx) = oneshot::channel();
        *shared.request.lock().unwrap() = Some(Request { screen, reply: reply_tx });
        shared.wake.notify_all();
        let setup = match tokio::time::timeout(Duration::from_secs(30), reply_rx).await {
            Ok(Ok(Ok(s))) => s,
            Ok(Ok(Err(message))) => {
                write_msg(&mut send, &HostMsg::Error { message: message.clone() }).await?;
                let _ = send.finish();
                let _ = tokio::time::timeout(Duration::from_secs(2), conn.closed()).await;
                return Err(anyhow!(message));
            }
            _ => bail!("the host could not get ready in time"),
        };

        write_msg(
            &mut send,
            &HostMsg::Welcome {
                version: PROTOCOL_VERSION,
                name: host_name.to_string(),
                width: setup.width,
                height: setup.height,
                fps: setup.fps,
                control_allowed: allow_control,
                extend: setup.extend,
            },
        )
        .await?;
        println!("  {name} ({addr}) is watching.");
        let injector = Injector::new(setup.screen);

        let mut video = conn.open_uni().await?;
        let (tx, mut rx) = mpsc::channel::<VideoPacket>(3);
        *shared.sender.lock().unwrap() = Some(tx);
        shared.keyframe.store(true, Ordering::Relaxed);

        let video_task = async {
            while let Some(p) = rx.recv().await {
                p.write(&mut video).await?;
            }
            anyhow::Ok(())
        };
        let control_task = async {
            loop {
                match read_msg::<ViewerMsg>(&mut recv).await? {
                    ViewerMsg::RequestKeyframe => shared.keyframe.store(true, Ordering::Relaxed),
                    ViewerMsg::Ping { viewer_us } => {
                        let host_us = now().as_micros() as u64;
                        write_msg(&mut send, &HostMsg::Pong { viewer_us, host_us }).await?;
                    }
                    ViewerMsg::Input(ev) if allow_control => injector.inject(ev),
                    _ => {}
                }
            }
            #[allow(unreachable_code)]
            anyhow::Ok(())
        };
        let result = tokio::select! {
            r = video_task => r,
            r = control_task => r,
        };
        println!("  {name} left.");
        result
    }
}
