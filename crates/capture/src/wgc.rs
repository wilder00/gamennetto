use anyhow::{bail, Context, Result};
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use windows::core::{IInspectable, Interface};
use windows::Foundation::TypedEventHandler;
use windows::Graphics::Capture::{
    Direct3D11CaptureFrame, Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Graphics::SizeInt32;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::Graphics::Gdi::HMONITOR;
use windows::Win32::System::WinRT::Direct3D11::{CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;

/// A monitor that can be captured.
#[derive(Clone)]
pub struct Monitor {
    pub index: usize,
    /// Windows device name, e.g. `\\.\DISPLAY1`.
    pub name: String,
    /// Position on the Windows desktop, in pixels (can be negative).
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
    pub primary: bool,
    /// The GPU this monitor is attached to.
    pub adapter_name: String,
    handle: HMONITOR,
    adapter: IDXGIAdapter1,
}

impl std::fmt::Debug for Monitor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Monitor")
            .field("index", &self.index)
            .field("name", &self.name)
            .field("size", &(self.width, self.height))
            .field("primary", &self.primary)
            .field("adapter", &self.adapter_name)
            .finish()
    }
}

fn wide(s: &[u16]) -> String {
    let len = s.iter().position(|&c| c == 0).unwrap_or(s.len());
    String::from_utf16_lossy(&s[..len])
}

/// Lists monitors attached to the desktop, together with the GPU that drives
/// each one. Capture must run on that GPU to avoid cross-GPU copies.
pub fn monitors() -> Result<Vec<Monitor>> {
    // Without this, Windows reports sizes scaled by the display zoom
    // (e.g. 2048x1152 instead of 2560x1440 at 125%). Fails harmlessly if
    // the awareness was already set.
    unsafe {
        use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1()? };
    let mut out = Vec::new();
    let mut a = 0;
    while let Ok(adapter) = unsafe { factory.EnumAdapters1(a) } {
        a += 1;
        let adesc = unsafe { adapter.GetDesc1()? };
        let mut o = 0;
        while let Ok(output) = unsafe { adapter.EnumOutputs(o) } {
            o += 1;
            let d = unsafe { output.GetDesc()? };
            if !d.AttachedToDesktop.as_bool() {
                continue;
            }
            let r = d.DesktopCoordinates;
            out.push(Monitor {
                index: out.len(),
                name: wide(&d.DeviceName),
                left: r.left,
                top: r.top,
                width: (r.right - r.left) as u32,
                height: (r.bottom - r.top) as u32,
                primary: r.left == 0 && r.top == 0,
                adapter_name: wide(&adesc.Description),
                handle: d.Monitor,
                adapter: adapter.clone(),
            });
        }
    }
    Ok(out)
}

/// Current time on the same clock as `Frame::time` (the system performance
/// counter). Used to measure how old a frame is.
pub fn now() -> Duration {
    use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
    let (mut count, mut freq) = (0i64, 0i64);
    unsafe {
        let _ = QueryPerformanceCounter(&mut count);
        let _ = QueryPerformanceFrequency(&mut freq);
    }
    let freq = freq.max(1) as u128;
    Duration::from_nanos((count.max(0) as u128 * 1_000_000_000 / freq) as u64)
}

/// One captured frame. The texture lives on the GPU and is only valid while
/// this value is alive; drop it quickly so Windows can reuse the buffer.
pub struct Frame {
    pub texture: ID3D11Texture2D,
    pub width: u32,
    pub height: u32,
    /// When Windows composed this frame (monotonic system clock).
    pub time: Duration,
    inner: Direct3D11CaptureFrame,
}

impl Drop for Frame {
    fn drop(&mut self) {
        let _ = self.inner.Close();
    }
}

/// Latest frame slot. Only the newest frame is kept: for a live screen an old
/// frame is useless, and holding it would add latency.
#[derive(Default)]
struct Slot {
    frame: Mutex<Option<Direct3D11CaptureFrame>>,
    ready: Condvar,
    dropped: Mutex<u64>,
}

pub struct Capturer {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    winrt_device: IDirect3DDevice,
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    slot: Arc<Slot>,
    size: SizeInt32,
}

impl Capturer {
    pub fn new(monitor: &Monitor, show_cursor: bool) -> Result<Self> {
        unsafe {
            // Create the D3D device on the GPU that drives this monitor.
            let mut device = None;
            let mut context = None;
            D3D11CreateDevice(
                &monitor.adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
            .context("creating Direct3D 11 device")?;
            let device = device.context("no D3D11 device")?;
            let context = context.context("no D3D11 context")?;
            // The encoder will use this device from another thread.
            let _ = device.cast::<ID3D11Multithread>()?.SetMultithreadProtected(true);

            let dxgi: IDXGIDevice = device.cast()?;
            let winrt_device: IDirect3DDevice = CreateDirect3D11DeviceFromDXGIDevice(&dxgi)?.cast()?;

            let interop = windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
            let item: GraphicsCaptureItem = interop
                .CreateForMonitor(monitor.handle)
                .context("Windows Graphics Capture is not available for this monitor")?;
            let size = item.Size()?;

            let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
                &winrt_device,
                DirectXPixelFormat::B8G8R8A8UIntNormalized,
                2,
                size,
            )?;

            let slot = Arc::new(Slot::default());
            let s = slot.clone();
            pool.FrameArrived(&TypedEventHandler::<Direct3D11CaptureFramePool, IInspectable>::new(move |pool, _| {
                if let Some(pool) = pool.as_ref() {
                    if let Ok(frame) = pool.TryGetNextFrame() {
                        let mut guard = s.frame.lock().unwrap();
                        if let Some(old) = guard.replace(frame) {
                            let _ = old.Close();
                            *s.dropped.lock().unwrap() += 1;
                        }
                        s.ready.notify_one();
                    }
                }
                Ok(())
            }))?;

            let session = pool.CreateCaptureSession(&item)?;
            session.SetIsCursorCaptureEnabled(show_cursor)?;
            // Removes the yellow border on Windows 11; older versions don't support it.
            let _ = session.SetIsBorderRequired(false);
            session.StartCapture()?;

            Ok(Self { device, context, winrt_device, pool, session, slot, size })
        }
    }

    pub fn device(&self) -> &ID3D11Device {
        &self.device
    }

    /// Frames replaced by a newer one before they were read.
    pub fn dropped_frames(&self) -> u64 {
        *self.slot.dropped.lock().unwrap()
    }

    /// Waits for the next frame. Returns None on timeout. Windows only sends
    /// a frame when something on screen changes, so timeouts are normal on a
    /// still desktop.
    pub fn next_frame(&mut self, timeout: Duration) -> Result<Option<Frame>> {
        let deadline = Instant::now() + timeout;
        let mut guard = self.slot.frame.lock().unwrap();
        let frame = loop {
            if let Some(f) = guard.take() {
                break f;
            }
            let now = Instant::now();
            if now >= deadline {
                return Ok(None);
            }
            guard = self.slot.ready.wait_timeout(guard, deadline - now).unwrap().0;
        };
        drop(guard);

        // The monitor resolution changed: resize the pool for the next frames.
        let content = frame.ContentSize()?;
        if content.Width != self.size.Width || content.Height != self.size.Height {
            self.size = content;
            self.pool.Recreate(&self.winrt_device, DirectXPixelFormat::B8G8R8A8UIntNormalized, 2, content)?;
        }

        let access: IDirect3DDxgiInterfaceAccess = frame.Surface()?.cast()?;
        let texture: ID3D11Texture2D = unsafe { access.GetInterface()? };
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { texture.GetDesc(&mut desc) };
        let ticks = frame.SystemRelativeTime()?.Duration; // 100 ns units
        Ok(Some(Frame {
            texture,
            width: desc.Width,
            height: desc.Height,
            time: Duration::from_nanos(ticks.max(0) as u64 * 100),
            inner: frame,
        }))
    }

    /// Copies a frame to the CPU and writes it as a PNG. Slow; for testing only.
    pub fn save_png(&self, frame: &Frame, path: &Path) -> Result<()> {
        unsafe {
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            frame.texture.GetDesc(&mut desc);
            if desc.Format != DXGI_FORMAT_B8G8R8A8_UNORM {
                bail!("unexpected texture format {:?}", desc.Format);
            }
            desc.Usage = D3D11_USAGE_STAGING;
            desc.BindFlags = 0;
            desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
            desc.MiscFlags = 0;
            let mut staging = None;
            self.device.CreateTexture2D(&desc, None, Some(&mut staging))?;
            let staging = staging.context("no staging texture")?;
            self.context.CopyResource(&staging, &frame.texture);

            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
            let (w, h) = (desc.Width as usize, desc.Height as usize);
            let mut rgba = vec![0u8; w * h * 4];
            for y in 0..h {
                let row =
                    std::slice::from_raw_parts((mapped.pData as *const u8).add(y * mapped.RowPitch as usize), w * 4);
                for x in 0..w {
                    let (b, g, r) = (row[x * 4], row[x * 4 + 1], row[x * 4 + 2]);
                    rgba[(y * w + x) * 4..][..4].copy_from_slice(&[r, g, b, 255]);
                }
            }
            self.context.Unmap(&staging, 0);

            let file = std::io::BufWriter::new(std::fs::File::create(path)?);
            let mut enc = png::Encoder::new(file, w as u32, h as u32);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            enc.write_header()?.write_image_data(&rgba)?;
        }
        Ok(())
    }
}

impl Drop for Capturer {
    fn drop(&mut self) {
        let _ = self.session.Close();
        let _ = self.pool.Close();
    }
}
