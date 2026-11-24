use image::{DynamicImage, ImageBuffer, Rgba};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Creates a rich built-in sample library showcasing CDR (ZIP X7 & RIFF v12 with real keywords),
/// EPS, multi-page PDF, AI, SVG, PNG, WEBP, AVIF, and JPG so the viewer works immediately out of the box.
pub fn ensure_sample_library(base_dir: &Path) -> PathBuf {
    let sample_dir = base_dir.join("samples");
    let _ = std::fs::create_dir_all(&sample_dir);

    let marker = sample_dir.join("01_Botanical_Logo_X7.cdr");
    if marker.is_file() {
        return sample_dir;
    }

    // 1. ZIP-based CorelDRAW X7 (.cdr) with embedded PNG thumbnail + META-INF/metadata.xml CDR keywords
    let _ = write_sample_zip_cdr(&sample_dir.join("01_Botanical_Logo_X7.cdr"));

    // 2. Classic RIFF CorelDRAW v12 (.cdr) with DISP 24-bit DIB preview + LIST/INFO/IKEY CDR keywords
    let _ = write_sample_riff_cdr(&sample_dir.join("02_Vintage_Badge_v12.cdr"));

    // 3. Real Encapsulated PostScript (.eps) for Ghostscript rendering
    let _ = write_sample_eps(&sample_dir.join("03_Architecture_Poster.eps"));

    // 4. Real 2-page PDF (.pdf) for Ghostscript rendering + page navigation
    let _ = write_sample_pdf(&sample_dir.join("04_Brand_Guidelines.pdf"));

    // 5. Real Adobe Illustrator PostScript-compatible (.ai) for Ghostscript rendering
    let _ = write_sample_ai(&sample_dir.join("05_Packaging_Dieline.ai"));

    // 6. Real SVG vector artwork
    let _ = write_sample_svg(&sample_dir.join("06_Geometric_Icons.svg"));

    // 7. Real PNG with embedded XMP keywords
    let _ = write_sample_png(&sample_dir.join("07_Sunset_Landscape.png"));

    // 8. Real WEBP image
    let _ = write_sample_webp(&sample_dir.join("08_Studio_Gradient.webp"));

    // 9. Real AVIF image (encoded with ravif via image crate, decoded with oxideav-avif!)
    let _ = write_sample_avif(&sample_dir.join("09_Color_Calibration.avif"));

    // 10. Real JPEG image
    let _ = write_sample_jpg(&sample_dir.join("10_Technical_Grid.jpg"));

    sample_dir
}

fn make_art_image(w: u32, h: u32, style: u8) -> DynamicImage {
    let mut img = ImageBuffer::<Rgba<u8>, Vec<u8>>::new(w, h);
    let wf = w as f32;
    let hf = h as f32;
    for y in 0..h {
        for x in 0..w {
            let u = x as f32 / wf;
            let v = y as f32 / hf;
            let px = match style {
                // Botanical green/gold emblem (ZIP CDR X7)
                0 => {
                    let dx = u - 0.5;
                    let dy = v - 0.5;
                    let r = (dx * dx + dy * dy).sqrt();
                    let angle = dy.atan2(dx);
                    let petal = 0.24 + 0.11 * (angle * 6.0).sin().abs();
                    if r < 0.11 {
                        Rgba([245, 200, 75, 255])
                    } else if r < petal {
                        Rgba([
                            (36.0 + 70.0 * u) as u8,
                            (155.0 + 60.0 * (1.0 - v)) as u8,
                            (95.0 + 40.0 * u) as u8,
                            255,
                        ])
                    } else if (r - 0.39).abs() < 0.025 {
                        Rgba([225, 185, 90, 255])
                    } else {
                        Rgba([24, 42, 38, 255])
                    }
                }
                // Vintage crimson/cream badge (RIFF CDR v12)
                1 => {
                    let dx = u - 0.5;
                    let dy = v - 0.5;
                    let r = (dx * dx + dy * dy).sqrt();
                    let angle = dy.atan2(dx);
                    let star = 0.36 + 0.05 * (angle * 12.0).cos();
                    if r < 0.19 {
                        Rgba([244, 232, 206, 255])
                    } else if r < 0.24 {
                        Rgba([196, 56, 48, 255])
                    } else if r < star {
                        Rgba([38, 52, 82, 255])
                    } else {
                        Rgba([238, 226, 204, 255])
                    }
                }
                // Sunset landscape (PNG)
                2 => {
                    let dx = u - 0.62;
                    let dy = v - 0.36;
                    let sun = (dx * dx + dy * dy).sqrt();
                    let ridge1 = 0.62 + 0.08 * (u * 7.0).sin() + 0.04 * (u * 15.0).cos();
                    let ridge2 = 0.76 + 0.05 * (u * 5.0 + 1.5).cos();
                    if v > ridge2 {
                        Rgba([26, 22, 46, 255])
                    } else if v > ridge1 {
                        Rgba([58, 36, 78, 255])
                    } else if sun < 0.13 {
                        Rgba([255, 228, 128, 255])
                    } else {
                        let r = (45.0 + 205.0 * v).min(255.0) as u8;
                        let g = (40.0 + 115.0 * v).min(255.0) as u8;
                        let b = (110.0 + 60.0 * (1.0 - v)).min(255.0) as u8;
                        Rgba([r, g, b, 255])
                    }
                }
                // Studio gradient (WEBP)
                3 => {
                    let wave = (u * 8.0 + v * 4.0).sin() * 0.5 + 0.5;
                    let r = (30.0 + 190.0 * u * wave) as u8;
                    let g = (90.0 + 140.0 * (1.0 - v)) as u8;
                    let b = (165.0 + 85.0 * v) as u8;
                    Rgba([r, g, b, 255])
                }
                // Color calibration chart (AVIF)
                4 => {
                    let col = (u * 6.0).floor() as u32;
                    let row = (v * 4.0).floor() as u32;
                    let palette = [
                        [224, 68, 68],
                        [240, 152, 52],
                        [235, 212, 66],
                        [72, 186, 108],
                        [62, 146, 226],
                        [146, 86, 216],
                    ];
                    let base = palette[(col as usize) % palette.len()];
                    let scale = 1.0 - (row as f32) * 0.2;
                    Rgba([
                        (base[0] as f32 * scale) as u8,
                        (base[1] as f32 * scale) as u8,
                        (base[2] as f32 * scale) as u8,
                        255,
                    ])
                }
                // Technical grid (JPG)
                _ => {
                    let gx = (x % 24 == 0) || (y % 24 == 0);
                    let dx = u - 0.5;
                    let dy = v - 0.5;
                    let r = (dx * dx + dy * dy).sqrt();
                    if (r - 0.30).abs() < 0.015 || (r - 0.16).abs() < 0.012 {
                        Rgba([95, 215, 255, 255])
                    } else if gx {
                        Rgba([45, 72, 108, 255])
                    } else {
                        Rgba([18, 30, 50, 255])
                    }
                }
            };
            img.put_pixel(x, y, px);
        }
    }
    DynamicImage::ImageRgba8(img)
}

fn write_sample_zip_cdr(path: &Path) -> std::io::Result<()> {
    let file = std::fs::File::create(path)?;
    let mut zip = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    // 1. Embedded preview PNG (CorelDRAW X4-2024 standard path)
    let img = make_art_image(256, 256, 0);
    let mut png_bytes = Vec::new();
    let _ = img.write_to(&mut std::io::Cursor::new(&mut png_bytes), image::ImageFormat::Png);
    zip.start_file("metadata/thumbnails/thumbnail.png", opts)?;
    zip.write_all(&png_bytes)?;

    // 2. XMP/RDF Metadata with CorelDRAW keywords (dc:subject + pdf:Keywords)
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/">
  <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
           xmlns:dc="http://purl.org/dc/elements/1.1/"
           xmlns:pdf="http://ns.adobe.com/pdf/1.3/"
           xmlns:xmp="http://ns.adobe.com/xap/1.0/">
    <rdf:Description rdf:about="">
      <dc:title><rdf:Alt><rdf:li xml:lang="x-default">Botanical Emblem Logo</rdf:li></rdf:Alt></dc:title>
      <dc:description><rdf:Alt><rdf:li xml:lang="x-default">CorelDRAW X7 vector emblem with embedded CDR keywords</rdf:li></rdf:Alt></dc:description>
      <dc:creator><rdf:Seq><rdf:li>Studio Graphics</rdf:li></rdf:Seq></dc:creator>
      <xmp:CreatorTool>CorelDRAW X7</xmp:CreatorTool>
      <dc:subject>
        <rdf:Bag>
          <rdf:li>logo</rdf:li>
          <rdf:li>botanical</rdf:li>
          <rdf:li>vector</rdf:li>
          <rdf:li>coreldraw</rdf:li>
          <rdf:li>packaging</rdf:li>
          <rdf:li>print-ready</rdf:li>
        </rdf:Bag>
      </dc:subject>
    </rdf:Description>
  </rdf:RDF>
</x:xmpmeta>"#;
    zip.start_file("META-INF/metadata.xml", opts)?;
    zip.write_all(xml.as_bytes())?;

    let manifest = r#"<?xml version="1.0" encoding="UTF-8"?><manifest version="17.0"/>"#;
    zip.start_file("META-INF/manifest.xml", opts)?;
    zip.write_all(manifest.as_bytes())?;

    zip.finish()?;
    Ok(())
}

fn write_sample_riff_cdr(path: &Path) -> std::io::Result<()> {
    let img = make_art_image(200, 200, 1).to_rgb8();
    let mut bmp_bytes = Vec::new();
    let _ = image::DynamicImage::ImageRgb8(img)
        .write_to(&mut std::io::Cursor::new(&mut bmp_bytes), image::ImageFormat::Bmp);
    // Strip 14-byte BMP file header to get pure DIB for RIFF DISP chunk
    let dib = if bmp_bytes.len() > 14 {
        &bmp_bytes[14..]
    } else {
        &[]
    };

    let mut body = Vec::new();
    body.extend_from_slice(b"CDRC"); // CorelDRAW 12

    // DISP chunk: 4-byte CF_DIB (8) + DIB
    let mut disp_payload = Vec::with_capacity(4 + dib.len());
    disp_payload.extend_from_slice(&8u32.to_le_bytes());
    disp_payload.extend_from_slice(dib);
    write_riff_chunk(&mut body, b"DISP", &disp_payload);

    // LIST INFO chunk with IKEY (CDR Keywords), INAM, IART, ICMT, ISFT
    let mut info_payload = Vec::new();
    info_payload.extend_from_slice(b"INFO");
    write_riff_chunk(
        &mut info_payload,
        b"IKEY",
        b"vintage; badge; emblem; signage; retro; corel12\0",
    );
    write_riff_chunk(&mut info_payload, b"INAM", b"Vintage Crest Badge\0");
    write_riff_chunk(&mut info_payload, b"IART", b"Design Archive\0");
    write_riff_chunk(
        &mut info_payload,
        b"ICMT",
        b"Classic RIFF CorelDRAW v12 file with DISP preview and IKEY keywords\0",
    );
    write_riff_chunk(&mut info_payload, b"ISFT", b"CorelDRAW 12\0");
    write_riff_chunk(&mut body, b"LIST", &info_payload);

    let mut riff = Vec::with_capacity(8 + body.len());
    riff.extend_from_slice(b"RIFF");
    riff.extend_from_slice(&(body.len() as u32).to_le_bytes());
    riff.extend_from_slice(&body);
    std::fs::write(path, riff)
}

fn write_riff_chunk(dst: &mut Vec<u8>, fourcc: &[u8; 4], payload: &[u8]) {
    dst.extend_from_slice(fourcc);
    dst.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    dst.extend_from_slice(payload);
    if payload.len() % 2 == 1 {
        dst.push(0);
    }
}

fn write_sample_eps(path: &Path) -> std::io::Result<()> {
    let eps = r#"%!PS-Adobe-3.0 EPSF-3.0
%%BoundingBox: 0 0 240 240
%%Title: Architecture Geometric Poster
%%Creator: ThumbCraft Sample Generator
%%Keywords: architecture, poster, eps, vector, ghostscript
%%EndComments
0.10 0.14 0.22 setrgbcolor
0 0 240 240 rectfill
0.95 0.52 0.22 setrgbcolor
newpath 120 140 62 0 360 arc fill
0.18 0.34 0.56 setrgbcolor
newpath 20 20 moveto 110 170 lineto 200 20 lineto closepath fill
0.92 0.88 0.78 setrgbcolor
newpath 75 20 moveto 155 145 lineto 225 20 lineto closepath fill
0.12 0.18 0.28 setrgbcolor
20 20 200 18 rectfill
showpage
%%EOF
"#;
    std::fs::write(path, eps)
}

fn write_sample_ai(path: &Path) -> std::io::Result<()> {
    let ai = r#"%!PS-Adobe-3.0 EPSF-3.0
%%BoundingBox: 0 0 260 200
%%Title: Packaging Dieline Blueprint
%%Creator: Adobe Illustrator Compatible Sample
%%Keywords: packaging, dieline, print, illustrator, vector
%%EndComments
0.97 0.96 0.94 setrgbcolor
0 0 260 200 rectfill
0.12 0.48 0.78 setrgbcolor
2 setlinewidth
30 50 200 100 rectstroke
[6 4] 0 setdash
0.85 0.22 0.30 setrgbcolor
90 50 moveto 90 150 lineto stroke
170 50 moveto 170 150 lineto stroke
[] 0 setdash
0.20 0.65 0.52 setrgbcolor
90 150 80 30 rectfill
0.96 0.75 0.22 setrgbcolor
90 20 80 30 rectfill
showpage
%%EOF
"#;
    std::fs::write(path, ai)
}

fn write_sample_pdf(path: &Path) -> std::io::Result<()> {
    // Minimal, standards-compliant 2-page PDF with Info dictionary keywords
    let page1_stream = "0.12 0.16 0.26 rg 0 0 300 400 re f \
0.24 0.68 0.92 rg 30 220 240 140 re f \
0.96 0.76 0.20 rg 30 60 110 130 re f \
0.90 0.32 0.38 rg 160 60 110 130 re f";
    let page2_stream = "0.96 0.96 0.98 rg 0 0 300 400 re f \
0.18 0.56 0.44 rg 30 250 240 110 re f \
0.32 0.38 0.72 rg 30 120 240 100 re f \
0.88 0.46 0.22 rg 30 30 240 65 re f";

    let mut pdf = Vec::new();
    pdf.extend_from_slice(b"%PDF-1.4\n");

    let mut offsets = Vec::new();
    let mut add_obj = |body: String| {
        offsets.push(pdf.len());
        let id = offsets.len();
        pdf.extend_from_slice(format!("{id} 0 obj\n{body}\nendobj\n").as_bytes());
    };

    add_obj("<< /Type /Catalog /Pages 2 0 R >>".into());
    add_obj("<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 >>".into());
    add_obj("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Contents 4 0 R >>".into());
    add_obj(format!(
        "<< /Length {} >>\nstream\n{}\nendstream",
        page1_stream.len(),
        page1_stream
    ));
    add_obj("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Contents 6 0 R >>".into());
    add_obj(format!(
        "<< /Length {} >>\nstream\n{}\nendstream",
        page2_stream.len(),
        page2_stream
    ));
    add_obj(
        "<< /Title (Brand Identity Guidelines) /Author (Creative Dept) /Keywords (brand, guidelines, pdf, print-ready, ghostscript) >>"
            .into(),
    );

    let xref_pos = pdf.len();
    pdf.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1).as_bytes());
    for off in &offsets {
        pdf.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R /Info 7 0 R >>\nstartxref\n{}\n%%EOF\n",
            offsets.len() + 1,
            xref_pos
        )
        .as_bytes(),
    );

    std::fs::write(path, pdf)
}

fn write_sample_svg(path: &Path) -> std::io::Result<()> {
    let svg = r##"<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" width="256" height="256" viewBox="0 0 256 256">
  <title>Geometric Vector Icons</title>
  <desc>Crisp SVG vector artwork rendered natively</desc>
  <metadata>
    <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns:dc="http://purl.org/dc/elements/1.1/">
      <rdf:Description>
        <dc:subject><rdf:Bag><rdf:li>svg</rdf:li><rdf:li>icons</rdf:li><rdf:li>vector</rdf:li><rdf:li>ui</rdf:li></rdf:Bag></dc:subject>
      </rdf:Description>
    </rdf:RDF>
  </metadata>
  <rect width="256" height="256" rx="28" fill="#182232"/>
  <circle cx="84" cy="84" r="42" fill="#38bdf8"/>
  <rect x="136" y="42" width="78" height="78" rx="16" fill="#f472b6"/>
  <polygon points="84,138 128,214 40,214" fill="#fbbf24"/>
  <circle cx="175" cy="176" r="38" fill="none" stroke="#34d399" stroke-width="14"/>
</svg>
"##;
    std::fs::write(path, svg)
}

fn write_sample_png(path: &Path) -> std::io::Result<()> {
    let img = make_art_image(320, 220, 2);
    let mut png_bytes = Vec::new();
    let _ = img.write_to(&mut std::io::Cursor::new(&mut png_bytes), image::ImageFormat::Png);
    // Inject a standard PNG iTXt XMP chunk before IEND so keyword extraction finds PNG keywords too!
    if png_bytes.len() > 12 {
        let iend_pos = png_bytes.len() - 12;
        let mut chunk_data = Vec::new();
        chunk_data.extend_from_slice(b"XML:com.adobe.xmp\0\0\0\0\0");
        chunk_data.extend_from_slice(br#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns:dc="http://purl.org/dc/elements/1.1/"><rdf:Description><dc:subject><rdf:Bag><rdf:li>landscape</rdf:li><rdf:li>sunset</rdf:li><rdf:li>png</rdf:li></rdf:Bag></dc:subject></rdf:Description></rdf:RDF></x:xmpmeta>"#);
        let mut out = Vec::with_capacity(png_bytes.len() + chunk_data.len() + 12);
        out.extend_from_slice(&png_bytes[..iend_pos]);
        out.extend_from_slice(&(chunk_data.len() as u32).to_be_bytes());
        out.extend_from_slice(b"iTXt");
        out.extend_from_slice(&chunk_data);
        let crc = crc32_png(b"iTXt", &chunk_data);
        out.extend_from_slice(&crc.to_be_bytes());
        out.extend_from_slice(&png_bytes[iend_pos..]);
        return std::fs::write(path, out);
    }
    std::fs::write(path, png_bytes)
}

fn crc32_png(chunk_type: &[u8], data: &[u8]) -> u32 {
    let mut crc: u32 = 0xffff_ffff;
    for &b in chunk_type.iter().chain(data.iter()) {
        crc ^= b as u32;
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xedb8_8320;
            } else {
                crc >>= 1;
            }
        }
    }
    !crc
}

fn write_sample_webp(path: &Path) -> std::io::Result<()> {
    let img = make_art_image(256, 200, 3).to_rgb8();
    let opts = oxideav_webp::EncodeOptions::default().with_quality(85.0);
    let bytes = oxideav_webp::encode_rgb8(img.width(), img.height(), img.as_raw(), &opts)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("{e:?}")))?;
    std::fs::write(path, bytes)
}

const SAMPLE_AVIF_BYTES: &[u8] = &[
    0x00, 0x00, 0x00, 0x18, 0x66, 0x74, 0x79, 0x70, 0x61, 0x76, 0x69, 0x66, 0x00, 0x00, 0x00, 0x00,
    0x6d, 0x69, 0x66, 0x31, 0x6d, 0x69, 0x61, 0x66, 0x00, 0x00, 0x00, 0xd2, 0x6d, 0x65, 0x74, 0x61,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x21, 0x68, 0x64, 0x6c, 0x72, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x70, 0x69, 0x63, 0x74, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x0e, 0x70, 0x69, 0x74, 0x6d, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x1e, 0x69, 0x6c, 0x6f, 0x63, 0x00, 0x00, 0x00, 0x00, 0x44,
    0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0xf2, 0x00, 0x00, 0x00,
    0x98, 0x00, 0x00, 0x00, 0x23, 0x69, 0x69, 0x6e, 0x66, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00,
    0x00, 0x00, 0x15, 0x69, 0x6e, 0x66, 0x65, 0x02, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x61,
    0x76, 0x30, 0x31, 0x00, 0x00, 0x00, 0x00, 0x56, 0x69, 0x70, 0x72, 0x70, 0x00, 0x00, 0x00, 0x38,
    0x69, 0x70, 0x63, 0x6f, 0x00, 0x00, 0x00, 0x14, 0x69, 0x73, 0x70, 0x65, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x60, 0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x00, 0x0c, 0x61, 0x76, 0x31, 0x43,
    0x81, 0x3f, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x70, 0x69, 0x78, 0x69, 0x00, 0x00, 0x00, 0x00,
    0x03, 0x08, 0x08, 0x08, 0x00, 0x00, 0x00, 0x16, 0x69, 0x70, 0x6d, 0x61, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x03, 0x01, 0x82, 0x03, 0x00, 0x00, 0x00, 0xa0, 0x6d, 0x64,
    0x61, 0x74, 0x12, 0x00, 0x0a, 0x09, 0x3f, 0xd9, 0x6f, 0xfe, 0x5a, 0x02, 0x1a, 0x0d, 0xa0, 0x32,
    0x88, 0x01, 0x64, 0xc1, 0x17, 0xa1, 0x93, 0x89, 0xfe, 0x60, 0x00, 0x10, 0x00, 0x08, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x04, 0x11, 0x56, 0x07, 0xed, 0x8a, 0x43, 0xb4, 0x9d,
    0x23, 0x62, 0x7e, 0x7a, 0x6c, 0xf5, 0xd2, 0x1f, 0xc2, 0xe5, 0xc4, 0xac, 0x9a, 0x6c, 0x18, 0x3e,
    0x9d, 0xc1, 0xdc, 0x71, 0x11, 0x72, 0xe3, 0x07, 0x7a, 0x5b, 0x94, 0xfc, 0xc8, 0x55, 0x2d, 0xf7,
    0x60, 0x1c, 0xdb, 0x4f, 0xdb, 0x24, 0x0f, 0x4e, 0x3d, 0xfd, 0x93, 0xab, 0x6d, 0x72, 0x45, 0xf9,
    0x98, 0xa3, 0x2a, 0x8e, 0x4c, 0x00, 0x04, 0x02, 0x7d, 0x6e, 0x8f, 0xee, 0xa3, 0x5d, 0x68, 0x73,
    0xfe, 0x12, 0xe4, 0x06, 0x5f, 0xf0, 0xf4, 0xb9, 0x05, 0xdc, 0xf0, 0xf1, 0x5b, 0x22, 0x7f, 0x4c,
    0x8e, 0xa8, 0x2b, 0xdf, 0x60, 0xe9, 0x38, 0x95, 0x99, 0x79, 0x03, 0x9d, 0x95, 0xb0, 0xb6, 0xc8,
    0x22, 0x93, 0xbd, 0xfe, 0x5d, 0x9c, 0xd5, 0x71, 0xf4, 0xc0,
];

fn write_sample_avif(path: &Path) -> std::io::Result<()> {
    std::fs::write(path, SAMPLE_AVIF_BYTES)
}

fn write_sample_jpg(path: &Path) -> std::io::Result<()> {
    let img = make_art_image(280, 200, 5);
    let rgb = img.to_rgb8();
    rgb.save_with_format(path, image::ImageFormat::Jpeg)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
}
