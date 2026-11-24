use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use std::io::{Cursor, Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

static TMP_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone)]
pub struct WriteKeywordsOutcome {
    pub method: String,
}

/// Writes the full keyword list internally into `path` so Windows Explorer ("Tags" / `System.Keywords`),
/// CorelDRAW, and XMP/EXIF-aware applications see the exact same keywords stored in the catalog database.
pub fn write_keywords_to_file(
    path: &Path,
    keywords: &[String],
) -> Result<WriteKeywordsOutcome, String> {
    if !path.is_file() {
        return Err("File does not exist on disk".to_string());
    }

    let cleaned = normalize_keywords(keywords);
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    match ext.as_str() {
        // CorelDRAW:
        // 1. Try native Windows Property Store first (uses Corel's official ShellVista.dll CDR Property
        //    Handler when installed, guaranteeing 100% Windows Explorer & CorelDRAW compatibility).
        //    For ZIP CDRs, also sync <dc:subject> if stale tags remain from non-Corel writers.
        // 2. Fall back to pure-Rust ZIP CDR (writes Corel's native <crl:Keywords> + <dc:subject> +
        //    docProps/core.xml) or RIFF CDR (UTF-16LE IKEY for v12/X3+, ANSI IKEY for v3-v11).
        "cdr" | "cmx" | "ccx" | "pat" => {
            if let Ok(method) = try_windows_property_store_write(path, &cleaned) {
                // Keep <dc:subject> aligned on ZIP CDRs so removing tags doesn't leave stale XMP tags behind.
                let _ = sync_zip_cdr_dc_subject_only_if_present(path, &cleaned);
                notify_shell_item_updated(path);
                return Ok(WriteKeywordsOutcome { method });
            }
            let method = write_cdr_keywords(path, &cleaned)?;
            notify_shell_item_updated(path);
            Ok(WriteKeywordsOutcome { method })
        }

        // JPEG: try Windows Property System (writes XMP + EXIF XPKeywords + IPTC natively),
        // then fall back to lossless pure-Rust APP1 XMP injection.
        "jpg" | "jpeg" | "jpe" => {
            if let Ok(method) = try_windows_property_store_write(path, &cleaned) {
                notify_shell_item_updated(path);
                return Ok(WriteKeywordsOutcome { method });
            }
            write_jpeg_xmp(path, &cleaned)?;
            notify_shell_item_updated(path);
            Ok(WriteKeywordsOutcome {
                method: "JPEG APP1 XMP".to_string(),
            })
        }

        // PNG: try Windows Property System first, then fall back to lossless iTXt XMP chunk.
        "png" => {
            if let Ok(method) = try_windows_property_store_write(path, &cleaned) {
                notify_shell_item_updated(path);
                return Ok(WriteKeywordsOutcome { method });
            }
            write_png_xmp(path, &cleaned)?;
            notify_shell_item_updated(path);
            Ok(WriteKeywordsOutcome {
                method: "PNG iTXt XMP".to_string(),
            })
        }

        // WEBP: try Windows Property System first, then fall back to lossless RIFF VP8X + XMP chunk.
        "webp" => {
            if let Ok(method) = try_windows_property_store_write(path, &cleaned) {
                notify_shell_item_updated(path);
                return Ok(WriteKeywordsOutcome { method });
            }
            write_webp_xmp(path, &cleaned)?;
            notify_shell_item_updated(path);
            Ok(WriteKeywordsOutcome {
                method: "WEBP RIFF XMP".to_string(),
            })
        }

        // SVG / SVGZ: embed/update RDF/Dublin Core <metadata> block inside <svg>.
        "svg" | "svgz" => {
            write_svg_keywords(path, &cleaned, ext == "svgz")?;
            notify_shell_item_updated(path);
            Ok(WriteKeywordsOutcome {
                method: if ext == "svgz" {
                    "SVGZ RDF Metadata".to_string()
                } else {
                    "SVG RDF Metadata".to_string()
                },
            })
        }

        // EPS / PS / PRN: update %%Keywords: DSC header comment.
        "eps" | "epsi" | "epsf" | "ps" | "prn" => {
            write_ps_dsc_keywords(path, &cleaned)?;
            notify_shell_item_updated(path);
            Ok(WriteKeywordsOutcome {
                method: "PostScript DSC %%Keywords".to_string(),
            })
        }

        // PDF / AI: detect PDF vs PostScript AI; try Windows Property System, then safe in-place XMP or DSC.
        "pdf" | "ai" => {
            let head = read_head(path, 16).unwrap_or_default();
            if head.starts_with(b"%!PS") {
                write_ps_dsc_keywords(path, &cleaned)?;
                notify_shell_item_updated(path);
                return Ok(WriteKeywordsOutcome {
                    method: "PostScript AI %%Keywords".to_string(),
                });
            }
            if let Ok(method) = try_windows_property_store_write(path, &cleaned) {
                notify_shell_item_updated(path);
                return Ok(WriteKeywordsOutcome { method });
            }
            if try_inplace_xmp_update(path, &cleaned)? {
                notify_shell_item_updated(path);
                return Ok(WriteKeywordsOutcome {
                    method: "PDF In-Place XMP".to_string(),
                });
            }
            Err(
                "No writable Windows Property Handler or padded XMP packet found in PDF"
                    .to_string(),
            )
        }

        // TIFF and any other format (AVIF, JXL, GIF, BMP, etc.):
        // Try native Windows Shell IPropertyStore first (works for TIFF & any installed WIC/Shell handler),
        // then safe in-place XMP packet replacement if the file already has a padded xpacket.
        _ => {
            if let Ok(method) = try_windows_property_store_write(path, &cleaned) {
                notify_shell_item_updated(path);
                return Ok(WriteKeywordsOutcome { method });
            }
            if try_inplace_xmp_update(path, &cleaned)? {
                notify_shell_item_updated(path);
                return Ok(WriteKeywordsOutcome {
                    method: "In-Place XMP".to_string(),
                });
            }
            Err(format!(
                "No writable Windows Property Handler or native keyword writer for .{ext}"
            ))
        }
    }
}

/// Reads Windows Explorer `System.Keywords` (`PKEY_Keywords`) via `IPropertyStore` on Windows.
#[allow(dead_code)]
pub fn read_windows_shell_keywords(path: &Path) -> Vec<String> {
    #[cfg(windows)]
    {
        win_shell::read_property_store_keywords(path).unwrap_or_default()
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Vec::new()
    }
}

fn normalize_keywords(keywords: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for kw in keywords {
        let trimmed = kw.trim();
        if !trimmed.is_empty() && !out.iter().any(|k: &String| k.eq_ignore_ascii_case(trimmed)) {
            out.push(trimmed.to_string());
        }
    }
    out
}

fn read_head(path: &Path, max_len: usize) -> std::io::Result<Vec<u8>> {
    let mut f = std::fs::File::open(path)?;
    let mut buf = vec![0u8; max_len];
    let n = f.read(&mut buf)?;
    buf.truncate(n);
    Ok(buf)
}

fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c if (c as u32) < 0x20 && c != '\t' && c != '\n' && c != '\r' => {}
            c => out.push(c),
        }
    }
    out
}

/// Builds a standards-compliant Adobe XMP / Windows Photo metadata packet containing
/// `dc:subject` and `MicrosoftPhoto:LastKeywordXMP` (the two fields Windows Explorer reads).
fn build_xmp_packet(keywords: &[String], existing_xmp: Option<&str>) -> String {
    if let Some(existing) = existing_xmp {
        if let Some(updated) = update_existing_xmp_xml(existing, keywords) {
            return updated;
        }
    }

    let mut subject_block = String::new();
    if !keywords.is_empty() {
        subject_block.push_str("   <dc:subject>\n    <rdf:Bag>\n");
        for kw in keywords {
            subject_block.push_str("     <rdf:li>");
            subject_block.push_str(&xml_escape(kw));
            subject_block.push_str("</rdf:li>\n");
        }
        subject_block.push_str("    </rdf:Bag>\n   </dc:subject>\n");

        subject_block.push_str("   <MicrosoftPhoto:LastKeywordXMP>\n    <rdf:Bag>\n");
        for kw in keywords {
            subject_block.push_str("     <rdf:li>");
            subject_block.push_str(&xml_escape(kw));
            subject_block.push_str("</rdf:li>\n");
        }
        subject_block.push_str("    </rdf:Bag>\n   </MicrosoftPhoto:LastKeywordXMP>\n");
    }

    format!(
        "<?xpacket begin=\"\u{FEFF}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n\
         <x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"ThumbCraft Studio\">\n \
          <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n  \
           <rdf:Description rdf:about=\"\"\n    \
             xmlns:dc=\"http://purl.org/dc/elements/1.1/\"\n    \
             xmlns:MicrosoftPhoto=\"http://ns.microsoft.com/photo/1.0/\">\n\
         {subject_block}  \
           </rdf:Description>\n \
          </rdf:RDF>\n\
         </x:xmpmeta>\n\
         <?xpacket end=\"w\"?>"
    )
}

/// Builds CorelDRAW's `META-INF/metadata.xml` XMP/RDF document containing Corel's native
/// `<crl:Keywords>` (read by CorelDRAW and Corel's Windows Explorer `ShellVista.dll` Property Handler)
/// alongside `<dc:subject>` and `<MicrosoftPhoto:LastKeywordXMP>`.
fn build_cdr_metadata_xml(keywords: &[String], existing_xml: Option<&str>) -> String {
    if let Some(existing) = existing_xml {
        if let Some(updated) = update_existing_cdr_metadata_xml(existing, keywords) {
            return updated;
        }
    }

    let mut body = String::new();
    if !keywords.is_empty() {
        let joined = xml_escape(&keywords.join(";"));
        body.push_str("      <crl:Keywords>\n       <rdf:Alt>\n");
        body.push_str(&format!(
            "        <rdf:li xml:lang=\"x-default\">{joined}</rdf:li>\n"
        ));
        body.push_str(&format!(
            "        <rdf:li xml:lang=\"en\">{joined}</rdf:li>\n"
        ));
        body.push_str("       </rdf:Alt>\n      </crl:Keywords>\n");

        body.push_str("      <dc:subject>\n       <rdf:Bag>\n");
        for kw in keywords {
            body.push_str("        <rdf:li>");
            body.push_str(&xml_escape(kw));
            body.push_str("</rdf:li>\n");
        }
        body.push_str("       </rdf:Bag>\n      </dc:subject>\n");

        body.push_str("      <MicrosoftPhoto:LastKeywordXMP>\n       <rdf:Bag>\n");
        for kw in keywords {
            body.push_str("        <rdf:li>");
            body.push_str(&xml_escape(kw));
            body.push_str("</rdf:li>\n");
        }
        body.push_str("       </rdf:Bag>\n      </MicrosoftPhoto:LastKeywordXMP>\n");
    }

    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" ?>\n\
         <x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n \
          <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n  \
           <rdf:Description rdf:about=\"\"\n    \
             xmlns:crl=\"http://namespace.corel.com/zcf/metadata/1.0/\"\n    \
             xmlns:dc=\"http://purl.org/dc/elements/1.1/\"\n    \
             xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\"\n    \
             xmlns:MicrosoftPhoto=\"http://ns.microsoft.com/photo/1.0/\">\n\
         {body}  \
           </rdf:Description>\n \
          </rdf:RDF>\n\
         </x:xmpmeta>\n"
    )
}

fn update_existing_cdr_metadata_xml(xml: &str, keywords: &[String]) -> Option<String> {
    let mut s = expand_self_closing_rdf_description(xml);
    if !s.contains("</rdf:Description>") {
        return None;
    }

    for tag in [
        "crl:Keywords",
        "dc:subject",
        "MicrosoftPhoto:LastKeywordXMP",
        "pdf:Keywords",
    ] {
        s = strip_xml_element(&s, tag);
    }

    if let Some(desc_start) = s.find("<rdf:Description") {
        if let Some(rel_end) = s[desc_start..].find('>') {
            let insert_attr_pos = desc_start + rel_end;
            let header = &s[desc_start..insert_attr_pos];
            let mut extra_ns = String::new();
            // Always ensure namespaces are in scope directly on <rdf:Description> (ShellVista.dll
            // places xmlns:dc on individual child tags like <dc:title xmlns:dc="...">, not on Description).
            if !header.contains("xmlns:crl=") {
                extra_ns.push_str(" xmlns:crl=\"http://namespace.corel.com/zcf/metadata/1.0/\"");
            }
            if !header.contains("xmlns:dc=") {
                extra_ns.push_str(" xmlns:dc=\"http://purl.org/dc/elements/1.1/\"");
            }
            if !header.contains("xmlns:MicrosoftPhoto=") {
                extra_ns.push_str(" xmlns:MicrosoftPhoto=\"http://ns.microsoft.com/photo/1.0/\"");
            }
            if !extra_ns.is_empty() {
                s.insert_str(insert_attr_pos, &extra_ns);
            }
        }
    }

    let insert_pos = s.rfind("</rdf:Description>")?;
    if !keywords.is_empty() {
        let joined = xml_escape(&keywords.join(";"));
        let mut block = String::new();
        block.push_str("\n   <crl:Keywords>\n    <rdf:Alt>\n");
        block.push_str(&format!(
            "     <rdf:li xml:lang=\"x-default\">{joined}</rdf:li>\n"
        ));
        block.push_str(&format!(
            "     <rdf:li xml:lang=\"en\">{joined}</rdf:li>\n"
        ));
        block.push_str("    </rdf:Alt>\n   </crl:Keywords>\n");
        block.push_str("   <dc:subject>\n    <rdf:Bag>\n");
        for kw in keywords {
            block.push_str("     <rdf:li>");
            block.push_str(&xml_escape(kw));
            block.push_str("</rdf:li>\n");
        }
        block.push_str("    </rdf:Bag>\n   </dc:subject>\n");
        block.push_str("   <MicrosoftPhoto:LastKeywordXMP>\n    <rdf:Bag>\n");
        for kw in keywords {
            block.push_str("     <rdf:li>");
            block.push_str(&xml_escape(kw));
            block.push_str("</rdf:li>\n");
        }
        block.push_str("    </rdf:Bag>\n   </MicrosoftPhoto:LastKeywordXMP>\n  ");
        s.insert_str(insert_pos, &block);
    }

    Some(s)
}

fn expand_self_closing_rdf_description(xml: &str) -> String {
    if xml.contains("</rdf:Description>") {
        return xml.to_string();
    }
    if let Some(start) = xml.find("<rdf:Description") {
        if let Some(rel_end) = xml[start..].find("/>") {
            let mut out = xml.to_string();
            let slash_pos = start + rel_end;
            out.replace_range(slash_pos..slash_pos + 2, "></rdf:Description>");
            return out;
        }
    }
    xml.to_string()
}

/// Updates `dc:subject`, `MicrosoftPhoto:LastKeywordXMP`, and `pdf:Keywords` inside an existing
/// XMP/RDF XML document while preserving all other metadata elements (title, creator, etc.).
fn update_existing_xmp_xml(xml: &str, keywords: &[String]) -> Option<String> {
    let mut s = expand_self_closing_rdf_description(xml);
    if !s.contains("</rdf:Description>") {
        return None;
    }

    // Remove existing keyword elements before reinserting fresh ones.
    for tag in [
        "dc:subject",
        "MicrosoftPhoto:LastKeywordXMP",
        "pdf:Keywords",
    ] {
        s = strip_xml_element(&s, tag);
    }

    // Ensure xmlns:dc and xmlns:MicrosoftPhoto are declared directly on rdf:Description if missing.
    if let Some(desc_start) = s.find("<rdf:Description") {
        if let Some(rel_end) = s[desc_start..].find('>') {
            let insert_attr_pos = desc_start + rel_end;
            let header = &s[desc_start..insert_attr_pos];
            let mut extra_ns = String::new();
            if !header.contains("xmlns:dc=") {
                extra_ns.push_str(" xmlns:dc=\"http://purl.org/dc/elements/1.1/\"");
            }
            if !header.contains("xmlns:MicrosoftPhoto=") {
                extra_ns.push_str(" xmlns:MicrosoftPhoto=\"http://ns.microsoft.com/photo/1.0/\"");
            }
            if !extra_ns.is_empty() {
                s.insert_str(insert_attr_pos, &extra_ns);
            }
        }
    }

    let insert_pos = s.rfind("</rdf:Description>")?;
    if !keywords.is_empty() {
        let mut block = String::new();
        block.push_str("\n   <dc:subject>\n    <rdf:Bag>\n");
        for kw in keywords {
            block.push_str("     <rdf:li>");
            block.push_str(&xml_escape(kw));
            block.push_str("</rdf:li>\n");
        }
        block.push_str("    </rdf:Bag>\n   </dc:subject>\n");
        block.push_str("   <MicrosoftPhoto:LastKeywordXMP>\n    <rdf:Bag>\n");
        for kw in keywords {
            block.push_str("     <rdf:li>");
            block.push_str(&xml_escape(kw));
            block.push_str("</rdf:li>\n");
        }
        block.push_str("    </rdf:Bag>\n   </MicrosoftPhoto:LastKeywordXMP>\n  ");
        s.insert_str(insert_pos, &block);
    }

    Some(s)
}

fn strip_xml_element(xml: &str, tag: &str) -> String {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut cur = xml.to_string();
    while let Some(start) = cur.find(&open) {
        let after_tag = &cur[start + open.len()..];
        // Verify tag boundary (whitespace, '>', or '/')
        let boundary_ok = after_tag
            .chars()
            .next()
            .map(|c| c.is_whitespace() || c == '>' || c == '/')
            .unwrap_or(false);
        if !boundary_ok {
            break;
        }
        // Check for self-closing tag first
        if let Some(gt) = after_tag.find('>') {
            let tag_head = &after_tag[..gt];
            if tag_head.trim_end().ends_with('/') {
                let end = start + open.len() + gt + 1;
                cur.replace_range(start..end, "");
                continue;
            }
        }
        if let Some(rel_close) = cur[start..].find(&close) {
            let end = start + rel_close + close.len();
            cur.replace_range(start..end, "");
        } else {
            break;
        }
    }
    cur
}

/// Atomically replaces `path` with `bytes` by writing to a sibling temp file first.
fn write_file_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let fname = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file");
    let seq = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp_path = parent.join(format!(
        ".{fname}.thumbcraft_tmp_{}_{}",
        std::process::id(),
        seq
    ));

    {
        let mut f = std::fs::File::create(&tmp_path)
            .map_err(|e| format!("Failed creating temp file: {e}"))?;
        f.write_all(bytes)
            .map_err(|e| format!("Failed writing temp file: {e}"))?;
        f.sync_all()
            .map_err(|e| format!("Failed flushing temp file: {e}"))?;
    }

    if let Err(e) = replace_file(&tmp_path, path) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(e);
    }
    Ok(())
}

fn replace_file(src: &Path, dst: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        win_shell::move_file_replace_existing(src, dst)
    }
    #[cfg(not(windows))]
    {
        std::fs::rename(src, dst).map_err(|e| format!("Failed replacing file: {e}"))
    }
}

// ---------------------------------------------------------------------------
// 1. CorelDRAW Writer (ZIP X4+ & Classic RIFF v3-v13)
// ---------------------------------------------------------------------------

/// After Corel's `ShellVista.dll` writes `<crl:Keywords>`, check if the ZIP CDR also has
/// `<dc:subject>` or `<MicrosoftPhoto:LastKeywordXMP>` tags that differ from `keywords`; if so,
/// keep them synchronized so removing a tag removes it from both Corel and XMP readers.
fn sync_zip_cdr_dc_subject_only_if_present(path: &Path, keywords: &[String]) -> Result<(), String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    if !bytes.starts_with(b"PK\x03\x04") {
        return Ok(());
    }
    let has_xmp_subject = {
        let mut archive = zip::ZipArchive::new(Cursor::new(&bytes)).map_err(|e| e.to_string())?;
        let mut found = false;
        for i in 0..archive.len() {
            if let Ok(mut f) = archive.by_index(i) {
                let lower = f.name().to_ascii_lowercase();
                if lower == "meta-inf/metadata.xml"
                    || lower == "metadata/metadata.xml"
                    || lower == "content/metadata.xml"
                {
                    let mut raw = String::new();
                    if f.read_to_string(&mut raw).is_ok()
                        && (raw.contains("<dc:subject") || raw.contains("<MicrosoftPhoto:LastKeywordXMP"))
                    {
                        found = true;
                        break;
                    }
                }
            }
        }
        found
    };
    if !has_xmp_subject {
        return Ok(());
    }
    let updated = rewrite_zip_cdr_bytes(&bytes, keywords)?;
    write_file_atomic(path, &updated)
}

fn write_cdr_keywords(path: &Path, keywords: &[String]) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("Failed to read CDR: {e}"))?;
    if bytes.starts_with(b"PK\x03\x04") {
        let updated = rewrite_zip_cdr_bytes(&bytes, keywords)?;
        write_file_atomic(path, &updated)?;
        Ok("CDR ZIP metadata.xml (crl:Keywords + XMP)".to_string())
    } else if bytes.starts_with(b"RIFF") {
        let updated = rewrite_riff_cdr_bytes(&bytes, keywords)?;
        write_file_atomic(path, &updated)?;
        Ok("CDR RIFF LIST/INFO/IKEY".to_string())
    } else {
        Err("Legacy CorelDRAW container (WL/@CorelBMF) does not support embedded keywords".into())
    }
}

fn rewrite_zip_cdr_bytes(bytes: &[u8], keywords: &[String]) -> Result<Vec<u8>, String> {
    let cursor = Cursor::new(bytes);
    let mut archive =
        zip::ZipArchive::new(cursor).map_err(|e| format!("Invalid ZIP CDR archive: {e}"))?;

    let mut out_buf = Cursor::new(Vec::with_capacity(bytes.len() + 1024));
    let mut writer = zip::ZipWriter::new(&mut out_buf);

    let mut wrote_metadata_xml = false;

    for i in 0..archive.len() {
        let entry_name = {
            let f = archive
                .by_index(i)
                .map_err(|e| format!("ZIP entry read error: {e}"))?;
            f.name().to_string()
        };
        let lower = entry_name.to_ascii_lowercase();

        if lower == "meta-inf/metadata.xml"
            || lower == "metadata/metadata.xml"
            || lower == "content/metadata.xml"
        {
            let mut f = archive
                .by_index(i)
                .map_err(|e| format!("ZIP entry read error: {e}"))?;
            let method = f.compression();
            let mut raw = String::new();
            let _ = f.read_to_string(&mut raw);
            let updated_xml = build_cdr_metadata_xml(keywords, Some(&raw));
            let opts = zip::write::SimpleFileOptions::default().compression_method(method);
            writer
                .start_file(&entry_name, opts)
                .map_err(|e| format!("ZIP write error: {e}"))?;
            writer
                .write_all(updated_xml.as_bytes())
                .map_err(|e| format!("ZIP write error: {e}"))?;
            wrote_metadata_xml = true;
        } else if lower == "docprops/core.xml" {
            let mut f = archive
                .by_index(i)
                .map_err(|e| format!("ZIP entry read error: {e}"))?;
            let method = f.compression();
            let mut raw = String::new();
            let _ = f.read_to_string(&mut raw);
            let updated_core = update_ooxml_core_keywords(&raw, keywords);
            let opts = zip::write::SimpleFileOptions::default().compression_method(method);
            writer
                .start_file(&entry_name, opts)
                .map_err(|e| format!("ZIP write error: {e}"))?;
            writer
                .write_all(updated_core.as_bytes())
                .map_err(|e| format!("ZIP write error: {e}"))?;
        } else {
            let raw_file = archive
                .by_index_raw(i)
                .map_err(|e| format!("ZIP raw copy error: {e}"))?;
            writer
                .raw_copy_file(raw_file)
                .map_err(|e| format!("ZIP raw copy write error: {e}"))?;
        }
    }

    if !wrote_metadata_xml && !keywords.is_empty() {
        let xml = build_cdr_metadata_xml(keywords, None);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        writer
            .start_file("META-INF/metadata.xml", opts)
            .map_err(|e| format!("ZIP add metadata.xml error: {e}"))?;
        writer
            .write_all(xml.as_bytes())
            .map_err(|e| format!("ZIP write metadata.xml error: {e}"))?;
    }

    writer
        .finish()
        .map_err(|e| format!("ZIP finish error: {e}"))?;
    Ok(out_buf.into_inner())
}

fn update_ooxml_core_keywords(xml: &str, keywords: &[String]) -> String {
    let mut s = strip_xml_element(xml, "cp:keywords");
    if !keywords.is_empty() {
        let joined = keywords
            .iter()
            .map(|k| xml_escape(k))
            .collect::<Vec<_>>()
            .join("; ");
        if let Some(pos) = s.rfind("</cp:coreProperties>") {
            s.insert_str(pos, &format!("<cp:keywords>{joined}</cp:keywords>"));
        }
    }
    s
}

fn riff_uses_utf16_info(form_type: &[u8]) -> bool {
    // CorelDRAW 12 (CDRC) and X3+ (CDRD..CDRP) store LIST/INFO strings (including IKEY) as UTF-16LE,
    // which Corel's ShellVista.dll expects when reading System.Keywords for Windows Explorer.
    form_type.len() == 4
        && &form_type[0..3] == b"CDR"
        && form_type[3] >= b'C'
}

fn rewrite_riff_cdr_bytes(bytes: &[u8], keywords: &[String]) -> Result<Vec<u8>, String> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" {
        return Err("Not a valid RIFF container".to_string());
    }

    let form_type = &bytes[8..12];
    let use_utf16 = riff_uses_utf16_info(form_type);
    let mut body = Vec::with_capacity(bytes.len() + 256);
    let mut pos = 12usize;
    let mut found_info = false;

    while pos + 8 <= bytes.len() {
        let fourcc = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes([
            bytes[pos + 4],
            bytes[pos + 5],
            bytes[pos + 6],
            bytes[pos + 7],
        ]) as usize;
        let payload_start = pos + 8;
        let payload_end = payload_start.saturating_add(size).min(bytes.len());
        let payload = &bytes[payload_start..payload_end];

        if fourcc == b"LIST" && payload.len() >= 4 && &payload[0..4] == b"INFO" {
            found_info = true;
            let rebuilt = rebuild_riff_info_list(&payload[4..], keywords, use_utf16);
            if let Some(info_chunk) = rebuilt {
                body.extend_from_slice(&info_chunk);
            }
        } else {
            body.extend_from_slice(fourcc);
            body.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            body.extend_from_slice(payload);
            if payload.len() % 2 == 1 {
                body.push(0);
            }
        }

        let padded = size.saturating_add(size & 1);
        let next = payload_start.saturating_add(padded);
        if next <= pos {
            break;
        }
        pos = next;
    }

    if !found_info && !keywords.is_empty() {
        if let Some(info_chunk) = rebuild_riff_info_list(&[], keywords, use_utf16) {
            body.extend_from_slice(&info_chunk);
        }
    }

    let mut out = Vec::with_capacity(12 + body.len());
    out.extend_from_slice(b"RIFF");
    let total_riff_size = (4 + body.len()) as u32;
    out.extend_from_slice(&total_riff_size.to_le_bytes());
    out.extend_from_slice(form_type);
    out.extend_from_slice(&body);
    Ok(out)
}

fn rebuild_riff_info_list(
    existing_subchunks: &[u8],
    keywords: &[String],
    use_utf16: bool,
) -> Option<Vec<u8>> {
    let mut sub = Vec::new();
    let mut pos = 0usize;
    while pos + 8 <= existing_subchunks.len() {
        let fourcc = &existing_subchunks[pos..pos + 4];
        let size = u32::from_le_bytes([
            existing_subchunks[pos + 4],
            existing_subchunks[pos + 5],
            existing_subchunks[pos + 6],
            existing_subchunks[pos + 7],
        ]) as usize;
        let payload_start = pos + 8;
        let payload_end = payload_start
            .saturating_add(size)
            .min(existing_subchunks.len());
        let payload = &existing_subchunks[payload_start..payload_end];

        if fourcc != b"IKEY" {
            sub.extend_from_slice(fourcc);
            sub.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            sub.extend_from_slice(payload);
            if payload.len() % 2 == 1 {
                sub.push(0);
            }
        }

        let padded = size.saturating_add(size & 1);
        let next = payload_start.saturating_add(padded);
        if next <= pos {
            break;
        }
        pos = next;
    }

    if !keywords.is_empty() {
        let joined = keywords.join(";");
        let ikey_bytes: Vec<u8> = if use_utf16 {
            let mut b = Vec::with_capacity((joined.len() + 1) * 2);
            for u in joined.encode_utf16().chain(std::iter::once(0u16)) {
                b.extend_from_slice(&u.to_le_bytes());
            }
            b
        } else {
            let mut b = joined.into_bytes();
            b.push(0);
            b
        };
        sub.extend_from_slice(b"IKEY");
        sub.extend_from_slice(&(ikey_bytes.len() as u32).to_le_bytes());
        sub.extend_from_slice(&ikey_bytes);
        if ikey_bytes.len() % 2 == 1 {
            sub.push(0);
        }
    }

    if sub.is_empty() {
        return None;
    }

    let mut list = Vec::with_capacity(12 + sub.len());
    list.extend_from_slice(b"LIST");
    list.extend_from_slice(&((4 + sub.len()) as u32).to_le_bytes());
    list.extend_from_slice(b"INFO");
    list.extend_from_slice(&sub);
    if list.len() % 2 == 1 {
        list.push(0);
    }
    Some(list)
}

// ---------------------------------------------------------------------------
// 2. Lossless JPEG APP1 XMP Writer (fallback if Windows IPropertyStore unavailable)
// ---------------------------------------------------------------------------

const JPEG_XMP_HEADER: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";

fn write_jpeg_xmp(path: &Path, keywords: &[String]) -> Result<(), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("Failed reading JPEG: {e}"))?;
    let updated = rewrite_jpeg_xmp_bytes(&bytes, keywords)?;
    write_file_atomic(path, &updated)
}

fn rewrite_jpeg_xmp_bytes(bytes: &[u8], keywords: &[String]) -> Result<Vec<u8>, String> {
    if bytes.len() < 4 || bytes[0] != 0xFF || bytes[1] != 0xD8 {
        return Err("Not a valid JPEG file".to_string());
    }

    let mut existing_xmp: Option<String> = None;
    let mut segments: Vec<&[u8]> = Vec::new();
    let mut pos = 2usize;

    while pos + 2 <= bytes.len() {
        if bytes[pos] != 0xFF {
            // Unexpected byte; append remainder verbatim
            segments.push(&bytes[pos..]);
            break;
        }
        // Skip fill 0xFF bytes
        let seg_start = pos;
        while pos < bytes.len() && bytes[pos] == 0xFF {
            pos += 1;
        }
        if pos >= bytes.len() {
            segments.push(&bytes[seg_start..]);
            break;
        }
        let marker = bytes[pos];
        pos += 1;

        // Start of Scan (0xDA) or End of Image (0xD9): rest of file is entropy-coded data
        if marker == 0xDA || marker == 0xD9 {
            segments.push(&bytes[seg_start..]);
            break;
        }
        // Standalone markers without length
        if marker == 0x01 || (0xD0..=0xD7).contains(&marker) {
            segments.push(&bytes[seg_start..pos]);
            continue;
        }
        if pos + 2 > bytes.len() {
            segments.push(&bytes[seg_start..]);
            break;
        }
        let seg_len = u16::from_be_bytes([bytes[pos], bytes[pos + 1]]) as usize;
        if seg_len < 2 || pos + seg_len > bytes.len() {
            segments.push(&bytes[seg_start..]);
            break;
        }
        let payload = &bytes[pos + 2..pos + seg_len];
        let seg_end = pos + seg_len;

        if marker == 0xE1 && payload.starts_with(JPEG_XMP_HEADER) {
            let xmp_bytes = &payload[JPEG_XMP_HEADER.len()..];
            if existing_xmp.is_none() {
                existing_xmp = Some(String::from_utf8_lossy(xmp_bytes).to_string());
            }
            // Drop old XMP APP1 segment so we can re-insert updated one cleanly
        } else {
            segments.push(&bytes[seg_start..seg_end]);
        }
        pos = seg_end;
    }

    let mut out = Vec::with_capacity(bytes.len() + 1024);
    out.extend_from_slice(&[0xFF, 0xD8]);

    let mut inserted_xmp = false;
    let xmp_segment = if !keywords.is_empty() || existing_xmp.is_some() {
        let xml = build_xmp_packet(keywords, existing_xmp.as_deref());
        let payload_len = JPEG_XMP_HEADER.len() + xml.len();
        if payload_len + 2 > 65535 {
            return Err("XMP packet exceeds JPEG APP1 64 KB segment limit".to_string());
        }
        let mut seg = Vec::with_capacity(4 + payload_len);
        seg.extend_from_slice(&[0xFF, 0xE1]);
        seg.extend_from_slice(&((payload_len + 2) as u16).to_be_bytes());
        seg.extend_from_slice(JPEG_XMP_HEADER);
        seg.extend_from_slice(xml.as_bytes());
        Some(seg)
    } else {
        None
    };

    for seg in segments {
        // Keep APP0 (JFIF, 0xE0) before APP1 XMP if present
        let is_app0 = seg.len() >= 2 && seg[0] == 0xFF && seg[1] == 0xE0;
        if !inserted_xmp && !is_app0 {
            if let Some(ref xmp_seg) = xmp_segment {
                out.extend_from_slice(xmp_seg);
            }
            inserted_xmp = true;
        }
        out.extend_from_slice(seg);
    }
    if !inserted_xmp {
        if let Some(ref xmp_seg) = xmp_segment {
            out.extend_from_slice(xmp_seg);
        }
    }

    Ok(out)
}

// ---------------------------------------------------------------------------
// 3. Lossless PNG iTXt XMP Writer
// ---------------------------------------------------------------------------

const PNG_SIG: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
const PNG_XMP_ITXT_PREFIX: &[u8] = b"XML:com.adobe.xmp\0\0\0\0\0";

fn write_png_xmp(path: &Path, keywords: &[String]) -> Result<(), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("Failed reading PNG: {e}"))?;
    let updated = rewrite_png_xmp_bytes(&bytes, keywords)?;
    write_file_atomic(path, &updated)
}

fn rewrite_png_xmp_bytes(bytes: &[u8], keywords: &[String]) -> Result<Vec<u8>, String> {
    if bytes.len() < 8 || &bytes[0..8] != PNG_SIG {
        return Err("Not a valid PNG file".to_string());
    }

    let mut existing_xmp: Option<String> = None;
    let mut chunks: Vec<([u8; 4], &[u8])> = Vec::new();
    let mut pos = 8usize;

    while pos + 12 <= bytes.len() {
        let len = u32::from_be_bytes([bytes[pos], bytes[pos + 1], bytes[pos + 2], bytes[pos + 3]])
            as usize;
        let ctype = [bytes[pos + 4], bytes[pos + 5], bytes[pos + 6], bytes[pos + 7]];
        let data_start = pos + 8;
        let data_end = data_start.saturating_add(len);
        if data_end + 4 > bytes.len() {
            return Err("Truncated PNG chunk".to_string());
        }
        let data = &bytes[data_start..data_end];
        pos = data_end + 4;

        if &ctype == b"iTXt" && data.starts_with(b"XML:com.adobe.xmp\0") {
            if existing_xmp.is_none() {
                if let Some(xmp_str) = extract_png_itxt_text(data) {
                    existing_xmp = Some(xmp_str);
                }
            }
            continue;
        }
        chunks.push((ctype, data));
    }

    let mut out = Vec::with_capacity(bytes.len() + 1024);
    out.extend_from_slice(PNG_SIG);

    let xmp_chunk_data = if !keywords.is_empty() || existing_xmp.is_some() {
        let xml = build_xmp_packet(keywords, existing_xmp.as_deref());
        let mut payload = Vec::with_capacity(PNG_XMP_ITXT_PREFIX.len() + xml.len());
        payload.extend_from_slice(PNG_XMP_ITXT_PREFIX);
        payload.extend_from_slice(xml.as_bytes());
        Some(payload)
    } else {
        None
    };

    let mut inserted = false;
    for (ctype, data) in chunks {
        if !inserted && (&ctype == b"IDAT" || &ctype == b"IEND") {
            if let Some(ref payload) = xmp_chunk_data {
                write_png_chunk(&mut out, *b"iTXt", payload);
            }
            inserted = true;
        }
        write_png_chunk(&mut out, ctype, data);
    }
    if !inserted {
        if let Some(ref payload) = xmp_chunk_data {
            write_png_chunk(&mut out, *b"iTXt", payload);
        }
    }

    Ok(out)
}

fn extract_png_itxt_text(data: &[u8]) -> Option<String> {
    // Format: keyword\0 comp_flag comp_method lang\0 translated_kw\0 text
    let kw_end = data.iter().position(|&b| b == 0)?;
    if kw_end + 3 >= data.len() {
        return None;
    }
    let comp_flag = data[kw_end + 1];
    let rest = &data[kw_end + 3..];
    let lang_end = rest.iter().position(|&b| b == 0)?;
    let rest2 = &rest[lang_end + 1..];
    let trans_end = rest2.iter().position(|&b| b == 0)?;
    let text_bytes = &rest2[trans_end + 1..];
    if comp_flag == 0 {
        Some(String::from_utf8_lossy(text_bytes).to_string())
    } else {
        let mut dec = flate2::read::ZlibDecoder::new(text_bytes);
        let mut out = String::new();
        dec.read_to_string(&mut out).ok()?;
        Some(out)
    }
}

fn write_png_chunk(out: &mut Vec<u8>, ctype: [u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(&ctype);
    out.extend_from_slice(data);
    let crc = crc32_ieee(&ctype, data);
    out.extend_from_slice(&crc.to_be_bytes());
}

fn crc32_ieee(prefix: &[u8], data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in prefix.iter().chain(data.iter()) {
        crc ^= b as u32;
        for _ in 0..8 {
            if (crc & 1) != 0 {
                crc = (crc >> 1) ^ 0xEDB8_8320;
            } else {
                crc >>= 1;
            }
        }
    }
    !crc
}

// ---------------------------------------------------------------------------
// 4. Lossless WEBP RIFF VP8X + XMP Writer
// ---------------------------------------------------------------------------

fn write_webp_xmp(path: &Path, keywords: &[String]) -> Result<(), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("Failed reading WEBP: {e}"))?;
    let updated = rewrite_webp_xmp_bytes(&bytes, keywords)?;
    write_file_atomic(path, &updated)
}

fn rewrite_webp_xmp_bytes(bytes: &[u8], keywords: &[String]) -> Result<Vec<u8>, String> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WEBP" {
        return Err("Not a valid WEBP file".to_string());
    }

    let mut chunks: Vec<([u8; 4], Vec<u8>)> = Vec::new();
    let mut existing_xmp: Option<String> = None;
    let mut has_vp8x = false;
    let mut pos = 12usize;

    while pos + 8 <= bytes.len() {
        let fourcc = [bytes[pos], bytes[pos + 1], bytes[pos + 2], bytes[pos + 3]];
        let size = u32::from_le_bytes([
            bytes[pos + 4],
            bytes[pos + 5],
            bytes[pos + 6],
            bytes[pos + 7],
        ]) as usize;
        let payload_start = pos + 8;
        let payload_end = payload_start.saturating_add(size).min(bytes.len());
        let payload = &bytes[payload_start..payload_end];

        if &fourcc == b"XMP " {
            existing_xmp = Some(String::from_utf8_lossy(payload).to_string());
        } else {
            if &fourcc == b"VP8X" {
                has_vp8x = true;
            }
            chunks.push((fourcc, payload.to_vec()));
        }

        let padded = size.saturating_add(size & 1);
        let next = payload_start.saturating_add(padded);
        if next <= pos {
            break;
        }
        pos = next;
    }

    let want_xmp = !keywords.is_empty() || existing_xmp.is_some();
    if want_xmp && !has_vp8x {
        // Upgrade simple VP8/VP8L WebP container cleanly to extended VP8X without re-encoding pixels
        let (w, h, has_alpha) = probe_webp_dimensions(bytes)?;
        let mut vp8x = [0u8; 10];
        vp8x[0] = 0x04 | if has_alpha { 0x10 } else { 0x00 }; // XMP bit (0x04) + optional Alpha (0x10)
        let w_m1 = w.saturating_sub(1);
        let h_m1 = h.saturating_sub(1);
        vp8x[4] = (w_m1 & 0xFF) as u8;
        vp8x[5] = ((w_m1 >> 8) & 0xFF) as u8;
        vp8x[6] = ((w_m1 >> 16) & 0xFF) as u8;
        vp8x[7] = (h_m1 & 0xFF) as u8;
        vp8x[8] = ((h_m1 >> 8) & 0xFF) as u8;
        vp8x[9] = ((h_m1 >> 16) & 0xFF) as u8;
        chunks.insert(0, (*b"VP8X", vp8x.to_vec()));
    } else if has_vp8x {
        for (fourcc, payload) in &mut chunks {
            if fourcc == b"VP8X" && !payload.is_empty() {
                if want_xmp {
                    payload[0] |= 0x04;
                } else {
                    payload[0] &= !0x04;
                }
            }
        }
    }

    if want_xmp {
        let xml = build_xmp_packet(keywords, existing_xmp.as_deref());
        chunks.push((*b"XMP ", xml.into_bytes()));
    }

    let mut body = Vec::with_capacity(bytes.len() + 1024);
    for (fourcc, payload) in chunks {
        body.extend_from_slice(&fourcc);
        body.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        body.extend_from_slice(&payload);
        if payload.len() % 2 == 1 {
            body.push(0);
        }
    }

    let mut out = Vec::with_capacity(12 + body.len());
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&((4 + body.len()) as u32).to_le_bytes());
    out.extend_from_slice(b"WEBP");
    out.extend_from_slice(&body);
    Ok(out)
}

fn probe_webp_dimensions(bytes: &[u8]) -> Result<(u32, u32, bool), String> {
    if bytes.len() >= 30 && &bytes[12..16] == b"VP8 " {
        // Lossy VP8 bitstream header: keyframe start code 9D 01 2A at chunk offset +3
        let p = &bytes[20..];
        if p.len() >= 10 && p[3] == 0x9D && p[4] == 0x01 && p[5] == 0x2A {
            let w = (u16::from_le_bytes([p[6], p[7]]) & 0x3FFF) as u32;
            let h = (u16::from_le_bytes([p[8], p[9]]) & 0x3FFF) as u32;
            if w > 0 && h > 0 {
                return Ok((w, h, false));
            }
        }
    } else if bytes.len() >= 25 && &bytes[12..16] == b"VP8L" {
        // Lossless VP8L: signature 0x2F followed by 14-bit width-1, 14-bit height-1, 1-bit alpha
        let p = &bytes[20..];
        if p.len() >= 5 && p[0] == 0x2F {
            let bits = u32::from_le_bytes([p[1], p[2], p[3], p[4]]);
            let w = (bits & 0x3FFF) + 1;
            let h = ((bits >> 14) & 0x3FFF) + 1;
            let alpha = ((bits >> 28) & 1) != 0;
            return Ok((w, h, alpha));
        }
    }
    // Fallback: decode header via oxideav_webp
    let decoded = oxideav_webp::decode_rgba8(bytes)
        .map_err(|e| format!("Failed probing WEBP dimensions: {e:?}"))?;
    let has_alpha = decoded.data.chunks_exact(4).any(|px| px[3] < 255);
    Ok((decoded.width, decoded.height, has_alpha))
}

// ---------------------------------------------------------------------------
// 5. SVG / SVGZ RDF Metadata Writer
// ---------------------------------------------------------------------------

fn write_svg_keywords(path: &Path, keywords: &[String], gzipped: bool) -> Result<(), String> {
    let raw = std::fs::read(path).map_err(|e| format!("Failed reading SVG: {e}"))?;
    let xml_bytes = if gzipped || raw.starts_with(&[0x1f, 0x8b]) {
        let mut dec = GzDecoder::new(&raw[..]);
        let mut out = Vec::new();
        dec.read_to_end(&mut out)
            .map_err(|e| format!("Failed decompressing SVGZ: {e}"))?;
        out
    } else {
        raw
    };

    let xml = String::from_utf8_lossy(&xml_bytes).to_string();
    let updated = update_svg_metadata_xml(&xml, keywords)?;

    if gzipped {
        let mut enc = GzEncoder::new(Vec::new(), Compression::default());
        enc.write_all(updated.as_bytes())
            .map_err(|e| format!("SVGZ compress error: {e}"))?;
        let gz = enc
            .finish()
            .map_err(|e| format!("SVGZ finish error: {e}"))?;
        write_file_atomic(path, &gz)
    } else {
        write_file_atomic(path, updated.as_bytes())
    }
}

fn update_svg_metadata_xml(xml: &str, keywords: &[String]) -> Result<String, String> {
    if xml.contains("</rdf:Description>") {
        if let Some(updated) = update_existing_xmp_xml(xml, keywords) {
            return Ok(updated);
        }
    }

    let svg_open = xml
        .find("<svg")
        .ok_or_else(|| "Not a valid SVG document (missing <svg>)".to_string())?;
    let rel_gt = xml[svg_open..]
        .find('>')
        .ok_or_else(|| "Malformed <svg> opening tag".to_string())?;
    let insert_pos = svg_open + rel_gt + 1;

    if keywords.is_empty() {
        return Ok(xml.to_string());
    }

    let mut block = String::new();
    block.push_str(
        "\n  <metadata>\n   <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:MicrosoftPhoto=\"http://ns.microsoft.com/photo/1.0/\">\n    <rdf:Description rdf:about=\"\">\n     <dc:subject><rdf:Bag>",
    );
    for kw in keywords {
        block.push_str("<rdf:li>");
        block.push_str(&xml_escape(kw));
        block.push_str("</rdf:li>");
    }
    block.push_str("</rdf:Bag></dc:subject>\n    </rdf:Description>\n   </rdf:RDF>\n  </metadata>");

    let mut out = xml.to_string();
    out.insert_str(insert_pos, &block);
    Ok(out)
}

// ---------------------------------------------------------------------------
// 6. EPS / PS DSC %%Keywords Writer
// ---------------------------------------------------------------------------

fn write_ps_dsc_keywords(path: &Path, keywords: &[String]) -> Result<(), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("Failed reading PostScript: {e}"))?;
    if !bytes.starts_with(b"%!PS") {
        return Err("Binary or non-DSC PostScript header; skipping direct edit".to_string());
    }

    // Preserve exact line endings and any binary preview/body bytes after the DSC header.
    let eol: &[u8] = if bytes.windows(2).take(256).any(|w| w == b"\r\n") {
        b"\r\n"
    } else {
        b"\n"
    };

    // Locate first line end
    let first_nl = bytes
        .iter()
        .position(|&b| b == b'\n')
        .ok_or_else(|| "Malformed PostScript file".to_string())?;

    // Scan header lines up to %%EndComments or first non-comment line (capped at 64 KB)
    let scan_limit = bytes.len().min(65536);
    let mut pos = first_nl + 1;
    let mut existing_span: Option<(usize, usize)> = None;
    let mut insert_at = pos;

    while pos < scan_limit {
        let line_end = bytes[pos..scan_limit]
            .iter()
            .position(|&b| b == b'\n')
            .map(|p| pos + p + 1)
            .unwrap_or(scan_limit);
        let line = &bytes[pos..line_end];

        if line.starts_with(b"%%Keywords:") {
            existing_span = Some((pos, line_end));
            break;
        }
        if line.starts_with(b"%%EndComments") || !line.starts_with(b"%") {
            insert_at = pos;
            break;
        }
        insert_at = line_end;
        pos = line_end;
    }

    let new_line = if keywords.is_empty() {
        Vec::new()
    } else {
        let mut v = Vec::new();
        v.extend_from_slice(b"%%Keywords: ");
        v.extend_from_slice(keywords.join(", ").as_bytes());
        v.extend_from_slice(eol);
        v
    };

    let mut out = Vec::with_capacity(bytes.len() + new_line.len());
    if let Some((start, end)) = existing_span {
        out.extend_from_slice(&bytes[..start]);
        out.extend_from_slice(&new_line);
        out.extend_from_slice(&bytes[end..]);
    } else {
        out.extend_from_slice(&bytes[..insert_at]);
        out.extend_from_slice(&new_line);
        out.extend_from_slice(&bytes[insert_at..]);
    }

    write_file_atomic(path, &out)
}

// ---------------------------------------------------------------------------
// 7. Safe In-Place Writable Adobe XMP Packet Replacement (PDF / AVIF / JXL)
// ---------------------------------------------------------------------------

fn try_inplace_xmp_update(path: &Path, keywords: &[String]) -> Result<bool, String> {
    let mut bytes = std::fs::read(path).map_err(|e| format!("Failed reading file: {e}"))?;
    let Some(start) = find_subslice(&bytes, b"<?xpacket begin=") else {
        return Ok(false);
    };
    let rest = &bytes[start..];
    let Some(end_marker_rel) = find_subslice(rest, b"<?xpacket end=\"w\"?>") else {
        return Ok(false);
    };
    let total_len = end_marker_rel + b"<?xpacket end=\"w\"?>".len();
    let existing = String::from_utf8_lossy(&bytes[start..start + total_len]).to_string();

    let updated = build_xmp_packet(keywords, Some(&existing));
    let trailer = "<?xpacket end=\"w\"?>";
    let body_without_trailer = updated
        .rfind(trailer)
        .map(|p| &updated[..p])
        .unwrap_or(&updated);
    let min_needed = body_without_trailer.len() + trailer.len();
    if min_needed > total_len {
        return Ok(false);
    }

    let pad = total_len - min_needed;
    let mut padded = Vec::with_capacity(total_len);
    padded.extend_from_slice(body_without_trailer.as_bytes());
    padded.resize(body_without_trailer.len() + pad, b' ');
    padded.extend_from_slice(trailer.as_bytes());

    bytes[start..start + total_len].copy_from_slice(&padded);
    write_file_atomic(path, &bytes)?;
    Ok(true)
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

// ---------------------------------------------------------------------------
// 8. Native Windows Shell Property Store (IPropertyStore + SHChangeNotify)
// ---------------------------------------------------------------------------

fn try_windows_property_store_write(path: &Path, keywords: &[String]) -> Result<String, String> {
    #[cfg(windows)]
    {
        win_shell::write_property_store_keywords(path, keywords)
            .map(|()| "Windows Property Store (Explorer Tags)".to_string())
    }
    #[cfg(not(windows))]
    {
        let _ = (path, keywords);
        Err("Not running on Windows".to_string())
    }
}

fn notify_shell_item_updated(path: &Path) {
    #[cfg(windows)]
    {
        win_shell::sh_change_notify_update_item(path);
    }
    #[cfg(not(windows))]
    {
        let _ = path;
    }
}

#[cfg(windows)]
mod win_shell {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    const COINIT_APARTMENTTHREADED: u32 = 0x2;
    const S_OK: i32 = 0;
    const S_FALSE: i32 = 1;
    const RPC_E_CHANGED_MODE: i32 = 0x8001_0106u32 as i32;

    const GPS_DEFAULT: u32 = 0x0;
    const GPS_READWRITE: u32 = 0x2;

    const SHCNE_UPDATEITEM: i32 = 0x0000_2000;
    const SHCNF_PATHW: u32 = 0x0005;

    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Guid {
        data1: u32,
        data2: u16,
        data3: u16,
        data4: [u8; 8],
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct PropertyKey {
        fmtid: Guid,
        pid: u32,
    }

    // IID_IPropertyStore = {886D8EEB-8CF2-4446-8D02-CDBA1DBDCF99}
    const IID_IPROPERTYSTORE: Guid = Guid {
        data1: 0x886d_8eeb,
        data2: 0x8cf2,
        data3: 0x4446,
        data4: [0x8d, 0x02, 0xcd, 0xba, 0x1d, 0xbd, 0xcf, 0x99],
    };

    // PKEY_Keywords (System.Keywords) = {F29F85E0-4FF9-1068-AB91-08002B27B3D9}, PID 5
    const PKEY_KEYWORDS: PropertyKey = PropertyKey {
        fmtid: Guid {
            data1: 0xf29f_85e0,
            data2: 0x4ff9,
            data3: 0x1068,
            data4: [0xab, 0x91, 0x08, 0x00, 0x2b, 0x27, 0xb3, 0xd9],
        },
        pid: 5,
    };

    #[cfg(target_pointer_width = "64")]
    #[repr(C, align(8))]
    struct PropVariant {
        raw: [u64; 3],
    }

    #[cfg(target_pointer_width = "32")]
    #[repr(C, align(8))]
    struct PropVariant {
        raw: [u64; 2],
    }

    impl PropVariant {
        fn empty() -> Self {
            #[cfg(target_pointer_width = "64")]
            {
                Self { raw: [0; 3] }
            }
            #[cfg(target_pointer_width = "32")]
            {
                Self { raw: [0; 2] }
            }
        }
    }

    #[repr(C)]
    struct IPropertyStoreVtbl {
        query_interface:
            unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> i32,
        add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
        release: unsafe extern "system" fn(*mut c_void) -> u32,
        get_count: unsafe extern "system" fn(*mut c_void, *mut u32) -> i32,
        get_at: unsafe extern "system" fn(*mut c_void, u32, *mut PropertyKey) -> i32,
        get_value:
            unsafe extern "system" fn(*mut c_void, *const PropertyKey, *mut PropVariant) -> i32,
        set_value:
            unsafe extern "system" fn(*mut c_void, *const PropertyKey, *const PropVariant) -> i32,
        commit: unsafe extern "system" fn(*mut c_void) -> i32,
    }

    #[repr(C)]
    struct IPropertyStore {
        vtbl: *const IPropertyStoreVtbl,
    }

    type FnInitPropVariantFromStringVector =
        unsafe extern "system" fn(*const *const u16, u32, *mut PropVariant) -> i32;
    type FnPropVariantToStringVectorAlloc =
        unsafe extern "system" fn(*const PropVariant, *mut *mut *mut u16, *mut u32) -> i32;
    type FnSHGetPropertyStoreFromParsingName = unsafe extern "system" fn(
        *const u16,
        *mut c_void,
        u32,
        *const Guid,
        *mut *mut c_void,
    ) -> i32;

    #[link(name = "ole32")]
    extern "system" {
        fn CoInitializeEx(pv_reserved: *mut c_void, dw_co_init: u32) -> i32;
        fn CoUninitialize();
        fn CoTaskMemFree(pv: *mut c_void);
        fn PropVariantClear(pvar: *mut PropVariant) -> i32;
    }

    #[link(name = "shell32")]
    extern "system" {
        fn SHChangeNotify(
            w_event_id: i32,
            u_flags: u32,
            dw_item1: *const c_void,
            dw_item2: *const c_void,
        );
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn MoveFileExW(
            lp_existing_file_name: *const u16,
            lp_new_file_name: *const u16,
            dw_flags: u32,
        ) -> i32;
        fn LoadLibraryW(lp_lib_file_name: *const u16) -> *mut c_void;
        fn GetProcAddress(h_module: *mut c_void, lp_proc_name: *const u8) -> *mut c_void;
    }

    struct DynamicShellApis {
        sh_get_property_store: FnSHGetPropertyStoreFromParsingName,
        init_pv_from_str_vec: FnInitPropVariantFromStringVector,
        pv_to_str_vec_alloc: FnPropVariantToStringVectorAlloc,
    }

    fn load_shell_apis() -> Result<DynamicShellApis, String> {
        let shell32_name = str_to_wide_nul("shell32.dll");
        let propsys_name = str_to_wide_nul("propsys.dll");
        let h_shell32 = unsafe { LoadLibraryW(shell32_name.as_ptr()) };
        if h_shell32.is_null() {
            return Err("Failed to load shell32.dll".to_string());
        }
        let h_propsys = unsafe { LoadLibraryW(propsys_name.as_ptr()) };
        if h_propsys.is_null() {
            return Err("Failed to load propsys.dll".to_string());
        }
        let p_get_store = unsafe {
            GetProcAddress(
                h_shell32,
                b"SHGetPropertyStoreFromParsingName\0".as_ptr(),
            )
        };
        let p_init_vec = unsafe {
            GetProcAddress(
                h_propsys,
                b"InitPropVariantFromStringVector\0".as_ptr(),
            )
        };
        let p_to_vec = unsafe {
            GetProcAddress(
                h_propsys,
                b"PropVariantToStringVectorAlloc\0".as_ptr(),
            )
        };
        if p_get_store.is_null() || p_init_vec.is_null() || p_to_vec.is_null() {
            return Err("Required Windows Property System functions unavailable".to_string());
        }
        unsafe {
            Ok(DynamicShellApis {
                sh_get_property_store: std::mem::transmute(p_get_store),
                init_pv_from_str_vec: std::mem::transmute(p_init_vec),
                pv_to_str_vec_alloc: std::mem::transmute(p_to_vec),
            })
        }
    }

    struct ComGuard {
        should_uninit: bool,
    }

    impl ComGuard {
        fn init() -> Result<Self, String> {
            let hr = unsafe { CoInitializeEx(std::ptr::null_mut(), COINIT_APARTMENTTHREADED) };
            if hr == S_OK || hr == S_FALSE {
                Ok(Self {
                    should_uninit: true,
                })
            } else if hr == RPC_E_CHANGED_MODE {
                Ok(Self {
                    should_uninit: false,
                })
            } else {
                Err(format!("CoInitializeEx failed: 0x{:08X}", hr as u32))
            }
        }
    }

    impl Drop for ComGuard {
        fn drop(&mut self) {
            if self.should_uninit {
                unsafe {
                    CoUninitialize();
                }
            }
        }
    }

    fn to_wide_nul(path: &Path) -> Vec<u16> {
        let abs = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        // Strip \\?\ extended prefix (preserving UNC \\server\share) because Shell APIs prefer standard Win32 paths
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

    fn str_to_wide_nul(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn move_file_replace_existing(src: &Path, dst: &Path) -> Result<(), String> {
        let src_w: Vec<u16> = src
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let dst_w: Vec<u16> = dst
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let ok = unsafe {
            MoveFileExW(
                src_w.as_ptr(),
                dst_w.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        };
        if ok != 0 {
            Ok(())
        } else {
            Err(format!(
                "MoveFileExW failed: {}",
                std::io::Error::last_os_error()
            ))
        }
    }

    pub fn sh_change_notify_update_item(path: &Path) {
        let wide = to_wide_nul(path);
        unsafe {
            SHChangeNotify(
                SHCNE_UPDATEITEM,
                SHCNF_PATHW,
                wide.as_ptr() as *const c_void,
                std::ptr::null(),
            );
        }
    }

    pub fn write_property_store_keywords(path: &Path, keywords: &[String]) -> Result<(), String> {
        let _com = ComGuard::init()?;
        let apis = load_shell_apis()?;
        let wide_path = to_wide_nul(path);

        let mut raw_store: *mut c_void = std::ptr::null_mut();
        let hr = unsafe {
            (apis.sh_get_property_store)(
                wide_path.as_ptr(),
                std::ptr::null_mut(),
                GPS_READWRITE,
                &IID_IPROPERTYSTORE,
                &mut raw_store,
            )
        };
        if hr < 0 || raw_store.is_null() {
            return Err(format!(
                "SHGetPropertyStoreFromParsingName(GPS_READWRITE) failed: 0x{:08X}",
                hr as u32
            ));
        }

        let store = raw_store as *mut IPropertyStore;
        let result = (|| -> Result<(), String> {
            let mut pv = PropVariant::empty();
            if !keywords.is_empty() {
                let wide_kws: Vec<Vec<u16>> =
                    keywords.iter().map(|k| str_to_wide_nul(k)).collect();
                let ptrs: Vec<*const u16> = wide_kws.iter().map(|w| w.as_ptr()).collect();
                let hr_init = unsafe {
                    (apis.init_pv_from_str_vec)(ptrs.as_ptr(), ptrs.len() as u32, &mut pv)
                };
                if hr_init < 0 {
                    return Err(format!(
                        "InitPropVariantFromStringVector failed: 0x{:08X}",
                        hr_init as u32
                    ));
                }
            }

            let hr_set = unsafe { ((*(*store).vtbl).set_value)(raw_store, &PKEY_KEYWORDS, &pv) };
            unsafe {
                PropVariantClear(&mut pv);
            }
            if hr_set < 0 {
                return Err(format!(
                    "IPropertyStore::SetValue(PKEY_Keywords) failed: 0x{:08X}",
                    hr_set as u32
                ));
            }

            let hr_commit = unsafe { ((*(*store).vtbl).commit)(raw_store) };
            if hr_commit < 0 {
                return Err(format!(
                    "IPropertyStore::Commit failed: 0x{:08X}",
                    hr_commit as u32
                ));
            }
            Ok(())
        })();

        unsafe {
            ((*(*store).vtbl).release)(raw_store);
        }
        result
    }

    #[allow(dead_code)]
    pub fn read_property_store_keywords(path: &Path) -> Result<Vec<String>, String> {
        let _com = ComGuard::init()?;
        let apis = load_shell_apis()?;
        let wide_path = to_wide_nul(path);

        let mut raw_store: *mut c_void = std::ptr::null_mut();
        let hr = unsafe {
            (apis.sh_get_property_store)(
                wide_path.as_ptr(),
                std::ptr::null_mut(),
                GPS_DEFAULT,
                &IID_IPROPERTYSTORE,
                &mut raw_store,
            )
        };
        if hr < 0 || raw_store.is_null() {
            return Err(format!(
                "SHGetPropertyStoreFromParsingName(GPS_DEFAULT) failed: 0x{:08X}",
                hr as u32
            ));
        }

        let store = raw_store as *mut IPropertyStore;
        let result = (|| -> Result<Vec<String>, String> {
            let mut pv = PropVariant::empty();
            let hr_get =
                unsafe { ((*(*store).vtbl).get_value)(raw_store, &PKEY_KEYWORDS, &mut pv) };
            if hr_get < 0 {
                return Err(format!(
                    "IPropertyStore::GetValue failed: 0x{:08X}",
                    hr_get as u32
                ));
            }

            let mut arr: *mut *mut u16 = std::ptr::null_mut();
            let mut count: u32 = 0;
            let hr_vec = unsafe { (apis.pv_to_str_vec_alloc)(&pv, &mut arr, &mut count) };
            unsafe {
                PropVariantClear(&mut pv);
            }

            let mut out = Vec::new();
            if hr_vec >= 0 && !arr.is_null() {
                for i in 0..count as usize {
                    let s_ptr = unsafe { *arr.add(i) };
                    if !s_ptr.is_null() {
                        let mut len = 0usize;
                        while unsafe { *s_ptr.add(len) } != 0 {
                            len += 1;
                        }
                        let slice = unsafe { std::slice::from_raw_parts(s_ptr, len) };
                        let s = String::from_utf16_lossy(slice).trim().to_string();
                        if !s.is_empty() {
                            out.push(s);
                        }
                        unsafe {
                            CoTaskMemFree(s_ptr as *mut c_void);
                        }
                    }
                }
                unsafe {
                    CoTaskMemFree(arr as *mut c_void);
                }
            }
            Ok(out)
        })();

        unsafe {
            ((*(*store).vtbl).release)(raw_store);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cdr;
    use crate::config::GhostscriptConfig;
    use crate::decoder;
    use image::{ImageBuffer, Rgba};

    fn sample_rgba() -> image::DynamicImage {
        let mut buf = ImageBuffer::<Rgba<u8>, Vec<u8>>::new(32, 32);
        for (x, y, p) in buf.enumerate_pixels_mut() {
            *p = Rgba([(x * 8) as u8, (y * 8) as u8, 128, 255]);
        }
        image::DynamicImage::ImageRgba8(buf)
    }

    #[test]
    fn write_and_readback_keywords_png_jpeg_webp_svg_eps() {
        let tmp = std::env::temp_dir().join(format!(
            "thumbcraft_kw_test_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let img = sample_rgba();
        let kws = vec!["AlphaTag".to_string(), "Windows Explorer".to_string()];
        let gs = GhostscriptConfig::default();

        // 1. JPEG (uses Windows IPropertyStore on Windows!)
        let jpg_path = tmp.join("test.jpg");
        img.to_rgb8().save(&jpg_path).unwrap();
        let out_jpg = write_keywords_to_file(&jpg_path, &kws).unwrap();
        assert!(!out_jpg.method.is_empty());
        let dec_jpg = decoder::decode_thumbnail(&jpg_path, &gs);
        assert!(dec_jpg.image.is_some());
        assert!(dec_jpg.keywords.iter().any(|k| k == "AlphaTag"));
        assert!(dec_jpg.keywords.iter().any(|k| k == "Windows Explorer"));
        #[cfg(windows)]
        {
            let win_kws = read_windows_shell_keywords(&jpg_path);
            assert!(
                win_kws.iter().any(|k| k == "AlphaTag")
                    && win_kws.iter().any(|k| k == "Windows Explorer"),
                "Windows Explorer IPropertyStore did not see written JPEG keywords: {win_kws:?}"
            );
        }

        // 2. PNG
        let png_path = tmp.join("test.png");
        img.save(&png_path).unwrap();
        write_keywords_to_file(&png_path, &kws).unwrap();
        let dec_png = decoder::decode_thumbnail(&png_path, &gs);
        assert!(dec_png.image.is_some());
        assert!(dec_png.keywords.iter().any(|k| k == "AlphaTag"));
        assert!(dec_png.keywords.iter().any(|k| k == "Windows Explorer"));

        // 3. WEBP
        let webp_path = tmp.join("test.webp");
        let (webp_bytes, _) =
            decoder::encode_thumbnail(&img, crate::config::ThumbFormat::Webp, 85);
        std::fs::write(&webp_path, webp_bytes).unwrap();
        write_keywords_to_file(&webp_path, &kws).unwrap();
        let dec_webp = decoder::decode_thumbnail(&webp_path, &gs);
        assert!(dec_webp.image.is_some());
        assert!(dec_webp.keywords.iter().any(|k| k == "AlphaTag"));

        // 4. SVG
        let svg_path = tmp.join("test.svg");
        std::fs::write(
            &svg_path,
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"32\" height=\"32\"><rect width=\"32\" height=\"32\" fill=\"red\"/></svg>",
        )
        .unwrap();
        write_keywords_to_file(&svg_path, &kws).unwrap();
        let dec_svg = decoder::decode_thumbnail(&svg_path, &gs);
        assert!(dec_svg.image.is_some());
        assert!(dec_svg.keywords.iter().any(|k| k == "AlphaTag"));

        // 5. EPS
        let eps_path = tmp.join("test.eps");
        std::fs::write(
            &eps_path,
            "%!PS-Adobe-3.0 EPSF-3.0\n%%BoundingBox: 0 0 32 32\n%%EndComments\nshowpage\n%%EOF\n",
        )
        .unwrap();
        write_keywords_to_file(&eps_path, &kws).unwrap();
        let (eps_kws, _) = crate::ghostscript::extract_ps_pdf_metadata(&eps_path);
        assert!(eps_kws.iter().any(|k| k == "AlphaTag"));
        assert!(eps_kws.iter().any(|k| k == "Windows Explorer"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn write_and_readback_keywords_zip_and_riff_cdr() {
        let tmp = std::env::temp_dir().join(format!(
            "thumbcraft_cdr_kw_test_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        let sample_dir = crate::sample_gen::ensure_sample_library(&tmp);

        let zip_cdr = sample_dir.join("01_Botanical_Logo_X7.cdr");
        let riff_cdr = sample_dir.join("02_Vintage_Badge_v12.cdr");

        let new_kws = vec![
            "Botanical".to_string(),
            "CustomExplorerTag".to_string(),
        ];

        // ZIP CDR roundtrip + Windows Explorer ShellVista.dll readback verification
        write_keywords_to_file(&zip_cdr, &new_kws).unwrap();
        let parsed_zip = cdr::parse_cdr(&zip_cdr).unwrap();
        assert!(parsed_zip.preview.is_some());
        assert!(parsed_zip.keywords.iter().any(|k| k == "CustomExplorerTag"));
        assert!(parsed_zip.keywords.iter().any(|k| k == "Botanical"));
        #[cfg(windows)]
        {
            let win_zip_kws = read_windows_shell_keywords(&zip_cdr);
            if !win_zip_kws.is_empty() {
                assert!(
                    win_zip_kws.iter().any(|k| k == "CustomExplorerTag")
                        && win_zip_kws.iter().any(|k| k == "Botanical"),
                    "Windows Explorer CDR handler did not see ZIP CDR keywords: {win_zip_kws:?}"
                );
            }
        }

        // RIFF CDR v12 roundtrip (UTF-16LE IKEY) + Windows Explorer ShellVista.dll readback verification
        write_keywords_to_file(&riff_cdr, &new_kws).unwrap();
        let parsed_riff = cdr::parse_cdr(&riff_cdr).unwrap();
        assert!(parsed_riff.preview.is_some());
        assert!(parsed_riff.keywords.iter().any(|k| k == "CustomExplorerTag"));
        #[cfg(windows)]
        {
            let win_riff_kws = read_windows_shell_keywords(&riff_cdr);
            if !win_riff_kws.is_empty() {
                assert!(
                    win_riff_kws.iter().any(|k| k == "CustomExplorerTag")
                        && win_riff_kws.iter().any(|k| k == "Botanical"),
                    "Windows Explorer CDR handler did not see RIFF v12 CDR keywords: {win_riff_kws:?}"
                );
            }
        }

        // Removing a keyword persists back to the CDR file & Windows Explorer too
        let trimmed_kws = vec!["Botanical".to_string()];
        write_keywords_to_file(&zip_cdr, &trimmed_kws).unwrap();
        let parsed_zip2 = cdr::parse_cdr(&zip_cdr).unwrap();
        assert!(!parsed_zip2.keywords.iter().any(|k| k == "CustomExplorerTag"));
        assert!(parsed_zip2.keywords.iter().any(|k| k == "Botanical"));
        #[cfg(windows)]
        {
            let win_zip_kws2 = read_windows_shell_keywords(&zip_cdr);
            if !win_zip_kws2.is_empty() {
                assert!(
                    !win_zip_kws2.iter().any(|k| k == "CustomExplorerTag")
                        && win_zip_kws2.iter().any(|k| k == "Botanical"),
                    "Windows Explorer CDR handler still saw removed ZIP CDR keyword: {win_zip_kws2:?}"
                );
            }
        }

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
