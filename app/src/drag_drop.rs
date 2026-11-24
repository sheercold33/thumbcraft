use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragOutcome {
    Dropped,
    Cancelled,
}

/// Returns `true` if the OS mouse cursor is currently outside the active application window's
/// client area (used to hand off smoothly from internal folder-move drag to native OS `DoDragDrop`).
pub fn is_cursor_outside_active_window() -> bool {
    #[cfg(windows)]
    {
        win_ole_dnd::is_cursor_outside_active_window()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Returns `true` if the primary mouse button is currently held down at the OS level,
/// even if the app window just lost focus (e.g., during Alt+Tab).
pub fn is_primary_mouse_button_down() -> bool {
    #[cfg(windows)]
    {
        win_ole_dnd::is_primary_mouse_button_down()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Returns `true` if a window-switching modifier key (Alt or Win) is currently held down
/// at the OS level, allowing immediate handoff to native OLE `DoDragDrop` before Alt+Tab
/// switches focus away from the window.
pub fn is_switch_modifier_down() -> bool {
    #[cfg(windows)]
    {
        win_ole_dnd::is_switch_modifier_down()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Starts a native OS file drag-and-drop operation for `paths` so the user can drag-and-drop
/// single or multiple files directly into Photoshop, CorelDRAW, Windows Explorer, or any other app.
pub fn start_file_drag(paths: &[PathBuf]) -> Result<DragOutcome, String> {
    let valid: Vec<PathBuf> = paths.iter().filter(|p| p.is_file()).cloned().collect();
    if valid.is_empty() {
        return Err("No valid files to drag".to_string());
    }

    #[cfg(windows)]
    {
        win_ole_dnd::do_shell_file_drag(&valid)
    }
    #[cfg(not(windows))]
    {
        let _ = valid;
        Err("External OS file drag-and-drop is only supported on Windows".to_string())
    }
}

#[cfg(windows)]
mod win_ole_dnd {
    use super::DragOutcome;
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};

    const S_OK: i32 = 0;
    const S_FALSE: i32 = 1;
    const E_NOINTERFACE: i32 = 0x8000_4002u32 as i32;
    const DRAGDROP_S_DROP: i32 = 0x0004_0100;
    const DRAGDROP_S_CANCEL: i32 = 0x0004_0101;
    const DRAGDROP_S_USEDEFAULTCURSORS: i32 = 0x0004_0102;

    const MK_LBUTTON: u32 = 0x0001;
    const DROPEFFECT_COPY: u32 = 1;
    const DROPEFFECT_LINK: u32 = 4;

    const VK_LBUTTON: i32 = 0x01;
    const VK_RBUTTON: i32 = 0x02;
    const VK_MENU: i32 = 0x12; // Alt
    const VK_LWIN: i32 = 0x5B;
    const VK_RWIN: i32 = 0x5C;
    const SM_SWAPBUTTON: i32 = 23;
    const WM_LBUTTONUP: u32 = 0x0202;

    #[repr(C)]
    #[derive(Clone, Copy, PartialEq, Eq)]
    struct Guid {
        data1: u32,
        data2: u16,
        data3: u16,
        data4: [u8; 8],
    }

    // IID_IUnknown = {00000000-0000-0000-C000-000000000046}
    const IID_IUNKNOWN: Guid = Guid {
        data1: 0x0000_0000,
        data2: 0x0000,
        data3: 0x0000,
        data4: [0xc0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
    };

    // IID_IDataObject = {0000010E-0000-0000-C000-000000000046}
    const IID_IDATAOBJECT: Guid = Guid {
        data1: 0x0000_010e,
        data2: 0x0000,
        data3: 0x0000,
        data4: [0xc0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
    };

    // IID_IDropSource = {00000121-0000-0000-C000-000000000046}
    const IID_IDROPSOURCE: Guid = Guid {
        data1: 0x0000_0121,
        data2: 0x0000,
        data3: 0x0000,
        data4: [0xc0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
    };

    // BHID_DataObject = {B8C0BD9F-ED24-455C-83E6-D5390C4FE8C4}
    const BHID_DATAOBJECT: Guid = Guid {
        data1: 0xb8c0_bd9f,
        data2: 0xed24,
        data3: 0x455c,
        data4: [0x83, 0xe6, 0xd5, 0x39, 0x0c, 0x4f, 0xe8, 0xc4],
    };

    #[repr(C)]
    struct IUnknownVtbl {
        query_interface:
            unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> i32,
        add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
        release: unsafe extern "system" fn(*mut c_void) -> u32,
    }

    #[repr(C)]
    struct IShellItemArrayVtbl {
        unk: IUnknownVtbl,
        bind_to_handler: unsafe extern "system" fn(
            *mut c_void,
            *mut c_void,
            *const Guid,
            *const Guid,
            *mut *mut c_void,
        ) -> i32,
    }

    #[repr(C)]
    struct IShellItemArray {
        vtbl: *const IShellItemArrayVtbl,
    }

    #[repr(C)]
    struct IDropSourceVtbl {
        query_interface:
            unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> i32,
        add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
        release: unsafe extern "system" fn(*mut c_void) -> u32,
        query_continue_drag: unsafe extern "system" fn(*mut c_void, i32, u32) -> i32,
        give_feedback: unsafe extern "system" fn(*mut c_void, u32) -> i32,
    }

    #[repr(C)]
    struct SimpleDropSource {
        vtbl: *const IDropSourceVtbl,
        ref_count: AtomicU32,
    }

    static DROP_SOURCE_VTBL: IDropSourceVtbl = IDropSourceVtbl {
        query_interface: drop_source_qi,
        add_ref: drop_source_add_ref,
        release: drop_source_release,
        query_continue_drag: drop_source_query_continue,
        give_feedback: drop_source_give_feedback,
    };

    unsafe extern "system" fn drop_source_qi(
        this: *mut c_void,
        riid: *const Guid,
        ppv: *mut *mut c_void,
    ) -> i32 {
        if ppv.is_null() || riid.is_null() {
            return E_NOINTERFACE;
        }
        let iid = *riid;
        if iid == IID_IUNKNOWN || iid == IID_IDROPSOURCE {
            *ppv = this;
            drop_source_add_ref(this);
            S_OK
        } else {
            *ppv = std::ptr::null_mut();
            E_NOINTERFACE
        }
    }

    unsafe extern "system" fn drop_source_add_ref(this: *mut c_void) -> u32 {
        let obj = &*(this as *const SimpleDropSource);
        obj.ref_count.fetch_add(1, Ordering::Relaxed) + 1
    }

    unsafe extern "system" fn drop_source_release(this: *mut c_void) -> u32 {
        let obj = &*(this as *const SimpleDropSource);
        let prev = obj.ref_count.fetch_sub(1, Ordering::AcqRel);
        if prev == 1 {
            drop(Box::from_raw(this as *mut SimpleDropSource));
            0
        } else {
            prev - 1
        }
    }

    unsafe extern "system" fn drop_source_query_continue(
        _this: *mut c_void,
        f_escape_pressed: i32,
        grf_key_state: u32,
    ) -> i32 {
        if f_escape_pressed != 0 {
            DRAGDROP_S_CANCEL
        } else if (grf_key_state & MK_LBUTTON) == 0 {
            DRAGDROP_S_DROP
        } else {
            S_OK
        }
    }

    unsafe extern "system" fn drop_source_give_feedback(_this: *mut c_void, _dw_effect: u32) -> i32 {
        DRAGDROP_S_USEDEFAULTCURSORS
    }

    fn create_drop_source() -> *mut c_void {
        let boxed = Box::new(SimpleDropSource {
            vtbl: &DROP_SOURCE_VTBL,
            ref_count: AtomicU32::new(1),
        });
        Box::into_raw(boxed) as *mut c_void
    }

    type FnILCreateFromPathW = unsafe extern "system" fn(*const u16) -> *mut c_void;
    type FnILFree = unsafe extern "system" fn(*mut c_void);
    type FnSHCreateShellItemArrayFromIDLists =
        unsafe extern "system" fn(u32, *const *const c_void, *mut *mut c_void) -> i32;

    #[link(name = "ole32")]
    extern "system" {
        fn OleInitialize(pv_reserved: *mut c_void) -> i32;
        fn OleUninitialize();
        fn DoDragDrop(
            p_data_obj: *mut c_void,
            p_drop_source: *mut c_void,
            dw_ok_effects: u32,
            pdw_effect: *mut u32,
        ) -> i32;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn LoadLibraryW(lp_lib_file_name: *const u16) -> *mut c_void;
        fn GetProcAddress(h_module: *mut c_void, lp_proc_name: *const u8) -> *mut c_void;
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Point {
        x: i32,
        y: i32,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Rect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }

    #[link(name = "user32")]
    extern "system" {
        fn GetActiveWindow() -> *mut c_void;
        fn ReleaseCapture() -> i32;
        fn GetAsyncKeyState(v_key: i32) -> i16;
        fn GetSystemMetrics(n_index: i32) -> i32;
        fn PostMessageW(hwnd: *mut c_void, msg: u32, wparam: usize, lparam: isize) -> i32;
        fn GetCursorPos(lp_point: *mut Point) -> i32;
        fn GetClientRect(hwnd: *mut c_void, lp_rect: *mut Rect) -> i32;
        fn ClientToScreen(hwnd: *mut c_void, lp_point: *mut Point) -> i32;
    }

    pub fn is_primary_mouse_button_down() -> bool {
        unsafe {
            let vk = if GetSystemMetrics(SM_SWAPBUTTON) != 0 {
                VK_RBUTTON
            } else {
                VK_LBUTTON
            };
            ((GetAsyncKeyState(vk) as u16) & 0x8000) != 0
        }
    }

    pub fn is_switch_modifier_down() -> bool {
        unsafe {
            let down = |vk: i32| ((GetAsyncKeyState(vk) as u16) & 0x8000) != 0;
            down(VK_MENU) || down(VK_LWIN) || down(VK_RWIN)
        }
    }

    pub fn is_cursor_outside_active_window() -> bool {
        unsafe {
            let hwnd = GetActiveWindow();
            if hwnd.is_null() {
                return false;
            }
            let mut cursor = Point { x: 0, y: 0 };
            if GetCursorPos(&mut cursor) == 0 {
                return false;
            }
            let mut rc = Rect {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            };
            if GetClientRect(hwnd, &mut rc) == 0 {
                return false;
            }
            let mut tl = Point {
                x: rc.left,
                y: rc.top,
            };
            let mut br = Point {
                x: rc.right,
                y: rc.bottom,
            };
            if ClientToScreen(hwnd, &mut tl) == 0 || ClientToScreen(hwnd, &mut br) == 0 {
                return false;
            }
            cursor.x < tl.x || cursor.x > br.x || cursor.y < tl.y || cursor.y > br.y
        }
    }

    struct OleGuard {
        should_uninit: bool,
    }

    impl OleGuard {
        fn init() -> Result<Self, String> {
            let hr = unsafe { OleInitialize(std::ptr::null_mut()) };
            if hr == S_OK || hr == S_FALSE {
                Ok(Self {
                    should_uninit: true,
                })
            } else {
                Err(format!("OleInitialize failed: 0x{:08X}", hr as u32))
            }
        }
    }

    impl Drop for OleGuard {
        fn drop(&mut self) {
            if self.should_uninit {
                unsafe {
                    OleUninitialize();
                }
            }
        }
    }

    fn to_wide_nul_str(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn to_win32_wide_path(path: &Path) -> Vec<u16> {
        let abs = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let s = abs.to_string_lossy();
        let clean = if let Some(unc) = s.strip_prefix(r"\\?\UNC\") {
            format!(r"\\{unc}")
        } else if let Some(local) = s.strip_prefix(r"\\?\") {
            local.to_string()
        } else {
            s.into_owned()
        };
        std::ffi::OsStr::new(&clean)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    /// Creates a genuine Windows Shell `IDataObject` (supporting CF_HDROP, Shell IDList Array,
    /// FileNameW, etc.) for one or more files, even across different folders.
    pub(crate) fn create_shell_data_object(paths: &[PathBuf]) -> Result<*mut c_void, String> {
        let shell32_name = to_wide_nul_str("shell32.dll");
        let h_shell32 = unsafe { LoadLibraryW(shell32_name.as_ptr()) };
        if h_shell32.is_null() {
            return Err("Failed to load shell32.dll".to_string());
        }

        let p_il_create = unsafe { GetProcAddress(h_shell32, b"ILCreateFromPathW\0".as_ptr()) };
        let p_il_free = unsafe { GetProcAddress(h_shell32, b"ILFree\0".as_ptr()) };
        let p_create_array = unsafe {
            GetProcAddress(
                h_shell32,
                b"SHCreateShellItemArrayFromIDLists\0".as_ptr(),
            )
        };
        if p_il_create.is_null() || p_il_free.is_null() || p_create_array.is_null() {
            return Err("Required Windows Shell drag-and-drop APIs unavailable".to_string());
        }

        let il_create: FnILCreateFromPathW = unsafe { std::mem::transmute(p_il_create) };
        let il_free: FnILFree = unsafe { std::mem::transmute(p_il_free) };
        let create_array: FnSHCreateShellItemArrayFromIDLists =
            unsafe { std::mem::transmute(p_create_array) };

        let mut pidls: Vec<*mut c_void> = Vec::with_capacity(paths.len());
        for p in paths {
            let wide = to_win32_wide_path(p);
            let pidl = unsafe { il_create(wide.as_ptr()) };
            if !pidl.is_null() {
                pidls.push(pidl);
            }
        }
        if pidls.is_empty() {
            return Err("Could not resolve Shell PIDLs for dragged files".to_string());
        }

        let mut item_array_raw: *mut c_void = std::ptr::null_mut();
        let pidl_const_ptrs: Vec<*const c_void> = pidls.iter().map(|&p| p as *const c_void).collect();
        let hr_arr = unsafe {
            create_array(
                pidl_const_ptrs.len() as u32,
                pidl_const_ptrs.as_ptr(),
                &mut item_array_raw,
            )
        };

        for pidl in pidls {
            unsafe {
                il_free(pidl);
            }
        }

        if hr_arr < 0 || item_array_raw.is_null() {
            return Err(format!(
                "SHCreateShellItemArrayFromIDLists failed: 0x{:08X}",
                hr_arr as u32
            ));
        }

        let item_array = item_array_raw as *mut IShellItemArray;
        let mut data_obj: *mut c_void = std::ptr::null_mut();
        let hr_bind = unsafe {
            ((*(*item_array).vtbl).bind_to_handler)(
                item_array_raw,
                std::ptr::null_mut(),
                &BHID_DATAOBJECT,
                &IID_IDATAOBJECT,
                &mut data_obj,
            )
        };
        unsafe {
            ((*(*item_array).vtbl).unk.release)(item_array_raw);
        }

        if hr_bind < 0 || data_obj.is_null() {
            return Err(format!(
                "IShellItemArray::BindToHandler(BHID_DataObject) failed: 0x{:08X}",
                hr_bind as u32
            ));
        }
        Ok(data_obj)
    }

    pub(crate) unsafe fn release_com(ptr: *mut c_void) {
        if !ptr.is_null() {
            let vtbl = *(ptr as *const *const IUnknownVtbl);
            ((*vtbl).release)(ptr);
        }
    }

    pub fn do_shell_file_drag(paths: &[PathBuf]) -> Result<DragOutcome, String> {
        let _ole = OleGuard::init()?;
        let active_hwnd = unsafe { GetActiveWindow() };
        let data_obj = create_shell_data_object(paths)?;
        let drop_source = create_drop_source();

        let mut effect: u32 = 0;
        let hr = unsafe {
            DoDragDrop(
                data_obj,
                drop_source,
                DROPEFFECT_COPY | DROPEFFECT_LINK,
                &mut effect,
            )
        };

        unsafe {
            release_com(data_obj);
            release_com(drop_source);
            ReleaseCapture();
            // DoDragDrop consumes WM_LBUTTONUP inside its modal loop; notify our window so winit/egui
            // cleanly releases pointer state and never sticks in a phantom drag.
            if !active_hwnd.is_null() && ((GetAsyncKeyState(VK_LBUTTON) as u16) & 0x8000) == 0 {
                PostMessageW(active_hwnd, WM_LBUTTONUP, 0, 0);
            }
        }

        if hr == DRAGDROP_S_DROP {
            Ok(DragOutcome::Dropped)
        } else if hr == DRAGDROP_S_CANCEL {
            Ok(DragOutcome::Cancelled)
        } else if hr < 0 {
            Err(format!("DoDragDrop failed: 0x{:08X}", hr as u32))
        } else {
            Ok(DragOutcome::Cancelled)
        }
    }

    #[cfg(test)]
    pub(crate) fn verify_data_object_supports_hdrop(paths: &[PathBuf]) -> Result<(), String> {
        const CF_HDROP: u16 = 15;
        const DVASPECT_CONTENT: u32 = 1;
        const TYMED_HGLOBAL: u32 = 1;

        #[repr(C)]
        struct FormatEtc {
            cf_format: u16,
            ptd: *mut c_void,
            dw_aspect: u32,
            lindex: i32,
            tymed: u32,
        }

        #[repr(C)]
        struct IDataObjectVtbl {
            unk: IUnknownVtbl,
            get_data: unsafe extern "system" fn(*mut c_void, *const FormatEtc, *mut c_void) -> i32,
            get_data_here:
                unsafe extern "system" fn(*mut c_void, *const FormatEtc, *mut c_void) -> i32,
            query_get_data: unsafe extern "system" fn(*mut c_void, *const FormatEtc) -> i32,
        }

        let _ole = OleGuard::init()?;
        let data_obj = create_shell_data_object(paths)?;
        let fmt = FormatEtc {
            cf_format: CF_HDROP,
            ptd: std::ptr::null_mut(),
            dw_aspect: DVASPECT_CONTENT,
            lindex: -1,
            tymed: TYMED_HGLOBAL,
        };
        let hr = unsafe {
            let vtbl = *(data_obj as *const *const IDataObjectVtbl);
            ((*vtbl).query_get_data)(data_obj, &fmt)
        };
        unsafe {
            release_com(data_obj);
        }
        if hr == S_OK {
            Ok(())
        } else {
            Err(format!("QueryGetData(CF_HDROP) returned 0x{:08X}", hr as u32))
        }
    }
}

#[cfg(test)]
mod tests {
    #[cfg(windows)]
    #[test]
    fn shell_data_object_supports_cf_hdrop_single_and_multi_folder() {
        let tmp = std::env::temp_dir().join(format!(
            "thumbcraft_dnd_test_{}",
            std::process::id()
        ));
        let dir_a = tmp.join("a");
        let dir_b = tmp.join("b");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&dir_a).unwrap();
        std::fs::create_dir_all(&dir_b).unwrap();

        let f1 = dir_a.join("bitmap1.png");
        let f2 = dir_a.join("bitmap2.jpg");
        let f3 = dir_b.join("vector3.cdr");
        std::fs::write(&f1, b"1").unwrap();
        std::fs::write(&f2, b"2").unwrap();
        std::fs::write(&f3, b"3").unwrap();

        super::win_ole_dnd::verify_data_object_supports_hdrop(&[f1.clone()]).unwrap();
        super::win_ole_dnd::verify_data_object_supports_hdrop(&[f1.clone(), f2.clone()]).unwrap();
        super::win_ole_dnd::verify_data_object_supports_hdrop(&[f1, f2, f3]).unwrap();

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
