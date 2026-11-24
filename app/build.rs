use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=assets/app_icon.ico");
    println!("cargo:rerun-if-changed=assets/app_icon.rc");

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "windows" {
        return;
    }

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let ico_path = manifest_dir.join("assets").join("app_icon.ico");
    if !ico_path.is_file() {
        return;
    }

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let obj_path = out_dir.join("app_icon.res.obj");

    // Try pure-Rust COFF .rsrc object generation from app_icon.ico (works on both
    // x86_64-pc-windows-gnu and x86_64-pc-windows-msvc without needing windres/cvtres).
    if let Ok(ico_bytes) = std::fs::read(&ico_path) {
        if let Ok(coff) = build_icon_coff_x64(&ico_bytes) {
            if std::fs::write(&obj_path, coff).is_ok() {
                println!("cargo:rustc-link-arg={}", obj_path.display());
                return;
            }
        }
    }

    // Fallback: compile .rc with Windows Kits rc.exe if present
    let rc_path = manifest_dir.join("assets").join("app_icon.rc");
    let res_path = out_dir.join("app_icon.res");
    if let Some(rc_exe) = find_rc_exe() {
        if let Ok(st) = Command::new(rc_exe)
            .args([
                "/nologo",
                "/fo",
                &res_path.to_string_lossy(),
                &rc_path.to_string_lossy(),
            ])
            .status()
        {
            if st.success() && res_path.is_file() {
                println!("cargo:rustc-link-arg={}", res_path.display());
            }
        }
    }
}

fn find_rc_exe() -> Option<PathBuf> {
    let kits_bin = Path::new(r"C:\Program Files (x86)\Windows Kits\10\bin");
    if let Ok(rd) = std::fs::read_dir(kits_bin) {
        let mut versions: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
        versions.sort();
        versions.reverse();
        for v in versions {
            let candidate = v.join("x64").join("rc.exe");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Builds a valid x86_64 COFF object file containing a `.rsrc` section with:
///   - RT_ICON (type 3), IDs 1..=N
///   - RT_GROUP_ICON (type 14), ID 1
/// All data entries use IMAGE_REL_AMD64_ADDR32NB (type 3) relocations so the linker
/// automatically resolves each resource's VirtualAddress (RVA) in the final PE image.
fn build_icon_coff_x64(ico: &[u8]) -> Result<Vec<u8>, &'static str> {
    if ico.len() < 6 {
        return Err("ico too short");
    }
    let reserved = u16::from_le_bytes([ico[0], ico[1]]);
    let ico_type = u16::from_le_bytes([ico[2], ico[3]]);
    let count = u16::from_le_bytes([ico[4], ico[5]]) as usize;
    if reserved != 0 || ico_type != 1 || count == 0 || ico.len() < 6 + count * 16 {
        return Err("invalid ico header");
    }

    struct IconFrame<'a> {
        width: u8,
        height: u8,
        color_count: u8,
        reserved: u8,
        planes: u16,
        bit_count: u16,
        data: &'a [u8],
    }

    let mut frames = Vec::with_capacity(count);
    for i in 0..count {
        let off = 6 + i * 16;
        let bytes_in_res =
            u32::from_le_bytes([ico[off + 8], ico[off + 9], ico[off + 10], ico[off + 11]]) as usize;
        let img_off =
            u32::from_le_bytes([ico[off + 12], ico[off + 13], ico[off + 14], ico[off + 15]]) as usize;
        if img_off + bytes_in_res > ico.len() {
            return Err("ico frame out of bounds");
        }
        frames.push(IconFrame {
            width: ico[off],
            height: ico[off + 1],
            color_count: ico[off + 2],
            reserved: ico[off + 3],
            planes: u16::from_le_bytes([ico[off + 4], ico[off + 5]]),
            bit_count: u16::from_le_bytes([ico[off + 6], ico[off + 7]]),
            data: &ico[img_off..img_off + bytes_in_res],
        });
    }

    // Build GRPICONDIR payload for RT_GROUP_ICON (type 14, ID 1)
    let mut group_data = Vec::with_capacity(6 + frames.len() * 14);
    group_data.extend_from_slice(&0u16.to_le_bytes()); // idReserved
    group_data.extend_from_slice(&1u16.to_le_bytes()); // idType (1 = icon)
    group_data.extend_from_slice(&(frames.len() as u16).to_le_bytes());
    for (idx, f) in frames.iter().enumerate() {
        group_data.push(f.width);
        group_data.push(f.height);
        group_data.push(f.color_count);
        group_data.push(f.reserved);
        group_data.extend_from_slice(&f.planes.to_le_bytes());
        group_data.extend_from_slice(&f.bit_count.to_le_bytes());
        group_data.extend_from_slice(&(f.data.len() as u32).to_le_bytes());
        group_data.extend_from_slice(&((idx + 1) as u16).to_le_bytes()); // nID = 1..=N
    }

    // Build `.rsrc` section layout:
    //   1. Root directory (2 entries: type 3 RT_ICON, type 14 RT_GROUP_ICON)
    //   2. Type 3 directory (N entries: ID 1..=N)
    //   3. Type 14 directory (1 entry: ID 1)
    //   4. Language directories: N for RT_ICON + 1 for RT_GROUP_ICON (each 1 entry, lang 0x0409)
    //   5. IMAGE_RESOURCE_DATA_ENTRY array: N + 1 entries (16 bytes each)
    //   6. Raw payload bytes (4-byte aligned): frame[0..N] + group_data
    const DIR_HDR: usize = 16;
    const DIR_ENT: usize = 8;
    const DATA_ENT: usize = 16;
    const SUBDIR_FLAG: u32 = 0x8000_0000;

    let n = frames.len();
    let root_off = 0usize;
    let type3_off = root_off + DIR_HDR + 2 * DIR_ENT;
    let type14_off = type3_off + DIR_HDR + n * DIR_ENT;
    let lang3_base = type14_off + DIR_HDR + DIR_ENT;
    let lang14_off = lang3_base + n * (DIR_HDR + DIR_ENT);
    let data_ent_base = lang14_off + (DIR_HDR + DIR_ENT);
    let payload_base = data_ent_base + (n + 1) * DATA_ENT;

    let mut rsrc = Vec::new();
    let write_dir_hdr = |buf: &mut Vec<u8>, num_id_entries: u16| {
        buf.extend_from_slice(&0u32.to_le_bytes()); // Characteristics
        buf.extend_from_slice(&0u32.to_le_bytes()); // TimeDateStamp
        buf.extend_from_slice(&0u16.to_le_bytes()); // MajorVersion
        buf.extend_from_slice(&0u16.to_le_bytes()); // MinorVersion
        buf.extend_from_slice(&0u16.to_le_bytes()); // NumberOfNamedEntries
        buf.extend_from_slice(&num_id_entries.to_le_bytes()); // NumberOfIdEntries
    };
    let write_dir_ent = |buf: &mut Vec<u8>, id: u32, offset: u32| {
        buf.extend_from_slice(&id.to_le_bytes());
        buf.extend_from_slice(&offset.to_le_bytes());
    };

    // 1. Root directory (sorted by ID: 3 then 14)
    write_dir_hdr(&mut rsrc, 2);
    write_dir_ent(&mut rsrc, 3, SUBDIR_FLAG | type3_off as u32);
    write_dir_ent(&mut rsrc, 14, SUBDIR_FLAG | type14_off as u32);

    // 2. RT_ICON (3) name/ID directory
    debug_assert_eq!(rsrc.len(), type3_off);
    write_dir_hdr(&mut rsrc, n as u16);
    for i in 0..n {
        let lang_off = lang3_base + i * (DIR_HDR + DIR_ENT);
        write_dir_ent(&mut rsrc, (i + 1) as u32, SUBDIR_FLAG | lang_off as u32);
    }

    // 3. RT_GROUP_ICON (14) name/ID directory
    debug_assert_eq!(rsrc.len(), type14_off);
    write_dir_hdr(&mut rsrc, 1);
    write_dir_ent(&mut rsrc, 1, SUBDIR_FLAG | lang14_off as u32);

    // 4. Language directories (US English 0x0409)
    debug_assert_eq!(rsrc.len(), lang3_base);
    for i in 0..n {
        write_dir_hdr(&mut rsrc, 1);
        write_dir_ent(&mut rsrc, 0x0409, (data_ent_base + i * DATA_ENT) as u32);
    }
    debug_assert_eq!(rsrc.len(), lang14_off);
    write_dir_hdr(&mut rsrc, 1);
    write_dir_ent(&mut rsrc, 0x0409, (data_ent_base + n * DATA_ENT) as u32);

    // Compute payload offsets (4-byte aligned)
    let mut payload_offsets = Vec::with_capacity(n + 1);
    let mut cur_payload = payload_base;
    for f in &frames {
        payload_offsets.push(cur_payload as u32);
        cur_payload = (cur_payload + f.data.len() + 3) & !3;
    }
    payload_offsets.push(cur_payload as u32);

    // 5. Data entries + collect IMAGE_REL_AMD64_ADDR32NB relocations against `.rsrc` symbol (sym 0)
    debug_assert_eq!(rsrc.len(), data_ent_base);
    let mut relocs: Vec<u32> = Vec::with_capacity(n + 1);
    for (i, f) in frames.iter().enumerate() {
        relocs.push(rsrc.len() as u32);
        rsrc.extend_from_slice(&payload_offsets[i].to_le_bytes()); // OffsetToData (relocated)
        rsrc.extend_from_slice(&(f.data.len() as u32).to_le_bytes()); // Size
        rsrc.extend_from_slice(&0u32.to_le_bytes()); // CodePage
        rsrc.extend_from_slice(&0u32.to_le_bytes()); // Reserved
    }
    relocs.push(rsrc.len() as u32);
    rsrc.extend_from_slice(&payload_offsets[n].to_le_bytes());
    rsrc.extend_from_slice(&(group_data.len() as u32).to_le_bytes());
    rsrc.extend_from_slice(&0u32.to_le_bytes());
    rsrc.extend_from_slice(&0u32.to_le_bytes());

    // 6. Payloads
    debug_assert_eq!(rsrc.len(), payload_base);
    for f in &frames {
        rsrc.extend_from_slice(f.data);
        while rsrc.len() % 4 != 0 {
            rsrc.push(0);
        }
    }
    rsrc.extend_from_slice(&group_data);
    while rsrc.len() % 4 != 0 {
        rsrc.push(0);
    }

    // Assemble x86_64 COFF object:
    //   COFF header (20 bytes)
    //   1 section header `.rsrc$01` (40 bytes)
    //   Section raw data (`rsrc`)
    //   Relocation table (`relocs.len()` * 10 bytes)
    //   Symbol table (1 symbol `.rsrc$01` = 18 bytes)
    //   String table (4 bytes: size = 4)
    let raw_data_ptr = 20u32 + 40u32;
    let reloc_ptr = raw_data_ptr + rsrc.len() as u32;
    let symtab_ptr = reloc_ptr + (relocs.len() as u32) * 10;

    let mut obj = Vec::with_capacity(symtab_ptr as usize + 22);

    // IMAGE_FILE_HEADER (20 bytes)
    obj.extend_from_slice(&0x8664u16.to_le_bytes()); // Machine = IMAGE_FILE_MACHINE_AMD64
    obj.extend_from_slice(&1u16.to_le_bytes()); // NumberOfSections = 1
    obj.extend_from_slice(&0u32.to_le_bytes()); // TimeDateStamp
    obj.extend_from_slice(&symtab_ptr.to_le_bytes()); // PointerToSymbolTable
    obj.extend_from_slice(&1u32.to_le_bytes()); // NumberOfSymbols = 1
    obj.extend_from_slice(&0u16.to_le_bytes()); // SizeOfOptionalHeader
    obj.extend_from_slice(&0x0000u16.to_le_bytes()); // Characteristics

    // IMAGE_SECTION_HEADER (40 bytes)
    obj.extend_from_slice(b".rsrc\0\0\0"); // Name (8 bytes)
    obj.extend_from_slice(&0u32.to_le_bytes()); // VirtualSize
    obj.extend_from_slice(&0u32.to_le_bytes()); // VirtualAddress
    obj.extend_from_slice(&(rsrc.len() as u32).to_le_bytes()); // SizeOfRawData
    obj.extend_from_slice(&raw_data_ptr.to_le_bytes()); // PointerToRawData
    obj.extend_from_slice(&reloc_ptr.to_le_bytes()); // PointerToRelocations
    obj.extend_from_slice(&0u32.to_le_bytes()); // PointerToLinenumbers
    obj.extend_from_slice(&(relocs.len() as u16).to_le_bytes()); // NumberOfRelocations
    obj.extend_from_slice(&0u16.to_le_bytes()); // NumberOfLinenumbers
    // IMAGE_SCN_CNT_INITIALIZED_DATA (0x40) | IMAGE_SCN_MEM_READ (0x40000000) | IMAGE_SCN_ALIGN_4BYTES (0x00300000)
    obj.extend_from_slice(&0x4030_0040u32.to_le_bytes());

    // Section raw data
    debug_assert_eq!(obj.len() as u32, raw_data_ptr);
    obj.extend_from_slice(&rsrc);

    // Relocations (10 bytes each): IMAGE_REL_AMD64_ADDR32NB = 3
    debug_assert_eq!(obj.len() as u32, reloc_ptr);
    for r_off in relocs {
        obj.extend_from_slice(&r_off.to_le_bytes()); // VirtualAddress (offset within section)
        obj.extend_from_slice(&0u32.to_le_bytes()); // SymbolTableIndex = 0 (`.rsrc`)
        obj.extend_from_slice(&3u16.to_le_bytes()); // Type = IMAGE_REL_AMD64_ADDR32NB
    }

    // Symbol table: 1 section symbol for `.rsrc` (18 bytes)
    debug_assert_eq!(obj.len() as u32, symtab_ptr);
    obj.extend_from_slice(b".rsrc\0\0\0"); // Name
    obj.extend_from_slice(&0u32.to_le_bytes()); // Value = 0
    obj.extend_from_slice(&1i16.to_le_bytes()); // SectionNumber = 1 (1-based)
    obj.extend_from_slice(&0u16.to_le_bytes()); // Type
    obj.extend_from_slice(&3u8.to_le_bytes()); // StorageClass = IMAGE_SYM_CLASS_STATIC
    obj.extend_from_slice(&0u8.to_le_bytes()); // NumberOfAuxSymbols

    // Empty string table (4 bytes)
    obj.extend_from_slice(&4u32.to_le_bytes());

    Ok(obj)
}
