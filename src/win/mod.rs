//! Win32 pieces: input hook, screen capture, layered tooltip windows, tray, registry.
pub mod capture;
pub mod hook;
pub mod tracker;
pub mod ui;

pub mod registry {
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};

    /// `InstallLocation` of an uninstall key under HKLM.
    pub fn install_location(subkey: &str) -> Option<String> {
        let key = HSTRING::from(subkey);
        let name = HSTRING::from("InstallLocation");
        let mut buf = [0u16; 1024];
        let mut size = (buf.len() * 2) as u32;
        unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                PCWSTR(key.as_ptr()),
                PCWSTR(name.as_ptr()),
                RRF_RT_REG_SZ,
                None,
                Some(buf.as_mut_ptr().cast()),
                Some(&mut size),
            )
            .ok()
            .ok()?;
        }
        let len = (size as usize / 2).saturating_sub(1);
        Some(String::from_utf16_lossy(&buf[..len])).filter(|s| !s.is_empty())
    }
}

/// Whether process `pid` runs `EscapeFromTarkov.exe`.
fn is_eft(pid: u32) -> bool {
    use windows::core::PWSTR;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    if pid == 0 {
        return false;
    }
    unsafe {
        let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return false };
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len).is_ok();
        let _ = CloseHandle(h);
        ok && String::from_utf16_lossy(&buf[..len as usize]).to_ascii_lowercase().ends_with("escapefromtarkov.exe")
    }
}

/// Whether the foreground window belongs to `EscapeFromTarkov.exe`.
pub fn eft_in_foreground() -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
    let mut pid = 0u32;
    unsafe {
        GetWindowThreadProcessId(GetForegroundWindow(), Some(&mut pid));
    }
    is_eft(pid)
}

/// Whether any `EscapeFromTarkov.exe` process exists.
pub fn eft_running() -> bool {
    use windows::Win32::System::ProcessStatus::EnumProcesses;
    let mut pids = [0u32; 4096];
    let mut bytes = 0u32;
    let listed = unsafe { EnumProcesses(pids.as_mut_ptr(), std::mem::size_of_val(&pids) as u32, &mut bytes) };
    if listed.is_err() {
        // Cannot tell: treat as running so a raid is never cut short by a failed query.
        return true;
    }
    let n = bytes as usize / std::mem::size_of::<u32>();
    pids[..n].iter().any(|&p| is_eft(p))
}

/// Centre of the foreground window in screen coordinates; picks the monitor EFT is on.
pub fn foreground_center() -> (i32, i32) {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowRect};
    let mut r = RECT::default();
    unsafe {
        let _ = GetWindowRect(GetForegroundWindow(), &mut r);
    }
    ((r.left + r.right) / 2, (r.top + r.bottom) / 2)
}

/// Monitor rectangle (left, top, right, bottom) containing a point.
pub fn monitor_rect(x: i32, y: i32) -> (i32, i32, i32, i32) {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST};
    unsafe {
        let mon = MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(mon, &mut info);
        let r = info.rcMonitor;
        (r.left, r.top, r.right, r.bottom)
    }
}

pub fn cursor_pos() -> (i32, i32) {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
    let mut p = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut p);
    }
    (p.x, p.y)
}
