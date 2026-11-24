//! Attaching the virtual monitor to the desktop and detaching it again,
//! with Windows' display configuration API (no administrator rights needed).
//!
//! Attaching is never saved in Windows' display database, so after a crash
//! plus a restart Windows goes back to the saved (detached) layout. Detaching
//! is saved. The layout from before attaching is kept in a file and restored
//! when possible, so other monitors end up exactly where they were.

use anyhow::{bail, Context, Result};
use std::collections::HashSet;
use std::path::PathBuf;
use windows::Win32::Devices::Display::*;
use windows::Win32::Foundation::{ERROR_SUCCESS, POINTL};
use windows::Win32::Graphics::Gdi::{DISPLAYCONFIG_PATH_ACTIVE, DISPLAYCONFIG_PATH_MODE_IDX_INVALID};

type Path = DISPLAYCONFIG_PATH_INFO;
type Mode = DISPLAYCONFIG_MODE_INFO;

#[derive(Debug, Clone)]
pub struct VirtualDisplay {
    /// True if it is part of the desktop right now.
    pub active: bool,
    /// Windows device name like `\\.\DISPLAY3` (only while active).
    pub gdi_name: Option<String>,
    pub width: u32,
    pub height: u32,
    pub left: i32,
    pub top: i32,
}

fn query(flags: QUERY_DISPLAY_CONFIG_FLAGS) -> Result<(Vec<Path>, Vec<Mode>)> {
    for _ in 0..5 {
        let (mut np, mut nm) = (0u32, 0u32);
        unsafe {
            let r = GetDisplayConfigBufferSizes(flags, &mut np, &mut nm);
            if r != ERROR_SUCCESS {
                bail!("reading the display layout failed ({r:?})");
            }
            let mut paths = vec![Path::default(); np as usize];
            let mut modes = vec![Mode::default(); nm as usize];
            let r = QueryDisplayConfig(flags, &mut np, paths.as_mut_ptr(), &mut nm, modes.as_mut_ptr(), None);
            if r == ERROR_SUCCESS {
                paths.truncate(np as usize);
                modes.truncate(nm as usize);
                return Ok((paths, modes));
            }
            // ERROR_INSUFFICIENT_BUFFER: the layout changed meanwhile; retry.
        }
    }
    bail!("the display layout kept changing")
}

fn wide(s: &[u16]) -> String {
    let len = s.iter().position(|&c| c == 0).unwrap_or(s.len());
    String::from_utf16_lossy(&s[..len])
}

fn target_name(p: &Path) -> Option<DISPLAYCONFIG_TARGET_DEVICE_NAME> {
    let mut n = DISPLAYCONFIG_TARGET_DEVICE_NAME::default();
    n.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME;
    n.header.size = std::mem::size_of_val(&n) as u32;
    n.header.adapterId = p.targetInfo.adapterId;
    n.header.id = p.targetInfo.id;
    (unsafe { DisplayConfigGetDeviceInfo(&mut n.header) } == 0).then_some(n)
}

fn source_gdi_name(p: &Path) -> Option<String> {
    let mut n = DISPLAYCONFIG_SOURCE_DEVICE_NAME::default();
    n.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME;
    n.header.size = std::mem::size_of_val(&n) as u32;
    n.header.adapterId = p.sourceInfo.adapterId;
    n.header.id = p.sourceInfo.id;
    (unsafe { DisplayConfigGetDeviceInfo(&mut n.header) } == 0).then(|| wide(&n.viewGdiDeviceName))
}

/// Is this path's monitor the driver's virtual one? Its EDID manufacturer
/// is "MTT", which shows in the device path: `\\?\DISPLAY#MTT1337#...`.
fn is_virtual(p: &Path) -> bool {
    target_name(p).is_some_and(|n| {
        let path = wide(&n.monitorDevicePath).to_ascii_uppercase();
        path.split('#').nth(1).is_some_and(|id| id.starts_with("MTT"))
    })
}

unsafe fn source_idx(p: &Path) -> u32 {
    p.sourceInfo.Anonymous.modeInfoIdx
}

fn source_mode(p: &Path, modes: &[Mode]) -> Option<DISPLAYCONFIG_SOURCE_MODE> {
    let i = unsafe { source_idx(p) } as usize;
    let m = modes.get(i)?;
    (m.infoType == DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE).then_some(unsafe { m.Anonymous.sourceMode })
}

/// The virtual monitor's state, or None if the driver isn't installed/running.
pub fn virtual_display() -> Result<Option<VirtualDisplay>> {
    let (active, modes) = query(QDC_ONLY_ACTIVE_PATHS)?;
    if let Some(p) = active.iter().find(|p| is_virtual(p)) {
        let m = source_mode(p, &modes).unwrap_or_default();
        return Ok(Some(VirtualDisplay {
            active: true,
            gdi_name: source_gdi_name(p),
            width: m.width,
            height: m.height,
            left: m.position.x,
            top: m.position.y,
        }));
    }
    let (all, _) = query(QDC_ALL_PATHS)?;
    Ok(all.iter().any(|p| p.targetInfo.targetAvailable.as_bool() && is_virtual(p)).then_some(VirtualDisplay {
        active: false,
        gdi_name: None,
        width: 0,
        height: 0,
        left: 0,
        top: 0,
    }))
}

/// Keeps only the modes the paths use, and renumbers the paths' indexes.
fn compact(paths: &mut [Path], modes: &[Mode]) -> Vec<Mode> {
    let mut out = Vec::new();
    let mut remap = |idx: &mut u32| {
        if *idx == DISPLAYCONFIG_PATH_MODE_IDX_INVALID {
            return;
        }
        match modes.get(*idx as usize) {
            Some(m) => {
                out.push(*m);
                *idx = (out.len() - 1) as u32;
            }
            None => *idx = DISPLAYCONFIG_PATH_MODE_IDX_INVALID,
        }
    };
    for p in paths.iter_mut() {
        unsafe {
            remap(&mut p.sourceInfo.Anonymous.modeInfoIdx);
            remap(&mut p.targetInfo.Anonymous.modeInfoIdx);
        }
    }
    out
}

fn apply(paths: &[Path], modes: Option<&[Mode]>, flags: SET_DISPLAY_CONFIG_FLAGS) -> Result<()> {
    let r = unsafe { SetDisplayConfig(Some(paths), modes, flags) };
    if r != 0 {
        bail!("Windows refused the display layout (error {r})");
    }
    Ok(())
}

// ---- Saved layout from before attaching --------------------------------

fn saved_layout_file() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join("Gamennetto").join("layout-before-extend.bin")
}

fn as_bytes<T>(v: &[T]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, std::mem::size_of_val(v)) }
}

fn from_bytes<T: Copy + Default>(b: &[u8]) -> Vec<T> {
    let n = b.len() / std::mem::size_of::<T>();
    let mut v = vec![T::default(); n];
    unsafe { std::ptr::copy_nonoverlapping(b.as_ptr(), v.as_mut_ptr() as *mut u8, n * std::mem::size_of::<T>()) };
    v
}

fn save_layout(paths: &[Path], modes: &[Mode]) -> Result<()> {
    let file = saved_layout_file();
    std::fs::create_dir_all(file.parent().context("no folder")?)?;
    let mut out = Vec::new();
    out.extend((paths.len() as u32).to_le_bytes());
    out.extend((modes.len() as u32).to_le_bytes());
    out.extend(as_bytes(paths));
    out.extend(as_bytes(modes));
    std::fs::write(file, out)?;
    Ok(())
}

fn load_layout() -> Option<(Vec<Path>, Vec<Mode>)> {
    let b = std::fs::read(saved_layout_file()).ok()?;
    let np = u32::from_le_bytes(b.get(0..4)?.try_into().ok()?) as usize;
    let nm = u32::from_le_bytes(b.get(4..8)?.try_into().ok()?) as usize;
    let ps = np * std::mem::size_of::<Path>();
    let ms = nm * std::mem::size_of::<Mode>();
    let rest = &b[8..];
    if rest.len() != ps + ms {
        return None;
    }
    Some((from_bytes(&rest[..ps]), from_bytes(&rest[ps..])))
}

// ---- Attach / detach -----------------------------------------------------

/// Adds the virtual monitor to the desktop at `width` x `height`, to the
/// right of the other monitors. The size must be one the driver offers.
/// Returns its Windows device name (e.g. `\\.\DISPLAY3`).
pub fn attach(width: u32, height: u32) -> Result<VirtualDisplay> {
    let (mut active, modes) = query(QDC_ONLY_ACTIVE_PATHS)?;
    if !active.iter().any(is_virtual) {
        save_layout(&active, &modes)?;
        let (all, _) = query(QDC_ALL_PATHS)?;
        let used: HashSet<(u32, i32, u32)> = active
            .iter()
            .map(|p| (p.sourceInfo.adapterId.LowPart, p.sourceInfo.adapterId.HighPart, p.sourceInfo.id))
            .collect();
        let mut cand = *all
            .iter()
            .filter(|p| p.targetInfo.targetAvailable.as_bool() && is_virtual(p))
            .find(|p| {
                !used.contains(&(p.sourceInfo.adapterId.LowPart, p.sourceInfo.adapterId.HighPart, p.sourceInfo.id))
            })
            .context("the virtual monitor is not available (is the driver installed and running?)")?;
        cand.flags |= DISPLAYCONFIG_PATH_ACTIVE;
        cand.sourceInfo.Anonymous.modeInfoIdx = DISPLAYCONFIG_PATH_MODE_IDX_INVALID;
        cand.targetInfo.Anonymous.modeInfoIdx = DISPLAYCONFIG_PATH_MODE_IDX_INVALID;
        active.push(cand);

        // Keep the other monitors' modes; let Windows pick the new one's.
        let mut paths = active.clone();
        let m = compact(&mut paths, &modes);
        let first = apply(&paths, Some(&m), SDC_APPLY | SDC_USE_SUPPLIED_DISPLAY_CONFIG | SDC_ALLOW_CHANGES);
        if first.is_err() {
            for p in active.iter_mut() {
                p.sourceInfo.Anonymous.modeInfoIdx = DISPLAYCONFIG_PATH_MODE_IDX_INVALID;
                p.targetInfo.Anonymous.modeInfoIdx = DISPLAYCONFIG_PATH_MODE_IDX_INVALID;
            }
            apply(&active, None, SDC_APPLY | SDC_TOPOLOGY_SUPPLIED | SDC_ALLOW_PATH_ORDER_CHANGES)
                .context("adding the virtual monitor to the desktop")?;
        }
    }
    place(width, height)?;
    virtual_display()?.filter(|v| v.active).context("the virtual monitor did not turn on")
}

/// Sets the virtual monitor's size and puts it to the right of the others.
fn place(width: u32, height: u32) -> Result<()> {
    let (mut paths, modes) = query(QDC_ONLY_ACTIVE_PATHS)?;
    let Some(vi) = paths.iter().position(is_virtual) else { bail!("the virtual monitor is not on") };
    if let Some(m) = source_mode(&paths[vi], &modes) {
        if m.width == width && m.height == height {
            return Ok(());
        }
    }
    let right = paths
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != vi)
        .filter_map(|(_, p)| source_mode(p, &modes))
        .map(|m| m.position.x + m.width as i32)
        .max()
        .unwrap_or(0);
    let mut modes = modes;
    let si = unsafe { source_idx(&paths[vi]) } as usize;
    let Some(sm) = modes.get_mut(si) else { bail!("the virtual monitor has no mode") };
    sm.Anonymous.sourceMode.width = width;
    sm.Anonymous.sourceMode.height = height;
    sm.Anonymous.sourceMode.position = POINTL { x: right, y: 0 };
    // Let Windows choose the matching signal timing for the new size.
    paths[vi].targetInfo.Anonymous.modeInfoIdx = DISPLAYCONFIG_PATH_MODE_IDX_INVALID;
    let m = compact(&mut paths, &modes);
    apply(&paths, Some(&m), SDC_APPLY | SDC_USE_SUPPLIED_DISPLAY_CONFIG | SDC_ALLOW_CHANGES)
        .with_context(|| format!("setting the virtual monitor to {width}x{height}"))
}

/// Removes the virtual monitor from the desktop and saves that, restoring
/// the layout from before attaching when possible. Returns false if it was
/// not attached.
pub fn detach() -> Result<bool> {
    let (active, modes) = query(QDC_ONLY_ACTIVE_PATHS)?;
    if !active.iter().any(is_virtual) {
        let _ = std::fs::remove_file(saved_layout_file());
        return Ok(false);
    }
    let save = SDC_APPLY | SDC_USE_SUPPLIED_DISPLAY_CONFIG | SDC_ALLOW_CHANGES | SDC_SAVE_TO_DATABASE;

    // 1. The exact layout from before (fails if monitors changed since,
    //    or after a restart, which renumbers the GPUs).
    if let Some((p, m)) = load_layout() {
        if !p.iter().any(is_virtual) && apply(&p, Some(&m), save).is_ok() {
            let _ = std::fs::remove_file(saved_layout_file());
            return Ok(true);
        }
    }
    // 2. The current layout minus the virtual monitor.
    let mut keep: Vec<Path> = active.iter().copied().filter(|p| !is_virtual(p)).collect();
    let main_left = keep.iter().any(|p| source_mode(p, &modes).is_some_and(|m| m.position.x == 0 && m.position.y == 0));
    if main_left {
        let m = compact(&mut keep, &modes);
        if apply(&keep, Some(&m), save).is_ok() {
            let _ = std::fs::remove_file(saved_layout_file());
            return Ok(true);
        }
    }
    // 3. Same monitors, Windows picks the modes; then save what it chose.
    for p in keep.iter_mut() {
        p.sourceInfo.Anonymous.modeInfoIdx = DISPLAYCONFIG_PATH_MODE_IDX_INVALID;
        p.targetInfo.Anonymous.modeInfoIdx = DISPLAYCONFIG_PATH_MODE_IDX_INVALID;
    }
    apply(&keep, None, SDC_APPLY | SDC_TOPOLOGY_SUPPLIED | SDC_ALLOW_PATH_ORDER_CHANGES)
        .context("removing the virtual monitor from the desktop")?;
    let (now, modes) = query(QDC_ONLY_ACTIVE_PATHS)?;
    let _ = apply(&now, Some(&modes), save);
    let _ = std::fs::remove_file(saved_layout_file());
    Ok(true)
}

/// Every monitor Windows knows (name and device path), for diagnostics.
pub fn known_monitors() -> Vec<String> {
    let Ok((all, _)) = query(QDC_ALL_PATHS) else { return vec![] };
    let mut out: Vec<String> = all
        .iter()
        .filter(|p| p.targetInfo.targetAvailable.as_bool())
        .filter_map(target_name)
        .map(|n| format!("{} | {}", wide(&n.monitorFriendlyDeviceName), wide(&n.monitorDevicePath)))
        .collect();
    out.sort();
    out.dedup();
    out
}
