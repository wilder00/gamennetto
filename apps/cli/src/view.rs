//! `gamennetto view`: watch (and optionally control) a host's screen.

use anyhow::{bail, Context, Result};
use gamennetto_net::{discovery, PairCode, DEFAULT_PORT};
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use crate::ui;

pub struct Options {
    pub address: Option<String>,
    pub code: Option<String>,
    /// Close automatically after this many seconds (for tests).
    pub seconds: Option<u64>,
    pub scale: gamennetto_decode::ScaleMode,
}

pub fn run(o: Options) -> Result<()> {
    let addr = resolve_address(o.address.as_deref())?;
    // Only needed the first time; asked for then if not given here.
    let code = o.code.as_deref().map(PairCode::parse).transpose()?;
    imp::run(addr, code, o.seconds.map(Duration::from_secs), o.scale)
}

fn parse_address(s: &str) -> Result<SocketAddr> {
    if let Ok(a) = s.parse::<SocketAddr>() {
        return Ok(a);
    }
    let ip: IpAddr = s
        .parse()
        .with_context(|| format!("'{s}' is not an address like 192.168.1.20 or 192.168.1.20:{DEFAULT_PORT}"))?;
    Ok(SocketAddr::new(ip, DEFAULT_PORT))
}

fn resolve_address(given: Option<&str>) -> Result<SocketAddr> {
    if let Some(s) = given {
        return parse_address(s);
    }
    println!("Looking for hosts on your network...");
    let hosts = discovery::browse(Duration::from_secs(2))?;
    match hosts.len() {
        0 => {
            println!("No host found. Is `gamennetto host` running on the same Wi-Fi/LAN?");
            if !ui::interactive() {
                bail!("no host found; give an address");
            }
            parse_address(&ui::input("Host address (e.g. 192.168.1.20)")?)
        }
        1 => {
            println!("Found {} at {}", hosts[0].name, hosts[0].addr);
            Ok(hosts[0].addr)
        }
        _ => {
            if !ui::interactive() {
                bail!("several hosts found; give an address");
            }
            let labels: Vec<String> = hosts.iter().map(|h| format!("{}  ({})", h.name, h.addr)).collect();
            Ok(hosts[ui::select("Which host?", &labels, 0)?].addr)
        }
    }
}

/// Estimates the difference between the host's clock and ours from
/// ping/pong round trips, so we can tell how old each frame is.
#[derive(Debug, Default)]
pub struct ClockSync {
    best_rtt: Option<f64>,
    /// host_time - viewer_time, microseconds.
    offset: i64,
}

impl ClockSync {
    pub fn on_pong(&mut self, sent_us: u64, received_us: u64, host_us: u64) {
        let rtt = received_us.saturating_sub(sent_us) as f64;
        // The quickest round trip gives the most accurate estimate. The kept
        // best slowly "ages" so the estimate follows slow clock drift.
        let best = self.best_rtt.map(|b| b * 1.02);
        if best.is_none_or(|b| rtt <= b) {
            self.best_rtt = Some(rtt);
            let midpoint = sent_us as f64 + rtt / 2.0;
            self.offset = host_us as i64 - midpoint as i64;
        } else {
            self.best_rtt = best;
        }
    }

    pub fn synced(&self) -> bool {
        self.best_rtt.is_some()
    }

    /// Converts a host timestamp to our clock.
    pub fn to_viewer(&self, host_us: u64) -> i64 {
        host_us as i64 - self.offset
    }

    pub fn rtt_ms(&self) -> Option<f64> {
        self.best_rtt.map(|r| r / 1000.0)
    }
}

#[cfg(windows)]
mod imp {
    use super::ClockSync;
    use crate::grab::{GrabEvent, KeyboardHook};
    use anyhow::{anyhow, bail, Result};
    use gamennetto_decode::{create_device, window_to_screen, H264Decoder, Presenter, ScaleMode};
    use gamennetto_input::Held;
    use gamennetto_net::*;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use std::net::SocketAddr;
    use std::sync::{mpsc as std_mpsc, Arc, LazyLock, Mutex};
    use std::time::{Duration, Instant};
    use tokio::sync::mpsc;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Foundation::RECT;
    use windows::Win32::UI::WindowsAndMessaging::ClipCursor;
    use winit::application::ApplicationHandler;
    use winit::dpi::LogicalSize;
    use winit::event::{ElementState, MouseScrollDelta, WindowEvent};
    use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
    use winit::keyboard::{KeyCode, ModifiersState, PhysicalKey};
    use winit::platform::scancode::PhysicalKeyExtScancode;
    use winit::window::{Fullscreen, Window, WindowId};

    /// `GAMENNETTO_DEBUG=1` prints what happens to each key.
    fn debug() -> bool {
        static ON: LazyLock<bool> = LazyLock::new(|| std::env::var_os("GAMENNETTO_DEBUG").is_some());
        *ON
    }

    static START: LazyLock<Instant> = LazyLock::new(Instant::now);
    fn now_us() -> u64 {
        START.elapsed().as_micros() as u64
    }

    #[derive(Clone)]
    struct Welcome {
        name: String,
        width: u32,
        height: u32,
        fps: u32,
        control: bool,
        extend: bool,
    }

    enum UserEvent {
        Connected(Welcome),
        Frame,
        Closed(String),
        /// From the keyboard hook while the keyboard is grabbed.
        Grab(GrabEvent),
    }

    #[derive(Default)]
    struct Stats {
        frames: u32,
        bytes: u64,
        latency_ms: Vec<f64>,
        all_latency_ms: Vec<f64>,
        total_frames: u64,
    }

    struct App {
        proxy: EventLoopProxy<UserEvent>,
        hook: Option<KeyboardHook>,
        /// Mouse and keyboard go to the host (VMware-style "grab").
        grabbed: bool,
        held: Held,
        mods: ModifiersState,
        frames: std_mpsc::Receiver<VideoPacket>,
        to_host: mpsc::UnboundedSender<ViewerMsg>,
        clock: Arc<Mutex<ClockSync>>,
        addr: SocketAddr,
        window: Option<Arc<Window>>,
        device: Option<windows::Win32::Graphics::Direct3D11::ID3D11Device>,
        presenter: Option<Presenter>,
        scale: ScaleMode,
        decoder: Option<H264Decoder>,
        welcome: Option<Welcome>,
        waiting_for_keyframe: bool,
        stats: Stats,
        last_numbers: String,
        next_title: Instant,
        deadline: Option<Instant>,
        result: Result<()>,
    }

    pub fn run(addr: SocketAddr, code: Option<PairCode>, seconds: Option<Duration>, scale: ScaleMode) -> Result<()> {
        let rt = tokio::runtime::Runtime::new()?;
        // Our main screen's size: in extend mode the host makes its virtual
        // monitor this size, so it fills our screen exactly.
        let screen = gamennetto_capture::monitors()
            .ok()
            .and_then(|l| l.into_iter().find(|m| m.primary))
            .map(|m| (m.width, m.height));
        // Connect and pair in the console first; the window opens after.
        println!("Connecting to {addr}...");
        let session = rt.block_on(connect(addr, code, screen))?;

        let event_loop = EventLoop::<UserEvent>::with_user_event().build()?;
        // No raw input: once winit registers the keyboard for raw input,
        // Windows stops calling our low-level keyboard hook (measured), and
        // Alt+Tab and the Windows key stay on this PC. The viewer only needs
        // window events.
        event_loop.listen_device_events(winit::event_loop::DeviceEvents::Never);
        let proxy = event_loop.create_proxy();
        let (frames_tx, frames_rx) = std_mpsc::channel();
        let (to_host_tx, to_host_rx) = mpsc::unbounded_channel();
        let clock = Arc::new(Mutex::new(ClockSync::default()));

        let (bye_tx, bye_rx) = tokio::sync::oneshot::channel();
        let welcome = session.welcome.clone();
        let net = rt.spawn(net_task(session, proxy.clone(), frames_tx, to_host_rx, clock.clone(), bye_rx));
        // Queued now, handled as soon as the window exists.
        let _ = proxy.send_event(UserEvent::Connected(welcome));

        let mut app = App {
            proxy,
            hook: None,
            grabbed: false,
            held: Held::default(),
            mods: ModifiersState::empty(),
            frames: frames_rx,
            to_host: to_host_tx,
            clock,
            addr,
            window: None,
            device: None,
            presenter: None,
            scale,
            decoder: None,
            welcome: None,
            waiting_for_keyframe: true,
            stats: Stats::default(),
            last_numbers: "connecting".into(),
            next_title: Instant::now(),
            deadline: seconds.map(|s| Instant::now() + s),
            result: Ok(()),
        };
        event_loop.run_app(&mut app)?;
        app.ungrab();
        app.hook = None;
        // Give the "release" messages a moment to leave before saying bye.
        std::thread::sleep(Duration::from_millis(50));
        // Say goodbye so the host frees its slot at once instead of
        // waiting for the connection to time out.
        let _ = bye_tx.send(());
        let _ = rt.block_on(async { tokio::time::timeout(Duration::from_secs(1), net).await });
        app.print_summary();
        drop(rt);
        app.result
    }

    /// A connection that got through pairing and was welcomed.
    struct Session {
        ep: quinn::Endpoint,
        conn: quinn::Connection,
        send: quinn::SendStream,
        recv: quinn::RecvStream,
        welcome: Welcome,
    }

    /// Asks in the console without blocking the network tasks.
    async fn ask(prompt: &'static str, allow_empty: bool) -> Result<String> {
        tokio::task::spawn_blocking(move || {
            dialoguer::Input::<String>::new().with_prompt(prompt).allow_empty(allow_empty).interact_text()
        })
        .await?
        .map_err(Into::into)
    }

    async fn confirm(prompt: String) -> Result<bool> {
        tokio::task::spawn_blocking(move || dialoguer::Confirm::new().with_prompt(prompt).default(false).interact())
            .await?
            .map_err(Into::into)
    }

    /// Connects, gets approved if this PC and the host don't know each other
    /// yet, and waits for the host's welcome.
    async fn connect(addr: SocketAddr, code: Option<PairCode>, screen: Option<(u32, u32)>) -> Result<Session> {
        let id = Identity::load_or_create(&data_dir().join("identity"))?;
        let mut trust = TrustStore::open()?;
        let ep = client_endpoint(&id)?;
        let conn = ep.connect(addr, SERVER_NAME)?.await.map_err(|e| {
            anyhow!(
                "could not connect ({e}). Check the address, that the host is running, \
                 and that its firewall allows Gamennetto."
            )
        })?;
        // TLS proved the host holds the key behind this fingerprint.
        let host_fp = peer_fingerprint(&conn)?;
        let known_host = trust.find(Role::Host, &host_fp).map(|d| d.name.clone());
        let (mut send, mut recv) = conn.open_bi().await?;
        let name = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "viewer".into());
        let hello = ViewerMsg::Hello { version: PROTOCOL_VERSION, name, screen, pair: known_host.is_none() };
        write_msg(&mut send, &hello).await?;

        let mut paired = false;
        let welcome = loop {
            match read_msg::<HostMsg>(&mut recv).await? {
                HostMsg::PairingRequired { by_code, by_approval } => {
                    pair(&conn, &mut send, &mut recv, code, by_code, by_approval).await?;
                    paired = true;
                }
                HostMsg::Welcome { name, width, height, fps, control_allowed, extend, .. } => {
                    break Welcome { name, width, height, fps, control: control_allowed, extend };
                }
                HostMsg::Busy => bail!("the host is busy: another viewer is already connected"),
                HostMsg::Error { message } => bail!("the host could not start sharing: {message}"),
                HostMsg::PairRefused { reason } => bail!("not allowed: {reason}"),
                other => bail!("unexpected answer from host: {other:?}"),
            }
        };
        // A host this PC doesn't know must have gone through pairing; one
        // that skips it is not the host it claims to be.
        if known_host.is_none() && !paired {
            bail!("this host is not approved on this PC and did not ask for approval: not connecting");
        }
        if paired {
            if trust.same_name_other_key(Role::Host, &welcome.name, &host_fp) {
                println!("  Note: {} was approved before with another identity (reinstalled?).", welcome.name);
            }
            trust.add(Role::Host, &welcome.name, &host_fp);
            trust.save()?;
            println!("  {} approved. Next time this PC connects to it without a code.", welcome.name);
        }
        Ok(Session { ep, conn, send, recv, welcome })
    }

    /// The first connection between this PC and the host: a code, or
    /// comparing a number on both screens.
    async fn pair(
        conn: &quinn::Connection,
        send: &mut quinn::SendStream,
        recv: &mut quinn::RecvStream,
        code: Option<PairCode>,
        by_code: bool,
        by_approval: bool,
    ) -> Result<()> {
        let binding = session_binding(conn)?;
        let interactive = crate::ui::interactive();
        println!("\nThis PC and the host don't know each other yet: approve this connection once.");
        let how = match code {
            Some(c) => ViewerPairing::Code(c),
            None if !interactive => bail!("this PC isn't approved by the host yet: give its code with --code"),
            None => loop {
                let typed = if by_approval {
                    ask("Type the 6-digit code shown on the host, or press Enter to approve it there instead", true)
                        .await?
                } else {
                    ask("Type the 6-digit code shown on the host", false).await?
                };
                if typed.trim().is_empty() && by_approval {
                    break ViewerPairing::Approval;
                }
                match PairCode::parse(&typed) {
                    Ok(c) => break ViewerPairing::Code(c),
                    Err(e) => println!("  {e}"),
                }
            },
        };
        if matches!(how, ViewerPairing::Code(_)) && !by_code {
            bail!("the host isn't accepting codes right now (too many wrong ones); wait a minute");
        }
        let approval = matches!(how, ViewerPairing::Approval);
        let number = format_number(check_number(&binding));
        if approval {
            println!("  Your number: {number}");
            println!("  On the host, allow the connection only if it shows the same number. Waiting...");
        }
        viewer_pair(send, recv, &binding, how).await?;
        // The number check counts only if both screens really showed it:
        // this catches a fake host that just says yes.
        if approval && !confirm(format!("Did the host show {number}?")).await? {
            bail!("not connecting: the numbers didn't match");
        }
        Ok(())
    }

    async fn net_task(
        session: Session,
        proxy: EventLoopProxy<UserEvent>,
        frames: std_mpsc::Sender<VideoPacket>,
        mut to_host: mpsc::UnboundedReceiver<ViewerMsg>,
        clock: Arc<Mutex<ClockSync>>,
        mut bye: tokio::sync::oneshot::Receiver<()>,
    ) {
        let p = proxy.clone();
        let Session { ep, conn, mut send, mut recv, .. } = session;
        let result: Result<()> = async {
            let mut video = conn.accept_uni().await?;

            let video_task = async {
                loop {
                    let packet = VideoPacket::read(&mut video).await?;
                    frames.send(packet).map_err(|_| anyhow!("window closed"))?;
                    let _ = p.send_event(UserEvent::Frame);
                }
            };
            let control_in = async {
                loop {
                    if let HostMsg::Pong { viewer_us, host_us } = read_msg(&mut recv).await? {
                        clock.lock().unwrap().on_pong(viewer_us, now_us(), host_us);
                    }
                }
            };
            let control_out = async {
                let mut tick = tokio::time::interval(Duration::from_millis(500));
                loop {
                    tokio::select! {
                        _ = tick.tick() => {
                            write_msg(&mut send, &ViewerMsg::Ping { viewer_us: now_us() }).await?;
                        }
                        msg = to_host.recv() => match msg {
                            Some(m) => write_msg(&mut send, &m).await?,
                            None => return anyhow::Ok(()),
                        },
                    }
                }
            };
            let r = tokio::select! {
                r = video_task => r,
                r = control_in => r,
                r = control_out => r,
                _ = &mut bye => Ok(()),
            };
            conn.close(0u32.into(), b"bye");
            let _ = tokio::time::timeout(Duration::from_millis(500), ep.wait_idle()).await;
            r
        }
        .await;
        let msg = match result {
            Ok(()) => String::new(),
            Err(e) => format!("{e:#}"),
        };
        let _ = proxy.send_event(UserEvent::Closed(msg));
    }

    impl App {
        fn redraw(&mut self) {
            let Some(p) = self.presenter.as_mut() else { return };
            let frame = self.decoder.as_ref().and_then(|d| d.frame().map(|t| (t, d.width, d.height)));
            if let Err(e) = p.present(frame) {
                eprintln!("Drawing failed: {e:#}");
            }
        }

        fn on_frames(&mut self) {
            let mut newest = None;
            while let Ok(packet) = self.frames.try_recv() {
                self.stats.bytes += packet.data.len() as u64;
                let Some(decoder) = self.decoder.as_mut() else { continue };
                // P-frames are useless until we have a full picture to build on.
                if self.waiting_for_keyframe && !packet.keyframe {
                    continue;
                }
                self.waiting_for_keyframe = false;
                match decoder.decode(&packet.data, Duration::from_micros(packet.capture_us)) {
                    Ok(true) => newest = Some(packet.capture_us),
                    Ok(false) => {}
                    Err(e) => {
                        eprintln!("Decoding error ({e:#}); asking for a full picture.");
                        self.waiting_for_keyframe = true;
                        let _ = self.to_host.send(ViewerMsg::RequestKeyframe);
                    }
                }
            }
            if let Some(capture_us) = newest {
                self.redraw();
                self.stats.frames += 1;
                self.stats.total_frames += 1;
                let clock = self.clock.lock().unwrap();
                if clock.synced() {
                    let age = (now_us() as i64 - clock.to_viewer(capture_us)) as f64 / 1000.0;
                    if (0.0..5000.0).contains(&age) {
                        self.stats.latency_ms.push(age);
                        self.stats.all_latency_ms.push(age);
                    }
                }
            }
        }

        /// Once a second: measure fps, delay and data rate, then show them.
        fn update_title(&mut self) {
            let s = &mut self.stats;
            let latency = if s.latency_ms.is_empty() {
                "measuring delay".to_string()
            } else {
                let avg = s.latency_ms.iter().sum::<f64>() / s.latency_ms.len() as f64;
                format!("{avg:.0} ms delay")
            };
            let rtt = self.clock.lock().unwrap().rtt_ms().map(|r| format!(", network {r:.1} ms")).unwrap_or_default();
            self.last_numbers =
                format!("{} fps - {}{} - {:.1} Mbit/s", s.frames, latency, rtt, s.bytes as f64 * 8.0 / 1e6);
            s.frames = 0;
            s.bytes = 0;
            s.latency_ms.clear();
            self.update_title_now();
        }

        /// Shows the title with the last measured numbers.
        fn update_title_now(&mut self) {
            let (Some(w), Some(win)) = (&self.welcome, &self.window) else { return };
            let control = match (w.control, self.grabbed) {
                (false, _) => "view only",
                (true, false) => "click to control",
                (true, true) => "CONTROLLING - Ctrl+Alt to release",
            };
            win.set_title(&format!("{} - {} - {}x{} - {}", w.name, control, w.width, w.height, self.last_numbers));
        }

        fn print_summary(&mut self) {
            let v = &mut self.stats.all_latency_ms;
            println!("\nSession summary");
            println!("  Frames shown: {}", self.stats.total_frames);
            if v.is_empty() {
                println!("  Delay: not measured");
                return;
            }
            v.sort_by(|a, b| a.total_cmp(b));
            let avg = v.iter().sum::<f64>() / v.len() as f64;
            println!(
                "  Delay (capture -> shown): average {avg:.1} ms, median {:.1} ms, 95% under {:.1} ms",
                v[v.len() / 2],
                v[(v.len() * 95 / 100).min(v.len() - 1)]
            );
        }

        fn control_allowed(&self) -> bool {
            self.welcome.as_ref().is_some_and(|w| w.control)
        }

        /// Sends input to the host, only while grabbed.
        fn send_input(&mut self, ev: InputEvent) {
            if self.grabbed && self.control_allowed() {
                self.held.track(&ev);
                let _ = self.to_host.send(ViewerMsg::Input(ev));
            }
        }

        /// Mouse and keyboard now go to the host, including Alt+Tab and
        /// the Windows key. The mouse can't leave the window.
        fn grab(&mut self) {
            if self.grabbed || !self.control_allowed() {
                return;
            }
            self.grabbed = true;
            if let Some(h) = &self.hook {
                h.set_active(true);
            }
            self.clip_cursor();
            if let Some(w) = &self.window {
                // The host's pointer is in the picture; hide ours.
                w.set_cursor_visible(false);
            }
            println!("Controlling the host. Press Ctrl+Alt to get your mouse and keyboard back.");
            self.update_title_now();
        }

        /// Gives mouse and keyboard back to this PC and lets go of anything
        /// still pressed on the host.
        fn ungrab(&mut self) {
            if !self.grabbed {
                return;
            }
            for ev in self.held.release_all() {
                let _ = self.to_host.send(ViewerMsg::Input(ev));
            }
            self.grabbed = false;
            if let Some(h) = &self.hook {
                h.set_active(false);
            }
            unsafe {
                let _ = ClipCursor(None);
            }
            if let Some(w) = &self.window {
                w.set_cursor_visible(true);
            }
            self.update_title_now();
        }

        fn clip_cursor(&self) {
            let (Some(w), true) = (&self.window, self.grabbed) else { return };
            let (Ok(pos), size) = (w.inner_position(), w.inner_size()) else { return };
            let r =
                RECT { left: pos.x, top: pos.y, right: pos.x + size.width as i32, bottom: pos.y + size.height as i32 };
            unsafe {
                let _ = ClipCursor(Some(&r));
            }
        }

        fn cycle_scale(&mut self) {
            self.scale = self.scale.next();
            if let Some(p) = self.presenter.as_mut() {
                p.mode = self.scale;
            }
            println!("Picture size: {}  (Ctrl+Alt+S changes it)", self.scale.describe());
            self.redraw();
        }

        fn toggle_fullscreen(&self) {
            if let Some(win) = &self.window {
                let full = win.fullscreen().is_some();
                win.set_fullscreen((!full).then_some(Fullscreen::Borderless(None)));
            }
        }
    }

    impl ApplicationHandler<UserEvent> for App {
        fn resumed(&mut self, el: &ActiveEventLoop) {
            if self.window.is_some() {
                return;
            }
            let setup = || -> Result<_> {
                let attrs = Window::default_attributes()
                    .with_title(format!("Gamennetto - connecting to {}", self.addr))
                    .with_inner_size(LogicalSize::new(1280.0, 720.0));
                let window = Arc::new(el.create_window(attrs)?);
                let hwnd = match window.window_handle()?.as_raw() {
                    RawWindowHandle::Win32(h) => HWND(h.hwnd.get() as *mut _),
                    _ => bail!("not a Windows window"),
                };
                let device = create_device()?;
                let presenter = Presenter::new(&device, hwnd)?;
                Ok((window, device, presenter))
            };
            match setup() {
                Ok((w, d, p)) => {
                    self.window = Some(w);
                    self.device = Some(d);
                    let mut p = p;
                    p.mode = self.scale;
                    self.presenter = Some(p);
                    let proxy = self.proxy.clone();
                    match KeyboardHook::install(move |ev| {
                        let _ = proxy.send_event(UserEvent::Grab(ev));
                    }) {
                        Ok(h) => {
                            if debug() {
                                crate::grab::set_debug(true);
                                eprintln!("[keys] keyboard hook installed");
                            }
                            self.hook = Some(h)
                        }
                        Err(e) => eprintln!("Alt+Tab and the Windows key will stay on this PC ({e:#})."),
                    }
                    self.redraw();
                }
                Err(e) => {
                    self.result = Err(e.context("opening the window"));
                    el.exit();
                }
            }
        }

        fn user_event(&mut self, el: &ActiveEventLoop, event: UserEvent) {
            match event {
                UserEvent::Connected(w) => {
                    println!(
                        "Connected to {} ({}x{}, {} fps){}",
                        w.name,
                        w.width,
                        w.height,
                        w.fps,
                        if w.control { ", control allowed" } else { ", view only" }
                    );
                    if w.extend {
                        // An extra monitor should fill this screen.
                        if let Some(win) = &self.window {
                            win.set_fullscreen(Some(Fullscreen::Borderless(None)));
                        }
                        println!("This is an extra screen for {}.", w.name);
                    }
                    if w.control {
                        println!("  Click the picture: your mouse and keyboard then control {} (Alt+Tab too).", w.name);
                        println!("  Ctrl+Alt: get your mouse and keyboard back.");
                    }
                    println!("  F11: full screen on/off.   Ctrl+Alt+S: picture size.   Ctrl+Alt+Q: close the viewer.");
                    let Some(device) = &self.device else { return };
                    match H264Decoder::new(device, w.width, w.height, w.fps) {
                        Ok(d) => self.decoder = Some(d),
                        Err(e) => {
                            self.result = Err(e.context("starting the video decoder"));
                            el.exit();
                        }
                    }
                    self.welcome = Some(w);
                    // Testing aid (with GAMENNETTO_DEBUG): take control at
                    // once, without a click.
                    if debug() && std::env::var_os("GAMENNETTO_DEBUG_GRAB").is_some() {
                        self.grab();
                    }
                }
                UserEvent::Frame => self.on_frames(),
                UserEvent::Grab(ev) => {
                    if debug() {
                        eprintln!("[keys] hook: {ev:?} (grabbed: {})", self.grabbed);
                    }
                    match ev {
                        GrabEvent::Key { scancode, down } => self.send_input(InputEvent::Key { scancode, down }),
                        GrabEvent::Release => self.ungrab(),
                        GrabEvent::ToggleFullscreen => self.toggle_fullscreen(),
                        GrabEvent::CycleScale => self.cycle_scale(),
                        GrabEvent::Quit => {
                            self.ungrab();
                            el.exit();
                        }
                    }
                }
                UserEvent::Closed(msg) => {
                    if !msg.is_empty() && self.result.is_ok() {
                        self.result = Err(anyhow!(msg));
                    }
                    el.exit();
                }
            }
        }

        fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
            match event {
                WindowEvent::CloseRequested => {
                    self.ungrab();
                    el.exit();
                }
                // Clicked another window, Ctrl+Alt+Del, a notification...:
                // give everything back and let go of pressed keys.
                WindowEvent::Focused(false) => self.ungrab(),
                WindowEvent::ModifiersChanged(m) => self.mods = m.state(),
                WindowEvent::Moved(_) => self.clip_cursor(),
                WindowEvent::Resized(size) => {
                    if let Some(p) = self.presenter.as_mut() {
                        if let Err(e) = p.resize(size.width, size.height) {
                            eprintln!("Resizing the picture to {}x{} failed: {e:#}", size.width, size.height);
                        } else if debug() {
                            eprintln!("[window] resized to {}x{}", size.width, size.height);
                        }
                    }
                    self.clip_cursor();
                    self.redraw();
                }
                WindowEvent::RedrawRequested => self.redraw(),
                WindowEvent::CursorMoved { position, .. } => {
                    let Some(p) = &self.presenter else { return };
                    let ((src, dst), (w, h)) = (p.shown, p.picture_size);
                    if let Some((x, y)) = window_to_screen(position.x, position.y, src, dst, w, h) {
                        self.send_input(InputEvent::MouseMove { x, y });
                    }
                }
                WindowEvent::MouseInput { state, button, .. } => {
                    let button = match button {
                        winit::event::MouseButton::Left => MouseButton::Left,
                        winit::event::MouseButton::Right => MouseButton::Right,
                        winit::event::MouseButton::Middle => MouseButton::Middle,
                        _ => return,
                    };
                    let down = state == ElementState::Pressed;
                    if !self.grabbed {
                        // Like a virtual machine window: the first click only grabs.
                        if down && button == MouseButton::Left {
                            self.grab();
                        }
                        return;
                    }
                    self.send_input(InputEvent::MouseButton { button, down });
                }
                WindowEvent::MouseWheel { delta, .. } => {
                    let (x, y) = match delta {
                        MouseScrollDelta::LineDelta(x, y) => (x, y),
                        MouseScrollDelta::PixelDelta(p) => (p.x as f32 / 120.0, p.y as f32 / 120.0),
                    };
                    self.send_input(InputEvent::Wheel { x, y });
                }
                WindowEvent::KeyboardInput { event, .. } => {
                    // While grabbed the keyboard hook normally takes every
                    // key before it gets here. If one arrives anyway (the hook
                    // is missing, or Windows skipped it), send it from here,
                    // so typing still reaches the host. Not on the same PC as
                    // the host: its typed keys would come back here and loop.
                    if self.grabbed {
                        let scancode = event.physical_key.to_scancode();
                        if debug() {
                            eprintln!(
                                "[keys] window got {:?} (hook missed it), scancode {scancode:x?}",
                                event.physical_key
                            );
                        }
                        if !self.addr.ip().is_loopback() {
                            if let Some(scancode) = scancode {
                                let down = event.state.is_pressed();
                                let shortcut = self.mods.control_key() && self.mods.alt_key();
                                match event.physical_key {
                                    PhysicalKey::Code(KeyCode::F11) => {
                                        if down && !event.repeat {
                                            self.toggle_fullscreen();
                                        }
                                    }
                                    PhysicalKey::Code(KeyCode::KeyQ) if shortcut => {
                                        self.ungrab();
                                        el.exit();
                                    }
                                    PhysicalKey::Code(KeyCode::KeyS) if shortcut => {
                                        if down && !event.repeat {
                                            self.cycle_scale();
                                        }
                                    }
                                    _ => self.send_input(InputEvent::Key { scancode, down }),
                                }
                            }
                        }
                        return;
                    }
                    // Not grabbed: keys stay on this PC.
                    if !event.state.is_pressed() || event.repeat {
                        return;
                    }
                    match event.physical_key {
                        PhysicalKey::Code(KeyCode::F11) => self.toggle_fullscreen(),
                        PhysicalKey::Code(KeyCode::KeyQ) if self.mods.control_key() && self.mods.alt_key() => el.exit(),
                        PhysicalKey::Code(KeyCode::KeyS) if self.mods.control_key() && self.mods.alt_key() => {
                            self.cycle_scale()
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }

        fn about_to_wait(&mut self, el: &ActiveEventLoop) {
            let now = Instant::now();
            if self.deadline.is_some_and(|d| now >= d) {
                el.exit();
                return;
            }
            if now >= self.next_title {
                self.update_title();
                self.next_title = now + Duration::from_secs(1);
            }
            el.set_control_flow(ControlFlow::WaitUntil(self.next_title));
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use anyhow::Result;
    use gamennetto_net::PairCode;
    use std::net::SocketAddr;
    use std::time::Duration;

    pub fn run(_: SocketAddr, _: Option<PairCode>, _: Option<Duration>, _: gamennetto_decode::ScaleMode) -> Result<()> {
        anyhow::bail!("the viewer is only implemented on Windows for now")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_sync_finds_offset() {
        // Host clock is 1_000_000 us ahead; network takes 2 ms each way.
        let mut c = ClockSync::default();
        let sent = 50_000;
        let host = sent + 2_000 + 1_000_000;
        c.on_pong(sent, sent + 4_000, host);
        assert!(c.synced());
        assert_eq!(c.to_viewer(host), (sent + 2_000) as i64);
        assert_eq!(c.rtt_ms(), Some(4.0));
    }

    #[test]
    fn clock_sync_prefers_fast_round_trips() {
        let mut c = ClockSync::default();
        c.on_pong(0, 2_000, 1_001_000); // good sample: offset 1_000_000
        c.on_pong(10_000, 30_000, 1_011_000); // slow, asymmetric sample: ignored
        assert_eq!(c.to_viewer(1_000_000), 0);
    }

    #[test]
    fn parses_addresses() {
        assert_eq!(parse_address("192.168.1.20").unwrap().port(), DEFAULT_PORT);
        assert_eq!(parse_address("192.168.1.20:5000").unwrap().port(), 5000);
        assert!(parse_address("not an ip").is_err());
    }
}
