use std::path::Path;

#[derive(Debug, Clone)]
pub struct NativeShellIcon {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub type_name: String,
}

/// Returns a cache key for a file's native OS icon.
/// Most extensions share a single system icon (O(1) lookup & shared GPU texture across thousands
/// of files), while executables/shortcuts/icon files can carry per-file icons.
pub fn icon_cache_key(path: &Path) -> String {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();

    if matches!(
        ext.as_str(),
        "exe" | "ico" | "cur" | "ani" | "lnk" | "scr" | "msi" | "dll" | "ocx" | "cpl" | "url"
    ) {
        format!("path:{}", path.to_string_lossy().to_ascii_lowercase())
    } else if ext.is_empty() {
        "ext:__none__".to_string()
    } else {
        format!("ext:{ext}")
    }
}

/// Extracts the native OS icon (32-bit RGBA with alpha) and shell type display name for `path`.
pub fn extract_native_file_icon(path: &Path) -> Option<NativeShellIcon> {
    #[cfg(windows)]
    {
        win_shell_icon::extract_native_file_icon(path)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        None
    }
}

#[cfg(windows)]
mod win_shell_icon {
    use super::NativeShellIcon;
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    const S_OK: i32 = 0;
    const S_FALSE: i32 = 1;
    const COINIT_APARTMENTTHREADED: u32 = 0x2;

    const FILE_ATTRIBUTE_NORMAL: u32 = 0x0000_0080;
    const SHGFI_ICON: u32 = 0x0000_0100;
    const SHGFI_LARGEICON: u32 = 0x0000_0000;
    const SHGFI_TYPENAME: u32 = 0x0000_0400;
    const SHGFI_SYSICONINDEX: u32 = 0x0000_4000;
    const SHGFI_USEFILEATTRIBUTES: u32 = 0x0000_0010;

    const SHIL_EXTRALARGE: i32 = 2; // 48x48
    const SHIL_JUMBO: i32 = 4; // 256x256
    const ILD_TRANSPARENT: u32 = 0x0000_0001;

    const BI_RGB: u32 = 0;
    const DIB_RGB_COLORS: u32 = 0;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Guid {
        data1: u32,
        data2: u16,
        data3: u16,
        data4: [u8; 8],
    }

    // IID_IImageList = {46EB5926-582E-4017-9FDF-E8998DAA0950}
    const IID_IIMAGELIST: Guid = Guid {
        data1: 0x46eb_5926,
        data2: 0x582e,
        data3: 0x4017,
        data4: [0x9f, 0xdf, 0xe8, 0x99, 0x8d, 0xaa, 0x09, 0x50],
    };

    #[repr(C)]
    struct ShFileInfoW {
        h_icon: *mut c_void,
        i_icon: i32,
        dw_attributes: u32,
        sz_display_name: [u16; 260],
        sz_type_name: [u16; 80],
    }

    #[repr(C)]
    struct IconInfo {
        f_icon: i32,
        x_hotspot: u32,
        y_hotspot: u32,
        hbm_mask: *mut c_void,
        hbm_color: *mut c_void,
    }

    #[repr(C)]
    struct Bitmap {
        bm_type: i32,
        bm_width: i32,
        bm_height: i32,
        bm_width_bytes: i32,
        bm_planes: u16,
        bm_bits_pixel: u16,
        bm_bits: *mut c_void,
    }

    #[repr(C)]
    struct BitmapInfoHeader {
        bi_size: u32,
        bi_width: i32,
        bi_height: i32,
        bi_planes: u16,
        bi_bit_count: u16,
        bi_compression: u32,
        bi_size_image: u32,
        bi_x_pels_per_meter: i32,
        bi_y_pels_per_meter: i32,
        bi_clr_used: u32,
        bi_clr_important: u32,
    }

    #[repr(C)]
    struct BitmapInfo {
        bmi_header: BitmapInfoHeader,
        bmi_colors: [u32; 1],
    }

    // Minimal prefix of IImageList vtable up to GetIcon (index 10)
    #[repr(C)]
    struct IImageListVtbl {
        query_interface:
            unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> i32,
        add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
        release: unsafe extern "system" fn(*mut c_void) -> u32,
        add: *const c_void,              // 3
        replace_icon: *const c_void,     // 4
        set_overlay_image: *const c_void,// 5
        replace: *const c_void,          // 6
        add_masked: *const c_void,       // 7
        draw: *const c_void,             // 8
        remove: *const c_void,           // 9
        get_icon: unsafe extern "system" fn(*mut c_void, i32, u32, *mut *mut c_void) -> i32, // 10
    }

    #[repr(C)]
    struct IImageList {
        vtbl: *const IImageListVtbl,
    }

    type FnSHGetImageList = unsafe extern "system" fn(i32, *const Guid, *mut *mut c_void) -> i32;

    #[link(name = "ole32")]
    extern "system" {
        fn CoInitializeEx(pv_reserved: *mut c_void, dw_co_init: u32) -> i32;
        fn CoUninitialize();
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn LoadLibraryW(lp_lib_file_name: *const u16) -> *mut c_void;
        fn GetProcAddress(h_module: *mut c_void, lp_proc_name: *const u8) -> *mut c_void;
    }

    #[link(name = "shell32")]
    extern "system" {
        fn SHGetFileInfoW(
            psz_path: *const u16,
            dw_file_attributes: u32,
            psfi: *mut ShFileInfoW,
            cb_file_info: u32,
            u_flags: u32,
        ) -> usize;
    }

    #[link(name = "user32")]
    extern "system" {
        fn GetIconInfo(h_icon: *mut c_void, piconinfo: *mut IconInfo) -> i32;
        fn DestroyIcon(h_icon: *mut c_void) -> i32;
        fn GetDC(hwnd: *mut c_void) -> *mut c_void;
        fn ReleaseDC(hwnd: *mut c_void, hdc: *mut c_void) -> i32;
    }

    #[link(name = "gdi32")]
    extern "system" {
        fn GetObjectW(h: *mut c_void, c: i32, pv: *mut c_void) -> i32;
        fn GetDIBits(
            hdc: *mut c_void,
            hbm: *mut c_void,
            start: u32,
            c_lines: u32,
            lpv_bits: *mut c_void,
            lpbmi: *mut BitmapInfo,
            usage: u32,
        ) -> i32;
        fn DeleteObject(ho: *mut c_void) -> i32;
    }

    struct ComGuard(bool);
    impl ComGuard {
        fn init() -> Self {
            let hr = unsafe { CoInitializeEx(std::ptr::null_mut(), COINIT_APARTMENTTHREADED) };
            Self(hr == S_OK || hr == S_FALSE)
        }
    }
    impl Drop for ComGuard {
        fn drop(&mut self) {
            if self.0 {
                unsafe {
                    CoUninitialize();
                }
            }
        }
    }

    fn to_wide_nul(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn wide_path_nul(path: &Path) -> Vec<u16> {
        path.as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    fn decode_wide_cstr(buf: &[u16]) -> String {
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..len]).trim().to_string()
    }

    fn fetch_image_list_icon(sys_idx: i32, shil_size: i32) -> *mut c_void {
        unsafe {
            let lib = LoadLibraryW(to_wide_nul("shell32.dll").as_ptr());
            if lib.is_null() {
                return std::ptr::null_mut();
            }
            let proc = GetProcAddress(lib, b"SHGetImageList\0".as_ptr());
            if proc.is_null() {
                return std::ptr::null_mut();
            }
            let sh_get_image_list: FnSHGetImageList = std::mem::transmute(proc);
            let mut img_list_raw: *mut c_void = std::ptr::null_mut();
            let hr = sh_get_image_list(shil_size, &IID_IIMAGELIST, &mut img_list_raw);
            if hr < 0 || img_list_raw.is_null() {
                return std::ptr::null_mut();
            }
            let img_list = img_list_raw as *mut IImageList;
            let mut hicon: *mut c_void = std::ptr::null_mut();
            let hr_icon =
                ((*(*img_list).vtbl).get_icon)(img_list_raw, sys_idx, ILD_TRANSPARENT, &mut hicon);
            ((*(*img_list).vtbl).release)(img_list_raw);
            if hr_icon >= 0 {
                hicon
            } else {
                std::ptr::null_mut()
            }
        }
    }

    unsafe fn hicon_to_rgba(hicon: *mut c_void) -> Option<(u32, u32, Vec<u8>)> {
        if hicon.is_null() {
            return None;
        }
        let mut info: IconInfo = std::mem::zeroed();
        if GetIconInfo(hicon, &mut info) == 0 {
            return None;
        }

        let res = (|| -> Option<(u32, u32, Vec<u8>)> {
            let hdc = GetDC(std::ptr::null_mut());
            if hdc.is_null() {
                return None;
            }
            let out = if !info.hbm_color.is_null() {
                let mut bm: Bitmap = std::mem::zeroed();
                if GetObjectW(
                    info.hbm_color,
                    std::mem::size_of::<Bitmap>() as i32,
                    &mut bm as *mut Bitmap as *mut c_void,
                ) == 0
                {
                    None
                } else {
                    let w = bm.bm_width.max(0) as u32;
                    let h = bm.bm_height.max(0) as u32;
                    if w == 0 || h == 0 || w > 512 || h > 512 {
                        None
                    } else {
                        let mut bmi: BitmapInfo = std::mem::zeroed();
                        bmi.bmi_header.bi_size = std::mem::size_of::<BitmapInfoHeader>() as u32;
                        bmi.bmi_header.bi_width = w as i32;
                        bmi.bmi_header.bi_height = -(h as i32); // top-down DIB
                        bmi.bmi_header.bi_planes = 1;
                        bmi.bmi_header.bi_bit_count = 32;
                        bmi.bmi_header.bi_compression = BI_RGB;

                        let mut bgra = vec![0u8; (w * h * 4) as usize];
                        let lines = GetDIBits(
                            hdc,
                            info.hbm_color,
                            0,
                            h,
                            bgra.as_mut_ptr() as *mut c_void,
                            &mut bmi,
                            DIB_RGB_COLORS,
                        );
                        if lines == 0 {
                            None
                        } else {
                            let has_alpha = bgra.chunks_exact(4).any(|px| px[3] != 0);
                            if !has_alpha && !info.hbm_mask.is_null() {
                                let mut mask_bgra = vec![0u8; (w * h * 4) as usize];
                                let m_lines = GetDIBits(
                                    hdc,
                                    info.hbm_mask,
                                    0,
                                    h,
                                    mask_bgra.as_mut_ptr() as *mut c_void,
                                    &mut bmi,
                                    DIB_RGB_COLORS,
                                );
                                if m_lines > 0 {
                                    for (px, m) in
                                        bgra.chunks_exact_mut(4).zip(mask_bgra.chunks_exact(4))
                                    {
                                        // Mask 0 = opaque pixel, non-zero (white) = transparent
                                        px[3] = if m[0] == 0 { 255 } else { 0 };
                                    }
                                } else {
                                    for px in bgra.chunks_exact_mut(4) {
                                        px[3] = 255;
                                    }
                                }
                            }
                            // Convert BGRA -> RGBA
                            for px in bgra.chunks_exact_mut(4) {
                                px.swap(0, 2);
                            }
                            Some((w, h, bgra))
                        }
                    }
                }
            } else {
                None
            };
            ReleaseDC(std::ptr::null_mut(), hdc);
            out
        })();

        if !info.hbm_color.is_null() {
            DeleteObject(info.hbm_color);
        }
        if !info.hbm_mask.is_null() {
            DeleteObject(info.hbm_mask);
        }
        res
    }

    /// Windows `SHIL_JUMBO` (256x256) places legacy 32x32 or 48x48 icons in the top-left corner
    /// of a 256x256 transparent canvas. Detect and trim that empty right/bottom margin cleanly.
    fn trim_jumbo_corner_padding(w: u32, h: u32, rgba: Vec<u8>) -> (u32, u32, Vec<u8>) {
        if w < 128 || h < 128 {
            return (w, h, rgba);
        }
        let mut max_x = 0u32;
        let mut max_y = 0u32;
        let mut any = false;
        for y in 0..h {
            let row_start = (y * w * 4) as usize;
            for x in 0..w {
                if rgba[row_start + (x * 4 + 3) as usize] > 4 {
                    any = true;
                    if x > max_x {
                        max_x = x;
                    }
                    if y > max_y {
                        max_y = y;
                    }
                }
            }
        }
        if !any {
            return (w, h, rgba);
        }
        // Only trim if content sits entirely inside the top-left quadrant (classic SHIL_JUMBO quirk)
        let extent = (max_x + 1).max(max_y + 1);
        if extent <= 72 {
            let side = if extent <= 32 {
                32
            } else if extent <= 48 {
                48
            } else {
                64
            }
            .min(w)
            .min(h);
            let mut cropped = Vec::with_capacity((side * side * 4) as usize);
            for y in 0..side {
                let src_row = (y * w * 4) as usize;
                cropped.extend_from_slice(&rgba[src_row..src_row + (side * 4) as usize]);
            }
            return (side, side, cropped);
        }
        (w, h, rgba)
    }

    pub fn extract_native_file_icon(path: &Path) -> Option<NativeShellIcon> {
        let _com = ComGuard::init();
        let exists = path.exists();
        let wide = wide_path_nul(path);
        let mut shfi: ShFileInfoW = unsafe { std::mem::zeroed() };
        let base_flags = SHGFI_SYSICONINDEX | SHGFI_TYPENAME;
        let flags = if exists {
            base_flags
        } else {
            base_flags | SHGFI_USEFILEATTRIBUTES
        };

        let ok = unsafe {
            SHGetFileInfoW(
                wide.as_ptr(),
                FILE_ATTRIBUTE_NORMAL,
                &mut shfi,
                std::mem::size_of::<ShFileInfoW>() as u32,
                flags,
            )
        };

        let type_name = decode_wide_cstr(&shfi.sz_type_name);
        let mut decoded: Option<(u32, u32, Vec<u8>)> = None;

        if ok != 0 {
            // Try 256x256 jumbo icon first (best sharpness in large thumbnail cards)
            let h_jumbo = fetch_image_list_icon(shfi.i_icon, SHIL_JUMBO);
            if !h_jumbo.is_null() {
                if let Some((w, h, rgba)) = unsafe { hicon_to_rgba(h_jumbo) } {
                    let (tw, th, trgba) = trim_jumbo_corner_padding(w, h, rgba);
                    if tw >= 64 {
                        decoded = Some((tw, th, trgba));
                    } else {
                        // Legacy small icon padded into 256x256 — fetch crisp 48x48 ExtraLarge instead
                        let h_xl = fetch_image_list_icon(shfi.i_icon, SHIL_EXTRALARGE);
                        if !h_xl.is_null() {
                            decoded = unsafe { hicon_to_rgba(h_xl) };
                            unsafe {
                                DestroyIcon(h_xl);
                            }
                        }
                        if decoded.is_none() {
                            decoded = Some((tw, th, trgba));
                        }
                    }
                }
                unsafe {
                    DestroyIcon(h_jumbo);
                }
            }
        }

        // Fallback to classic SHGFI_ICON | SHGFI_LARGEICON (32x32)
        if decoded.is_none() {
            let mut shfi2: ShFileInfoW = unsafe { std::mem::zeroed() };
            let f2 = SHGFI_ICON
                | SHGFI_LARGEICON
                | SHGFI_TYPENAME
                | if exists { 0 } else { SHGFI_USEFILEATTRIBUTES };
            let ok2 = unsafe {
                SHGetFileInfoW(
                    wide.as_ptr(),
                    FILE_ATTRIBUTE_NORMAL,
                    &mut shfi2,
                    std::mem::size_of::<ShFileInfoW>() as u32,
                    f2,
                )
            };
            if ok2 != 0 && !shfi2.h_icon.is_null() {
                decoded = unsafe { hicon_to_rgba(shfi2.h_icon) };
                unsafe {
                    DestroyIcon(shfi2.h_icon);
                }
            }
        }

        let (width, height, rgba) = decoded?;
        Some(NativeShellIcon {
            width,
            height,
            rgba,
            type_name: if type_name.is_empty() {
                "File".to_string()
            } else {
                type_name
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn extracts_native_windows_icon_for_generic_file() {
        let tmp = std::env::temp_dir().join(format!(
            "thumbcraft_icon_test_{}.txt",
            std::process::id()
        ));
        std::fs::write(&tmp, b"hello").unwrap();
        let icon = extract_native_file_icon(&tmp).expect("should extract native .txt icon");
        assert!(icon.width >= 16 && icon.height >= 16);
        assert_eq!(icon.rgba.len(), (icon.width * icon.height * 4) as usize);
        assert!(icon.rgba.chunks_exact(4).any(|px| px[3] > 0));
        let _ = std::fs::remove_file(&tmp);
    }
}
