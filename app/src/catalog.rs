use crate::decoder;
use crate::similarity::{self, VisualSignature};
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use flate2::Compression;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// Single-file compressed relational catalog database (`thumbcraft.tcdb`), modeled after
/// ThumbsPlus_tpdb8_001.schema.py (Path, Thumbnail, Keyword, ThumbnailKeyword, Gallery,
/// ThumbnailGallery, UserSettings).
///
/// Binary layout (`TCDB0001`):
/// - [0..8]   Magic: b"TCDB0001"
/// - [8..16]  u64 LE: byte offset of the latest compressed catalog index block
/// - [16..]   Append-only region of compressed thumbnail blobs (WEBP / JXL / JPG / PNG)
///            and zlib-compressed JSON index snapshots `[u32 len][zlib bytes]`.
///
/// Because new blobs and index snapshots are appended past the previous index before updating
/// the 8-byte header pointer at [8..16], the database stays crash-consistent even if stopped
/// mid-scan. `compact()` rewrites the file cleanly with only live blobs.

const TCDB_MAGIC: &[u8; 8] = b"TCDB0001";
const TCDB_HEADER_LEN: u64 = 16;

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct CatalogRecord {
    pub id_thumb: i64,
    pub full_path: PathBuf,
    pub file_name: String,
    pub filetype: String,
    pub file_time_iso: String,
    pub file_length: u64,
    pub width: u32,
    pub height: u32,
    pub color_depth: u16,
    pub page_count: u32,
    pub rating: u8,
    pub annotation: String,
    pub keywords: Vec<String>,
    pub metadata: BTreeMap<String, String>,
    pub thumb_blob: Option<Vec<u8>>,
    pub thumb_fmt: String,
    pub md5: String,
    pub visual_sig: Option<VisualSignature>,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct DuplicateGroup {
    pub group_index: usize,
    pub md5: String,
    pub file_size: u64,
    pub paths: Vec<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct SimilarMatch {
    pub path: PathBuf,
    pub similarity: f32,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Default)]
pub struct CatalogStats {
    pub total_thumbnails: usize,
    pub total_keywords: usize,
    pub total_folders: usize,
    pub total_galleries: usize,
    pub db_size_bytes: u64,
}

#[derive(Debug, Clone, Default)]
pub struct CompactReport {
    pub bytes_before: u64,
    pub bytes_after: u64,
    pub removed_thumbs: usize,
    pub removed_keywords: usize,
    pub removed_paths: usize,
}

fn is_zero_u8(v: &u8) -> bool {
    *v == 0
}
fn is_one_u32(v: &u32) -> bool {
    *v == 1
}
fn default_one_u32() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredThumb {
    id_thumb: i64,
    id_path: i64,
    name: String,
    filetype: String,
    file_time_iso: String,
    thumbnail_time_iso: String,
    filelength: u64,
    width: u32,
    height: u32,
    colordepth: u16,
    #[serde(default = "default_one_u32", skip_serializing_if = "is_one_u32")]
    page_count: u32,
    #[serde(default, skip_serializing_if = "is_zero_u8")]
    rating: u8,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    annotation: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    metadata: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    md5: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    visual_sig: Option<VisualSignature>,
    #[serde(default)]
    blob_offset: u64,
    #[serde(default)]
    blob_len: u32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    blob_fmt: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct StoredDb {
    next_path_id: i64,
    next_thumb_id: i64,
    next_keyword_id: i64,
    next_gallery_id: i64,
    /// Path table: idPath -> folder path
    paths: BTreeMap<i64, String>,
    /// Thumbnail table: idThumb -> StoredThumb
    thumbnails: BTreeMap<i64, StoredThumb>,
    /// Keyword table: idKeyword -> keyword
    keywords: BTreeMap<i64, String>,
    /// ThumbnailKeyword join table: (idKeyword, idThumb)
    thumb_keywords: BTreeSet<(i64, i64)>,
    /// Gallery table: idGallery -> gallery name
    galleries: BTreeMap<i64, String>,
    /// ThumbnailGallery join table: idGallery -> ordered vec of idThumb
    thumb_galleries: BTreeMap<i64, Vec<i64>>,
    /// UserSettings table
    user_settings: BTreeMap<String, String>,
    /// Normalized lowercase paths of folders that have been fully scanned
    #[serde(default)]
    scanned_folders: BTreeSet<String>,
}

pub struct CatalogDb {
    db_path: PathBuf,
    file: File,
    write_cursor: u64,
    data: StoredDb,
    /// O(1) lookup: lowercase dir -> id_path
    path_index: HashMap<String, i64>,
    /// O(1) lookup: (id_path, lowercase name) -> id_thumb
    thumb_index: HashMap<(i64, String), i64>,
    /// O(1) lookup: lowercase keyword -> id_keyword
    keyword_index: HashMap<String, i64>,
    /// O(1) lookup: id_thumb -> list of id_keyword
    thumb_kw_map: HashMap<i64, Vec<i64>>,
    /// Tracks whether migration from legacy json+thumb_cache occurred
    pub migrated_legacy_count: usize,
}

impl CatalogDb {
    pub fn open(db_path: &Path) -> Result<Self, String> {
        if let Some(parent) = db_path.parent() {
            if !parent.as_os_str().is_empty() {
                let _ = std::fs::create_dir_all(parent);
            }
        }

        let mut migrated_legacy_count = 0usize;

        if db_path.is_file() && std::fs::metadata(db_path).map(|m| m.len()).unwrap_or(0) >= TCDB_HEADER_LEN {
            let mut file = OpenOptions::new()
                .read(true)
                .write(true)
                .open(db_path)
                .map_err(|e| format!("Failed to open {}: {e}", db_path.display()))?;

            let mut hdr = [0u8; 16];
            file.read_exact(&mut hdr)
                .map_err(|e| format!("Failed reading TCDB header: {e}"))?;

            if &hdr[0..8] == TCDB_MAGIC {
                let index_offset = u64::from_le_bytes(hdr[8..16].try_into().unwrap());
                let file_len = file
                    .metadata()
                    .map(|m| m.len())
                    .unwrap_or(TCDB_HEADER_LEN);
                let data = read_index_at(&mut file, index_offset).unwrap_or_else(|_| new_empty_db());
                let mut db = Self::from_parts(db_path.to_path_buf(), file, file_len.max(TCDB_HEADER_LEN), data);
                db.migrated_legacy_count = 0;
                return Ok(db);
            }
        }

        // Create a fresh single-file TCDB database
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(db_path)
            .map_err(|e| format!("Failed to create {}: {e}", db_path.display()))?;

        file.write_all(TCDB_MAGIC)
            .map_err(|e| format!("Write header failed: {e}"))?;
        file.write_all(&TCDB_HEADER_LEN.to_le_bytes())
            .map_err(|e| format!("Write header offset failed: {e}"))?;

        let mut db = Self::from_parts(
            db_path.to_path_buf(),
            file,
            TCDB_HEADER_LEN,
            new_empty_db(),
        );

        // One-time seamless migration from legacy thumbcraft_catalog.json + thumb_cache/
        if let Some(parent) = db_path.parent() {
            let legacy_json = parent.join("thumbcraft_catalog.json");
            let legacy_cache = parent.join("thumb_cache");
            if legacy_json.is_file() {
                if let Ok(raw) = std::fs::read_to_string(&legacy_json) {
                    if let Ok(mut legacy_data) = serde_json::from_str::<StoredDb>(&raw) {
                        let thumb_ids: Vec<i64> = legacy_data.thumbnails.keys().copied().collect();
                        for tid in thumb_ids {
                            let png_path = legacy_cache.join(format!("t_{tid}.png"));
                            if let Ok(bytes) = std::fs::read(&png_path) {
                                if !bytes.is_empty() {
                                    if let Ok((offset, len)) = db.append_blob(&bytes) {
                                        if let Some(t) = legacy_data.thumbnails.get_mut(&tid) {
                                            t.blob_offset = offset;
                                            t.blob_len = len;
                                            t.blob_fmt = "png".to_string();
                                        }
                                    }
                                }
                            }
                        }
                        migrated_legacy_count = legacy_data.thumbnails.len();
                        db.data = legacy_data;
                        db.rebuild_indexes();
                        if db.persist().is_ok() {
                            // Remove legacy folder & json now that all data is packed inside thumbcraft.tcdb
                            let _ = std::fs::remove_dir_all(&legacy_cache);
                            let _ = std::fs::remove_file(&legacy_json);
                        }
                    }
                }
            }
        }

        if migrated_legacy_count == 0 {
            let _ = db.persist();
        }
        db.migrated_legacy_count = migrated_legacy_count;
        Ok(db)
    }

    fn from_parts(db_path: PathBuf, file: File, write_cursor: u64, data: StoredDb) -> Self {
        let mut db = Self {
            db_path,
            file,
            write_cursor,
            data,
            path_index: HashMap::new(),
            thumb_index: HashMap::new(),
            keyword_index: HashMap::new(),
            thumb_kw_map: HashMap::new(),
            migrated_legacy_count: 0,
        };
        db.rebuild_indexes();
        db
    }

    fn rebuild_indexes(&mut self) {
        self.path_index.clear();
        for (&id, dir) in &self.data.paths {
            self.path_index.insert(dir.to_ascii_lowercase(), id);
        }
        self.thumb_index.clear();
        for (&id, t) in &self.data.thumbnails {
            self.thumb_index
                .insert((t.id_path, t.name.to_ascii_lowercase()), id);
        }
        self.keyword_index.clear();
        for (&id, kw) in &self.data.keywords {
            self.keyword_index.insert(kw.to_ascii_lowercase(), id);
        }
        self.thumb_kw_map.clear();
        for &(kid, tid) in &self.data.thumb_keywords {
            self.thumb_kw_map.entry(tid).or_default().push(kid);
        }
    }

    fn append_blob(&mut self, bytes: &[u8]) -> Result<(u64, u32), String> {
        let offset = self.write_cursor;
        self.file
            .seek(SeekFrom::Start(offset))
            .map_err(|e| format!("Seek blob failed: {e}"))?;
        self.file
            .write_all(bytes)
            .map_err(|e| format!("Write blob failed: {e}"))?;
        let len = bytes.len() as u32;
        self.write_cursor = offset + len as u64;
        Ok((offset, len))
    }

    fn read_blob(&self, offset: u64, len: u32) -> Option<Vec<u8>> {
        if offset < TCDB_HEADER_LEN || len == 0 {
            return None;
        }
        let mut f = self.file.try_clone().ok()?;
        f.seek(SeekFrom::Start(offset)).ok()?;
        let mut buf = vec![0u8; len as usize];
        f.read_exact(&mut buf).ok()?;
        Some(buf)
    }

    fn persist(&mut self) -> Result<(), String> {
        let json = serde_json::to_vec(&self.data).map_err(|e| e.to_string())?;
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
        encoder
            .write_all(&json)
            .map_err(|e| format!("Zlib index compress failed: {e}"))?;
        let compressed = encoder
            .finish()
            .map_err(|e| format!("Zlib finish failed: {e}"))?;

        let new_index_offset = self.write_cursor;
        self.file
            .seek(SeekFrom::Start(new_index_offset))
            .map_err(|e| format!("Seek index failed: {e}"))?;
        let comp_len = compressed.len() as u32;
        self.file
            .write_all(&comp_len.to_le_bytes())
            .map_err(|e| format!("Write index len failed: {e}"))?;
        self.file
            .write_all(&compressed)
            .map_err(|e| format!("Write index data failed: {e}"))?;
        self.file.flush().map_err(|e| format!("Flush failed: {e}"))?;

        // Atomically point header [8..16] to the new index snapshot
        self.file
            .seek(SeekFrom::Start(8))
            .map_err(|e| format!("Seek header failed: {e}"))?;
        self.file
            .write_all(&new_index_offset.to_le_bytes())
            .map_err(|e| format!("Write header index_offset failed: {e}"))?;
        self.file.flush().map_err(|e| format!("Header flush failed: {e}"))?;

        self.write_cursor = new_index_offset + 4 + comp_len as u64;
        Ok(())
    }

    pub fn flush(&mut self) -> Result<(), String> {
        self.persist()?;
        // Auto-compact if dead/superseded bytes exceed 40% and file > 4 MB
        let total_size = self.db_file_size();
        if total_size > 4 * 1024 * 1024 {
            let live_blobs: u64 = self
                .data
                .thumbnails
                .values()
                .map(|t| t.blob_len as u64)
                .sum();
            if total_size > live_blobs.saturating_mul(2) + 2 * 1024 * 1024 {
                let _ = self.compact_internal(false);
            }
        }
        Ok(())
    }

    /// Removes ONLY dangling/unused records:
    /// - Thumbnails whose files no longer exist on mounted drives (or whose folder record is missing),
    /// - Keyword links (`thumb_keywords`) pointing to non-existent thumbnails or keywords,
    /// - Keywords (`keywords`) that have zero thumbnails linked to them,
    /// - Gallery entries (`thumb_galleries`) pointing to non-existent thumbnails,
    /// - Folder records (`paths`) that have zero thumbnails linked to them.
    /// Live thumbnails and any keyword still linked to at least one live thumbnail are never removed.
    fn prune_dangling_records(&mut self, check_disk_orphans: bool) -> (usize, usize, usize) {
        let mut dead_thumb_ids: BTreeSet<i64> = BTreeSet::new();
        for (&tid, t) in &self.data.thumbnails {
            let Some(dir) = self.data.paths.get(&t.id_path) else {
                dead_thumb_ids.insert(tid);
                continue;
            };
            if check_disk_orphans {
                let dir_path = Path::new(dir);
                // Preserve entries on currently unplugged/unmounted drives; only prune when the
                // drive/root is mounted and the file no longer exists on disk.
                let root_mounted = dir_path
                    .ancestors()
                    .last()
                    .map(|root| root.exists())
                    .unwrap_or(true);
                if root_mounted {
                    let full = dir_path.join(&t.name);
                    if !full.is_file() {
                        dead_thumb_ids.insert(tid);
                    }
                }
            }
        }

        let removed_thumbs = dead_thumb_ids.len();
        for tid in &dead_thumb_ids {
            self.data.thumbnails.remove(tid);
        }

        // Remove dangling thumbnail<->keyword links (pointing to missing thumbnail or missing keyword)
        let live_thumbs = &self.data.thumbnails;
        let all_kws = &self.data.keywords;
        self.data
            .thumb_keywords
            .retain(|(kid, tid)| live_thumbs.contains_key(tid) && all_kws.contains_key(kid));

        // Remove ONLY keywords that no longer have any live thumbnail linked to them
        let used_kw_ids: BTreeSet<i64> = self
            .data
            .thumb_keywords
            .iter()
            .map(|(kid, _)| *kid)
            .collect();
        let kw_before = self.data.keywords.len();
        self.data.keywords.retain(|kid, _| used_kw_ids.contains(kid));
        let removed_keywords = kw_before.saturating_sub(self.data.keywords.len());

        // Remove dangling gallery links pointing to missing thumbnails
        for list in self.data.thumb_galleries.values_mut() {
            list.retain(|tid| live_thumbs.contains_key(tid));
        }

        // Remove ONLY folder paths that have zero live thumbnails linked to them
        let used_path_ids: BTreeSet<i64> = self
            .data
            .thumbnails
            .values()
            .map(|t| t.id_path)
            .collect();
        let paths_before = self.data.paths.len();
        self.data.paths.retain(|pid, _| used_path_ids.contains(pid));
        let removed_paths = paths_before.saturating_sub(self.data.paths.len());

        if removed_thumbs > 0 || removed_keywords > 0 || removed_paths > 0 {
            self.rebuild_indexes();
        }
        (removed_thumbs, removed_keywords, removed_paths)
    }

    /// Prunes dangling/unused thumbnails, keywords, and folders, then rewrites `thumbcraft.tcdb`
    /// cleanly with only live compressed blobs and a single compressed index.
    #[allow(dead_code)]
    pub fn compact(&mut self) -> Result<(u64, u64), String> {
        let rep = self.compact_with_report()?;
        Ok((rep.bytes_before, rep.bytes_after))
    }

    /// Full compaction with detailed counts of pruned dangling thumbnails, unused keywords, and paths.
    pub fn compact_with_report(&mut self) -> Result<CompactReport, String> {
        self.compact_internal(true)
    }

    fn compact_internal(&mut self, check_disk_orphans: bool) -> Result<CompactReport, String> {
        let before = self.db_file_size();
        let (removed_thumbs, removed_keywords, removed_paths) =
            self.prune_dangling_records(check_disk_orphans);
        let tmp_path = self.db_path.with_extension("tcdb.tmp");

        let mut out = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp_path)
            .map_err(|e| format!("Compact create failed: {e}"))?;

        out.write_all(TCDB_MAGIC).map_err(|e| e.to_string())?;
        out.write_all(&TCDB_HEADER_LEN.to_le_bytes())
            .map_err(|e| e.to_string())?;

        let mut cursor = TCDB_HEADER_LEN;
        let ids: Vec<i64> = self.data.thumbnails.keys().copied().collect();
        for tid in ids {
            let (old_off, old_len) = match self.data.thumbnails.get(&tid) {
                Some(t) => (t.blob_offset, t.blob_len),
                None => continue,
            };
            if let Some(blob) = self.read_blob(old_off, old_len) {
                out.write_all(&blob).map_err(|e| e.to_string())?;
                if let Some(t) = self.data.thumbnails.get_mut(&tid) {
                    t.blob_offset = cursor;
                    t.blob_len = blob.len() as u32;
                }
                cursor += blob.len() as u64;
            } else if let Some(t) = self.data.thumbnails.get_mut(&tid) {
                t.blob_offset = 0;
                t.blob_len = 0;
            }
        }

        // Write fresh compressed index
        let json = serde_json::to_vec(&self.data).map_err(|e| e.to_string())?;
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&json).map_err(|e| e.to_string())?;
        let compressed = encoder.finish().map_err(|e| e.to_string())?;

        let index_offset = cursor;
        let comp_len = compressed.len() as u32;
        out.write_all(&comp_len.to_le_bytes())
            .map_err(|e| e.to_string())?;
        out.write_all(&compressed).map_err(|e| e.to_string())?;
        out.seek(SeekFrom::Start(8)).map_err(|e| e.to_string())?;
        out.write_all(&index_offset.to_le_bytes())
            .map_err(|e| e.to_string())?;
        out.flush().map_err(|e| e.to_string())?;
        drop(out);

        // Close our open handle so Windows allows atomic replacement
        let placeholder = OpenOptions::new()
            .read(true)
            .open(&tmp_path)
            .map_err(|e| e.to_string())?;
        let old_file = std::mem::replace(&mut self.file, placeholder);
        drop(old_file);
        // Close placeholder too before rename
        let _ = std::fs::remove_file(&self.db_path);
        // On Windows we must not hold any open handle to tmp_path during rename either
        let reopened_final = {
            std::fs::copy(&tmp_path, &self.db_path)
                .map_err(|e| format!("Compact replace failed: {e}"))?;
            OpenOptions::new()
                .read(true)
                .write(true)
                .open(&self.db_path)
                .map_err(|e| format!("Reopen compacted db failed: {e}"))?
        };
        let tmp_handle = std::mem::replace(&mut self.file, reopened_final);
        drop(tmp_handle);
        let _ = std::fs::remove_file(&tmp_path);

        let after = self.db_file_size();
        self.write_cursor = after.max(TCDB_HEADER_LEN);
        Ok(CompactReport {
            bytes_before: before,
            bytes_after: after,
            removed_thumbs,
            removed_keywords,
            removed_paths,
        })
    }

    pub fn db_file_size(&self) -> u64 {
        std::fs::metadata(&self.db_path).map(|m| m.len()).unwrap_or(0)
    }

    pub fn path(&self) -> &Path {
        &self.db_path
    }

    fn split_path(full_path: &Path) -> (String, String) {
        let dir = full_path
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let name = full_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        (dir, name)
    }

    fn find_path_id(&self, dir: &str) -> Option<i64> {
        self.path_index.get(&dir.to_ascii_lowercase()).copied()
    }

    fn get_or_create_path_id(&mut self, dir: &str) -> i64 {
        let key = dir.to_ascii_lowercase();
        if let Some(&id) = self.path_index.get(&key) {
            return id;
        }
        let id = self.data.next_path_id.max(1);
        self.data.next_path_id = id + 1;
        let clipped: String = dir.chars().take(400).collect();
        self.path_index.insert(clipped.to_ascii_lowercase(), id);
        self.data.paths.insert(id, clipped);
        id
    }

    fn find_thumb_id(&self, full_path: &Path) -> Option<i64> {
        let (dir, name) = Self::split_path(full_path);
        let id_path = self.find_path_id(&dir)?;
        self.thumb_index
            .get(&(id_path, name.to_ascii_lowercase()))
            .copied()
    }

    fn get_or_create_keyword_id(&mut self, kw: &str) -> Option<i64> {
        let clean: String = kw.trim().chars().take(255).collect();
        if clean.is_empty() {
            return None;
        }
        let key = clean.to_ascii_lowercase();
        if let Some(&id) = self.keyword_index.get(&key) {
            return Some(id);
        }
        let id = self.data.next_keyword_id.max(1);
        self.data.next_keyword_id = id + 1;
        self.keyword_index.insert(key, id);
        self.data.keywords.insert(id, clean);
        Some(id)
    }

    pub fn get_by_path(&self, full_path: &Path) -> Result<Option<CatalogRecord>, String> {
        let Some(id_thumb) = self.find_thumb_id(full_path) else {
            return Ok(None);
        };
        let Some(t) = self.data.thumbnails.get(&id_thumb) else {
            return Ok(None);
        };
        let dir = self.data.paths.get(&t.id_path).cloned().unwrap_or_default();
        let thumb_blob = self.read_blob(t.blob_offset, t.blob_len);
        let thumb_fmt = if t.blob_fmt.is_empty() {
            "png".to_string()
        } else {
            t.blob_fmt.clone()
        };
        let keywords = self.get_keywords_for_thumb(id_thumb)?;
        Ok(Some(CatalogRecord {
            id_thumb,
            full_path: PathBuf::from(dir).join(&t.name),
            file_name: t.name.clone(),
            filetype: t.filetype.clone(),
            file_time_iso: t.file_time_iso.clone(),
            file_length: t.filelength,
            width: t.width,
            height: t.height,
            color_depth: t.colordepth,
            page_count: t.page_count,
            rating: t.rating,
            annotation: t.annotation.clone(),
            keywords,
            metadata: t.metadata.clone(),
            thumb_blob,
            thumb_fmt,
            md5: t.md5.clone(),
            visual_sig: t.visual_sig.clone(),
        }))
    }

    /// Fast cache check (metadata only, without reading blob bytes from disk)
    pub fn is_up_to_date(&self, full_path: &Path, file_length: u64, file_time_iso: &str) -> bool {
        let Some(id_thumb) = self.find_thumb_id(full_path) else {
            return false;
        };
        self.data
            .thumbnails
            .get(&id_thumb)
            .map(|t| t.filelength == file_length && t.file_time_iso == file_time_iso && t.blob_len > 0)
            .unwrap_or(false)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn upsert_thumbnail_batch(
        &mut self,
        full_path: &Path,
        filetype: &str,
        file_time_iso: &str,
        file_length: u64,
        width: u32,
        height: u32,
        color_depth: u16,
        page_count: u32,
        extracted_keywords: &[String],
        metadata: &BTreeMap<String, String>,
        thumb_blob: Option<(&[u8], &str)>,
        md5: &str,
        visual_sig: Option<VisualSignature>,
    ) -> Result<i64, String> {
        let (dir, name) = Self::split_path(full_path);
        let id_path = self.get_or_create_path_id(&dir);
        let now_iso = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();

        let blob_loc = if let Some((bytes, fmt)) = thumb_blob {
            if !bytes.is_empty() {
                let (off, len) = self.append_blob(bytes)?;
                Some((off, len, fmt.to_string()))
            } else {
                None
            }
        } else {
            None
        };

        let id_thumb = if let Some(existing_id) = self.find_thumb_id(full_path) {
            if let Some(t) = self.data.thumbnails.get_mut(&existing_id) {
                t.filetype = filetype.to_string();
                t.file_time_iso = file_time_iso.to_string();
                t.thumbnail_time_iso = now_iso;
                t.filelength = file_length;
                t.width = width;
                t.height = height;
                t.colordepth = color_depth;
                t.page_count = page_count.max(1);
                t.metadata = metadata.clone();
                if !md5.is_empty() {
                    t.md5 = md5.to_string();
                }
                if visual_sig.is_some() {
                    t.visual_sig = visual_sig;
                }
                if let Some((off, len, fmt)) = blob_loc {
                    t.blob_offset = off;
                    t.blob_len = len;
                    t.blob_fmt = fmt;
                }
            }
            existing_id
        } else {
            let id = self.data.next_thumb_id.max(1);
            self.data.next_thumb_id = id + 1;
            let (blob_offset, blob_len, blob_fmt) = blob_loc.unwrap_or((0, 0, String::new()));
            self.thumb_index
                .insert((id_path, name.to_ascii_lowercase()), id);
            self.data.thumbnails.insert(
                id,
                StoredThumb {
                    id_thumb: id,
                    id_path,
                    name,
                    filetype: filetype.to_string(),
                    file_time_iso: file_time_iso.to_string(),
                    thumbnail_time_iso: now_iso,
                    filelength: file_length,
                    width,
                    height,
                    colordepth: color_depth,
                    page_count: page_count.max(1),
                    rating: 0,
                    annotation: String::new(),
                    metadata: metadata.clone(),
                    md5: md5.to_string(),
                    visual_sig,
                    blob_offset,
                    blob_len,
                    blob_fmt,
                },
            );
            id
        };

        for kw in extracted_keywords {
            if let Some(id_kw) = self.get_or_create_keyword_id(kw) {
                if self.data.thumb_keywords.insert((id_kw, id_thumb)) {
                    self.thumb_kw_map.entry(id_thumb).or_default().push(id_kw);
                }
            }
        }

        Ok(id_thumb)
    }

    pub fn get_keywords_for_thumb(&self, id_thumb: i64) -> Result<Vec<String>, String> {
        let mut out: Vec<String> = self
            .thumb_kw_map
            .get(&id_thumb)
            .map(|kids| {
                kids.iter()
                    .filter_map(|kid| self.data.keywords.get(kid).cloned())
                    .collect()
            })
            .unwrap_or_default();
        out.sort_by_key(|a| a.to_ascii_lowercase());
        Ok(out)
    }

    #[allow(dead_code)]
    pub fn add_keyword_to_path(&mut self, full_path: &Path, keyword: &str) -> Result<(), String> {
        self.add_keywords_to_path(full_path, &[keyword.to_string()])
    }

    pub fn add_keywords_to_path(
        &mut self,
        full_path: &Path,
        keywords: &[String],
    ) -> Result<(), String> {
        let Some(id_thumb) = self.find_thumb_id(full_path) else {
            return Ok(());
        };
        let mut changed = false;
        for keyword in keywords {
            if let Some(id_kw) = self.get_or_create_keyword_id(keyword) {
                if self.data.thumb_keywords.insert((id_kw, id_thumb)) {
                    self.thumb_kw_map.entry(id_thumb).or_default().push(id_kw);
                    changed = true;
                }
            }
        }
        if changed {
            self.persist()?;
        }
        Ok(())
    }

    /// Updates cached file size, modified timestamp, and MD5 after writing internal metadata
    /// (such as Windows Explorer keywords) to the file so the thumbnail cache stays valid.
    pub fn update_file_stat(
        &mut self,
        full_path: &Path,
        file_length: u64,
        file_time_iso: &str,
        md5: &str,
    ) -> Result<(), String> {
        if let Some(id_thumb) = self.find_thumb_id(full_path) {
            if let Some(t) = self.data.thumbnails.get_mut(&id_thumb) {
                t.filelength = file_length;
                t.file_time_iso = file_time_iso.to_string();
                t.md5 = md5.to_string();
                self.persist()?;
            }
        }
        Ok(())
    }

    pub fn remove_keyword_from_path(
        &mut self,
        full_path: &Path,
        keyword: &str,
    ) -> Result<(), String> {
        let Some(id_thumb) = self.find_thumb_id(full_path) else {
            return Ok(());
        };
        let target_key = keyword.trim().to_ascii_lowercase();
        if let Some(&kid) = self.keyword_index.get(&target_key) {
            self.data.thumb_keywords.remove(&(kid, id_thumb));
            if let Some(list) = self.thumb_kw_map.get_mut(&id_thumb) {
                list.retain(|&k| k != kid);
            }
            let still_used = self.data.thumb_keywords.iter().any(|(k, _)| *k == kid);
            if !still_used {
                self.data.keywords.remove(&kid);
                self.keyword_index.remove(&target_key);
            }
            self.persist()?;
        }
        Ok(())
    }

    pub fn set_rating(&mut self, full_path: &Path, rating: u8) -> Result<(), String> {
        if let Some(id_thumb) = self.find_thumb_id(full_path) {
            if let Some(t) = self.data.thumbnails.get_mut(&id_thumb) {
                t.rating = rating.min(5);
                self.persist()?;
            }
        }
        Ok(())
    }

    pub fn set_annotation(&mut self, full_path: &Path, annotation: &str) -> Result<(), String> {
        if let Some(id_thumb) = self.find_thumb_id(full_path) {
            if let Some(t) = self.data.thumbnails.get_mut(&id_thumb) {
                t.annotation = annotation.chars().take(4095).collect();
                self.persist()?;
            }
        }
        Ok(())
    }

    pub fn list_all_keywords(&self) -> Result<Vec<(String, usize)>, String> {
        let mut counts: BTreeMap<i64, usize> = BTreeMap::new();
        for (kid, _) in &self.data.thumb_keywords {
            *counts.entry(*kid).or_insert(0) += 1;
        }
        let mut out: Vec<(String, usize)> = counts
            .into_iter()
            .filter_map(|(kid, cnt)| self.data.keywords.get(&kid).map(|k| (k.clone(), cnt)))
            .collect();
        out.sort_by(|a, b| {
            b.1.cmp(&a.1)
                .then_with(|| a.0.to_ascii_lowercase().cmp(&b.0.to_ascii_lowercase()))
        });
        Ok(out)
    }

    pub fn find_paths_by_keyword(&self, keyword: &str) -> Result<Vec<PathBuf>, String> {
        let Some(&kid) = self.keyword_index.get(&keyword.to_ascii_lowercase()) else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        for &(k, tid) in &self.data.thumb_keywords {
            if k == kid {
                if let Some(t) = self.data.thumbnails.get(&tid) {
                    if let Some(dir) = self.data.paths.get(&t.id_path) {
                        out.push(PathBuf::from(dir).join(&t.name));
                    }
                }
            }
        }
        out.sort();
        Ok(out)
    }

    pub fn list_galleries(&self) -> Result<Vec<(String, usize)>, String> {
        let mut out: Vec<(String, usize)> = self
            .data
            .galleries
            .iter()
            .map(|(gid, name)| {
                let cnt = self
                    .data
                    .thumb_galleries
                    .get(gid)
                    .map(|v| v.len())
                    .unwrap_or(0);
                (name.clone(), cnt)
            })
            .collect();
        out.sort_by_key(|a| a.0.to_ascii_lowercase());
        Ok(out)
    }

    pub fn create_gallery(&mut self, name: &str) -> Result<(), String> {
        let clean = name.trim();
        if clean.is_empty() {
            return Ok(());
        }
        if self
            .data
            .galleries
            .values()
            .any(|g| g.eq_ignore_ascii_case(clean))
        {
            return Ok(());
        }
        let id = self.data.next_gallery_id.max(1);
        self.data.next_gallery_id = id + 1;
        self.data.galleries.insert(id, clean.to_string());
        self.data.thumb_galleries.entry(id).or_default();
        self.persist()
    }

    pub fn delete_gallery(&mut self, name: &str) -> Result<(), String> {
        let ids: Vec<i64> = self
            .data
            .galleries
            .iter()
            .filter(|(_, g)| g.eq_ignore_ascii_case(name))
            .map(|(&id, _)| id)
            .collect();
        for id in ids {
            self.data.galleries.remove(&id);
            self.data.thumb_galleries.remove(&id);
        }
        self.persist()
    }

    pub fn add_path_to_gallery(
        &mut self,
        gallery_name: &str,
        full_path: &Path,
    ) -> Result<(), String> {
        self.create_gallery(gallery_name)?;
        let Some(id_thumb) = self.find_thumb_id(full_path) else {
            return Ok(());
        };
        let Some((&gid, _)) = self
            .data
            .galleries
            .iter()
            .find(|(_, g)| g.eq_ignore_ascii_case(gallery_name))
        else {
            return Ok(());
        };
        let list = self.data.thumb_galleries.entry(gid).or_default();
        if !list.contains(&id_thumb) {
            list.push(id_thumb);
            self.persist()?;
        }
        Ok(())
    }

    pub fn get_gallery_paths(&self, gallery_name: &str) -> Result<Vec<PathBuf>, String> {
        let Some((&gid, _)) = self
            .data
            .galleries
            .iter()
            .find(|(_, g)| g.eq_ignore_ascii_case(gallery_name))
        else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        if let Some(ids) = self.data.thumb_galleries.get(&gid) {
            for &tid in ids {
                if let Some(t) = self.data.thumbnails.get(&tid) {
                    if let Some(dir) = self.data.paths.get(&t.id_path) {
                        out.push(PathBuf::from(dir).join(&t.name));
                    }
                }
            }
        }
        Ok(out)
    }

    #[allow(dead_code)]
    pub fn remove_path(&mut self, full_path: &Path) -> Result<(), String> {
        let _ = self.remove_paths(&[full_path.to_path_buf()])?;
        Ok(())
    }

    pub fn remove_paths(&mut self, paths: &[PathBuf]) -> Result<usize, String> {
        let mut removed_ids = BTreeSet::new();
        for full_path in paths {
            if let Some(id_thumb) = self.find_thumb_id(full_path) {
                if let Some(t) = self.data.thumbnails.remove(&id_thumb) {
                    self.thumb_index
                        .remove(&(t.id_path, t.name.to_ascii_lowercase()));
                }
                self.thumb_kw_map.remove(&id_thumb);
                removed_ids.insert(id_thumb);
            }
        }
        if removed_ids.is_empty() {
            return Ok(0);
        }
        self.data
            .thumb_keywords
            .retain(|(_, tid)| !removed_ids.contains(tid));
        for list in self.data.thumb_galleries.values_mut() {
            list.retain(|tid| !removed_ids.contains(tid));
        }
        self.persist()?;
        Ok(removed_ids.len())
    }

    pub fn rename_path(&mut self, old_path: &Path, new_path: &Path) -> Result<(), String> {
        self.rename_paths(&[(old_path.to_path_buf(), new_path.to_path_buf())])
    }

    pub fn rename_paths(&mut self, pairs: &[(PathBuf, PathBuf)]) -> Result<(), String> {
        let mut changed = false;
        for (old_path, new_path) in pairs {
            let Some(id_thumb) = self.find_thumb_id(old_path) else {
                continue;
            };
            let (new_dir, new_name) = Self::split_path(new_path);
            let new_path_id = self.get_or_create_path_id(&new_dir);
            if let Some(t) = self.data.thumbnails.get_mut(&id_thumb) {
                self.thumb_index
                    .remove(&(t.id_path, t.name.to_ascii_lowercase()));
                t.id_path = new_path_id;
                t.name = new_name.clone();
                self.thumb_index
                    .insert((new_path_id, new_name.to_ascii_lowercase()), id_thumb);
                changed = true;
            }
        }
        if changed {
            self.persist()?;
        }
        Ok(())
    }

    /// Marks one or more folders as fully scanned and persists to `thumbcraft.tcdb`.
    pub fn mark_folders_scanned(&mut self, dirs: &[PathBuf]) -> Result<(), String> {
        let mut changed = false;
        for d in dirs {
            let key = normalize_dir_lower(d);
            if !key.is_empty() && self.data.scanned_folders.insert(key) {
                changed = true;
            }
        }
        if changed {
            self.persist()?;
        }
        Ok(())
    }

    /// Returns the normalized set of fully scanned folder paths for fast O(1) tree badge lookups.
    pub fn scanned_folders_set(&self) -> std::collections::HashSet<String> {
        if !self.data.scanned_folders.is_empty() {
            return self.data.scanned_folders.iter().cloned().collect();
        }
        // Backwards compatibility for databases populated before `scanned_folders` existed:
        // treat previously cataloged folders as scanned until explicitly removed or rescanned.
        self.data
            .paths
            .values()
            .map(|p| normalize_dir_lower(Path::new(p)))
            .filter(|k| !k.is_empty())
            .collect()
    }

    /// Removes a folder and all its subfolders from the catalog database (does NOT delete files on disk).
    pub fn remove_folder_tree(&mut self, dir: &Path) -> Result<usize, String> {
        let prefix = normalize_dir_lower(dir);
        let path_ids: Vec<i64> = self
            .data
            .paths
            .iter()
            .filter(|(_, p)| is_same_or_child_dir(p, &prefix))
            .map(|(&id, _)| id)
            .collect();

        if path_ids.is_empty() {
            return Ok(0);
        }

        let path_set: BTreeSet<i64> = path_ids.iter().copied().collect();
        let thumb_ids: Vec<i64> = self
            .data
            .thumbnails
            .iter()
            .filter(|(_, t)| path_set.contains(&t.id_path))
            .map(|(&id, _)| id)
            .collect();
        let thumb_set: BTreeSet<i64> = thumb_ids.iter().copied().collect();

        for pid in &path_ids {
            self.data.paths.remove(pid);
        }
        for tid in &thumb_ids {
            self.data.thumbnails.remove(tid);
        }
        self.data
            .thumb_keywords
            .retain(|(_, tid)| !thumb_set.contains(tid));
        for list in self.data.thumb_galleries.values_mut() {
            list.retain(|tid| !thumb_set.contains(tid));
        }
        self.data
            .scanned_folders
            .retain(|p| !is_same_or_child_dir(p, &prefix));

        self.rebuild_indexes();
        self.persist()?;
        Ok(thumb_ids.len())
    }

    /// Updates catalog paths after renaming or moving a folder on disk.
    pub fn rename_folder_tree(&mut self, old_dir: &Path, new_dir: &Path) -> Result<usize, String> {
        let old_str = old_dir.to_string_lossy().to_string();
        let new_str = new_dir.to_string_lossy().to_string();
        let old_low = normalize_dir_lower(old_dir);
        let mut updated = 0usize;

        for p in self.data.paths.values_mut() {
            if is_same_or_child_dir(p, &old_low) {
                let p_trimmed = p.trim_end_matches(['\\', '/']);
                let old_trimmed = old_str.trim_end_matches(['\\', '/']);
                if p_trimmed.len() >= old_trimmed.len() {
                    let suffix = &p_trimmed[old_trimmed.len()..];
                    *p = format!("{}{}", new_str.trim_end_matches(['\\', '/']), suffix);
                    updated += 1;
                }
            }
        }
        let old_scanned: Vec<String> = self.data.scanned_folders.iter().cloned().collect();
        let mut new_scanned = BTreeSet::new();
        for p in old_scanned {
            if is_same_or_child_dir(&p, &old_low) {
                let p_trimmed = p.trim_end_matches(['\\', '/']);
                let old_trimmed = old_low.trim_end_matches(['\\', '/']);
                if p_trimmed.len() >= old_trimmed.len() {
                    let suffix = &p_trimmed[old_trimmed.len()..];
                    let renamed = format!(
                        "{}{}",
                        new_str.trim_end_matches(['\\', '/']).to_ascii_lowercase(),
                        suffix
                    );
                    new_scanned.insert(renamed);
                }
            } else {
                new_scanned.insert(p);
            }
        }
        self.data.scanned_folders = new_scanned;

        if updated > 0 {
            self.rebuild_indexes();
            self.persist()?;
        }
        Ok(updated)
    }

    /// Removes catalog entries whose files no longer exist on disk (optionally scoped to `scope_dir`).
    pub fn remove_orphans(&mut self, scope_dir: Option<&Path>) -> Result<usize, String> {
        let prefix = scope_dir.map(normalize_dir_lower);
        let orphan_ids: Vec<i64> = self
            .data
            .thumbnails
            .iter()
            .filter_map(|(&tid, t)| {
                let dir = self.data.paths.get(&t.id_path)?;
                if let Some(ref pref) = prefix {
                    if !is_same_or_child_dir(dir, pref) {
                        return None;
                    }
                }
                let full = PathBuf::from(dir).join(&t.name);
                if !full.is_file() {
                    Some(tid)
                } else {
                    None
                }
            })
            .collect();

        if orphan_ids.is_empty() {
            return Ok(0);
        }

        let orphan_set: BTreeSet<i64> = orphan_ids.iter().copied().collect();
        for tid in &orphan_ids {
            self.data.thumbnails.remove(tid);
        }
        self.data
            .thumb_keywords
            .retain(|(_, tid)| !orphan_set.contains(tid));
        for list in self.data.thumb_galleries.values_mut() {
            list.retain(|tid| !orphan_set.contains(tid));
        }

        // Remove empty paths & unreferenced keywords with no remaining thumbnails
        let used_kw_ids: BTreeSet<i64> = self
            .data
            .thumb_keywords
            .iter()
            .map(|(kid, _)| *kid)
            .collect();
        self.data.keywords.retain(|kid, _| used_kw_ids.contains(kid));
        let used_paths: BTreeSet<i64> = self.data.thumbnails.values().map(|t| t.id_path).collect();
        self.data.paths.retain(|pid, _| used_paths.contains(pid));

        self.rebuild_indexes();
        self.persist()?;
        Ok(orphan_ids.len())
    }

    pub fn folder_catalog_count(&self, dir: &Path) -> usize {
        let prefix = normalize_dir_lower(dir);
        let path_set: BTreeSet<i64> = self
            .data
            .paths
            .iter()
            .filter(|(_, p)| is_same_or_child_dir(p, &prefix))
            .map(|(&id, _)| id)
            .collect();
        self.data
            .thumbnails
            .values()
            .filter(|t| path_set.contains(&t.id_path))
            .count()
    }

    /// Fast O(N) in-memory lookup of cached `VisualSignature` entries for a list of paths (no disk I/O).
    pub fn get_visual_signatures_for_paths(&self, paths: &[&Path]) -> Vec<Option<VisualSignature>> {
        paths
            .iter()
            .map(|p| {
                let id = self.find_thumb_id(p)?;
                self.data
                    .thumbnails
                    .get(&id)
                    .and_then(|t| t.visual_sig.as_ref())
                    .filter(|s| s.is_valid())
                    .cloned()
            })
            .collect()
    }

    /// Backfills missing visual signatures from stored thumbnail blobs and searches the catalog
    /// (scoped to Entire Database, Current Tree, or Current Folder) for visually similar images.
    pub fn find_similar_to(
        &mut self,
        target_path: &Path,
        min_similarity: f32,
        color_weight: f32,
        scope: Option<(&Path, bool)>,
        only_filetype: Option<&str>,
    ) -> Result<Vec<SimilarMatch>, String> {
        let scope_prefix = scope.map(|(d, rec)| (normalize_dir_lower(d), rec));
        let ext_filter = only_filetype.map(|s| s.to_ascii_uppercase());

        let mut dirty = false;
        let thumb_ids: Vec<i64> = self.data.thumbnails.keys().copied().collect();
        for tid in thumb_ids {
            let blob_info = self.data.thumbnails.get(&tid).and_then(|t| {
                let dir = self.data.paths.get(&t.id_path)?;
                if let Some((ref pref, recursive)) = scope_prefix {
                    if !matches_scope_dir(dir, pref, recursive) {
                        return None;
                    }
                }
                let needs = t.visual_sig.as_ref().map(|s| !s.is_valid()).unwrap_or(true);
                if needs && t.blob_len > 0 {
                    Some((t.blob_offset, t.blob_len, t.blob_fmt.clone()))
                } else {
                    None
                }
            });
            if let Some((off, len, fmt)) = blob_info {
                if let Some(bytes) = self.read_blob(off, len) {
                    if let Some(img) = decoder::decode_thumbnail_blob(&bytes, &fmt) {
                        let sig = VisualSignature::from_image(&img);
                        if let Some(t) = self.data.thumbnails.get_mut(&tid) {
                            t.visual_sig = Some(sig);
                            dirty = true;
                        }
                    }
                }
            }
        }
        // Also ensure target_path itself has a signature even if outside scope
        if let Some(tid) = self.find_thumb_id(target_path) {
            let needs_target = self
                .data
                .thumbnails
                .get(&tid)
                .and_then(|t| {
                    if t.visual_sig.as_ref().map(|s| !s.is_valid()).unwrap_or(true) && t.blob_len > 0 {
                        Some((t.blob_offset, t.blob_len, t.blob_fmt.clone()))
                    } else {
                        None
                    }
                });
            if let Some((off, len, fmt)) = needs_target {
                if let Some(bytes) = self.read_blob(off, len) {
                    if let Some(img) = decoder::decode_thumbnail_blob(&bytes, &fmt) {
                        if let Some(t) = self.data.thumbnails.get_mut(&tid) {
                            t.visual_sig = Some(VisualSignature::from_image(&img));
                            dirty = true;
                        }
                    }
                }
            }
        }
        if dirty {
            let _ = self.persist();
        }

        let target_id = self.find_thumb_id(target_path);
        let target_sig = target_id
            .and_then(|id| self.data.thumbnails.get(&id))
            .and_then(|t| t.visual_sig.clone())
            .ok_or_else(|| "Selected file has no indexed thumbnail signature yet.".to_string())?;

        let mut matches = Vec::new();
        for (&tid, t) in &self.data.thumbnails {
            let Some(ref sig) = t.visual_sig else {
                continue;
            };
            let Some(dir) = self.data.paths.get(&t.id_path) else {
                continue;
            };
            let is_target = Some(tid) == target_id;
            if !is_target {
                if let Some((ref pref, recursive)) = scope_prefix {
                    if !matches_scope_dir(dir, pref, recursive) {
                        continue;
                    }
                }
                if let Some(ref ext) = ext_filter {
                    if !t.filetype.eq_ignore_ascii_case(ext) {
                        continue;
                    }
                }
            }
            let p = PathBuf::from(dir).join(&t.name);
            if !p.is_file() {
                continue;
            }
            let sim = if is_target {
                1.0
            } else {
                similarity::similarity_score(&target_sig, sig, color_weight)
            };
            if sim >= min_similarity || is_target {
                matches.push(SimilarMatch {
                    path: p,
                    similarity: sim,
                });
            }
        }

        matches.sort_by(|a, b| {
            b.similarity
                .partial_cmp(&a.similarity)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(matches)
    }

    /// Scans MD5 content hashes across the catalog database (Entire Database, Current Tree, or Current Folder)
    /// and returns all duplicate groups (2+ existing files with identical MD5).
    pub fn find_hash_duplicates(
        &mut self,
        scope: Option<(&Path, bool)>,
    ) -> Result<(Vec<DuplicateGroup>, usize), String> {
        let scope_prefix = scope.map(|(d, rec)| (normalize_dir_lower(d), rec));
        let mut dirty = false;
        let mut scanned = 0usize;
        let thumb_ids: Vec<i64> = self.data.thumbnails.keys().copied().collect();

        for tid in thumb_ids {
            let (full_path, needs_hash) = match self.data.thumbnails.get(&tid) {
                Some(t) => {
                    let dir = self.data.paths.get(&t.id_path).cloned().unwrap_or_default();
                    if let Some((ref pref, recursive)) = scope_prefix {
                        if !matches_scope_dir(&dir, pref, recursive) {
                            continue;
                        }
                    }
                    (PathBuf::from(dir).join(&t.name), t.md5.is_empty())
                }
                None => continue,
            };
            if !full_path.is_file() {
                continue;
            }
            scanned += 1;
            if needs_hash {
                if let Ok(hash) = similarity::md5_file(&full_path) {
                    if let Some(t) = self.data.thumbnails.get_mut(&tid) {
                        t.md5 = hash;
                        dirty = true;
                    }
                }
            }
        }
        if dirty {
            let _ = self.persist();
        }

        let mut by_hash: BTreeMap<String, (u64, Vec<PathBuf>)> = BTreeMap::new();
        for t in self.data.thumbnails.values() {
            if t.md5.is_empty() {
                continue;
            }
            let Some(dir) = self.data.paths.get(&t.id_path) else {
                continue;
            };
            if let Some((ref pref, recursive)) = scope_prefix {
                if !matches_scope_dir(dir, pref, recursive) {
                    continue;
                }
            }
            let p = PathBuf::from(dir).join(&t.name);
            if !p.is_file() {
                continue;
            }
            let entry = by_hash
                .entry(t.md5.clone())
                .or_insert_with(|| (t.filelength, Vec::new()));
            entry.1.push(p);
        }

        let mut groups = Vec::new();
        let mut group_idx = 1usize;
        for (md5, (size, mut paths)) in by_hash {
            if paths.len() > 1 {
                paths.sort();
                groups.push(DuplicateGroup {
                    group_index: group_idx,
                    md5,
                    file_size: size,
                    paths,
                });
                group_idx += 1;
            }
        }
        groups.sort_by(|a, b| {
            (b.file_size * b.paths.len() as u64).cmp(&(a.file_size * a.paths.len() as u64))
        });
        for (i, g) in groups.iter_mut().enumerate() {
            g.group_index = i + 1;
        }
        Ok((groups, scanned))
    }

    pub fn stats(&self) -> CatalogStats {
        let used_kws: BTreeSet<i64> = self.data.thumb_keywords.iter().map(|(k, _)| *k).collect();
        CatalogStats {
            total_thumbnails: self.data.thumbnails.len(),
            total_keywords: used_kws.len(),
            total_folders: self.data.paths.len(),
            total_galleries: self.data.galleries.len(),
            db_size_bytes: self.db_file_size(),
        }
    }
}

fn new_empty_db() -> StoredDb {
    StoredDb {
        next_path_id: 1,
        next_thumb_id: 1,
        next_keyword_id: 1,
        next_gallery_id: 1,
        ..Default::default()
    }
}

fn read_index_at(file: &mut File, index_offset: u64) -> Result<StoredDb, String> {
    file.seek(SeekFrom::Start(index_offset))
        .map_err(|e| e.to_string())?;
    let mut len_buf = [0u8; 4];
    file.read_exact(&mut len_buf).map_err(|e| e.to_string())?;
    let comp_len = u32::from_le_bytes(len_buf) as usize;
    let mut comp = vec![0u8; comp_len];
    file.read_exact(&mut comp).map_err(|e| e.to_string())?;
    let mut dec = ZlibDecoder::new(&comp[..]);
    let mut json = Vec::new();
    dec.read_to_end(&mut json).map_err(|e| e.to_string())?;
    serde_json::from_slice::<StoredDb>(&json).map_err(|e| e.to_string())
}

pub fn normalize_dir_key(dir: &Path) -> String {
    normalize_dir_lower(dir)
}

fn normalize_dir_lower(dir: &Path) -> String {
    dir.to_string_lossy()
        .trim_end_matches(['\\', '/'])
        .to_ascii_lowercase()
}

fn is_same_or_child_dir(candidate: &str, prefix_lower: &str) -> bool {
    let c = candidate
        .trim_end_matches(['\\', '/'])
        .to_ascii_lowercase();
    if c == prefix_lower {
        return true;
    }
    if let Some(rest) = c.strip_prefix(prefix_lower) {
        rest.starts_with('\\') || rest.starts_with('/')
    } else {
        false
    }
}

fn matches_scope_dir(candidate: &str, prefix_lower: &str, recursive: bool) -> bool {
    if recursive {
        is_same_or_child_dir(candidate, prefix_lower)
    } else {
        candidate
            .trim_end_matches(['\\', '/'])
            .to_ascii_lowercase()
            == prefix_lower
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tcdb_roundtrip_and_compact() {
        let dir = std::env::temp_dir().join(format!("tcdb_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("thumbcraft.tcdb");
        let sample_file = dir.join("sample.cdr");
        std::fs::write(&sample_file, b"live cdr file").unwrap();

        {
            let mut db = CatalogDb::open(&db_path).unwrap();
            let blob = vec![0x52, 0x49, 0x46, 0x46, 1, 2, 3, 4, 5, 6];
            db.upsert_thumbnail_batch(
                &sample_file,
                "CDR",
                "2026-01-01 00:00:00",
                1234,
                200,
                150,
                24,
                1,
                &["vector".to_string(), "badge".to_string()],
                &BTreeMap::new(),
                Some((&blob, "webp")),
                "abcdef0123456789abcdef0123456789",
                None,
            )
            .unwrap();
            db.flush().unwrap();
        }

        // Reopen and verify blob + keywords + compaction
        {
            let mut db = CatalogDb::open(&db_path).unwrap();
            let rec = db.get_by_path(&sample_file).unwrap().expect("record");
            assert_eq!(rec.thumb_fmt, "webp");
            assert_eq!(rec.thumb_blob.as_deref(), Some(&[0x52, 0x49, 0x46, 0x46, 1, 2, 3, 4, 5, 6][..]));
            assert_eq!(rec.keywords, vec!["badge".to_string(), "vector".to_string()]);
            let (before, after) = db.compact().unwrap();
            assert!(before > 0 && after > 0);
            let rec2 = db.get_by_path(&sample_file).unwrap().expect("record after compact");
            assert_eq!(rec2.thumb_blob, rec.thumb_blob);
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn compact_removes_only_dangling_thumbnails_and_unused_keywords() {
        let dir = std::env::temp_dir().join(format!("tcdb_compact_dangling_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let sub_live = dir.join("live");
        let sub_dead = dir.join("dead");
        std::fs::create_dir_all(&sub_live).unwrap();
        std::fs::create_dir_all(&sub_dead).unwrap();

        let keep_file = sub_live.join("keep.png");
        let deleted_file = sub_dead.join("deleted.png");
        std::fs::write(&keep_file, b"keep").unwrap();
        std::fs::write(&deleted_file, b"delete me").unwrap();

        let db_path = dir.join("thumbcraft.tcdb");
        let mut db = CatalogDb::open(&db_path).unwrap();
        let blob_keep = vec![1u8, 2, 3, 4, 5, 6, 7, 8];
        let blob_del = vec![9u8; 4096];

        db.upsert_thumbnail_batch(
            &keep_file,
            "PNG",
            "2026-01-01 00:00:00",
            4,
            100,
            100,
            24,
            1,
            &["shared".to_string(), "keep_only".to_string()],
            &BTreeMap::new(),
            Some((&blob_keep, "webp")),
            "11111111111111111111111111111111",
            None,
        )
        .unwrap();
        db.upsert_thumbnail_batch(
            &deleted_file,
            "PNG",
            "2026-01-01 00:00:00",
            9,
            100,
            100,
            24,
            1,
            &["shared".to_string(), "orphan_only".to_string()],
            &BTreeMap::new(),
            Some((&blob_del, "webp")),
            "22222222222222222222222222222222",
            None,
        )
        .unwrap();
        db.create_gallery("TestGal").unwrap();
        db.add_path_to_gallery("TestGal", &keep_file).unwrap();
        db.add_path_to_gallery("TestGal", &deleted_file).unwrap();
        db.flush().unwrap();

        // Simulate low-latency delete: remove file from disk WITHOUT touching the database
        std::fs::remove_file(&deleted_file).unwrap();

        let rep = db.compact_with_report().unwrap();
        assert_eq!(rep.removed_thumbs, 1, "should remove only the 1 dangling thumbnail");
        assert_eq!(rep.removed_keywords, 1, "should remove only 'orphan_only' keyword");
        assert_eq!(rep.removed_paths, 1, "should remove only the empty 'dead' folder record");
        assert!(rep.bytes_after < rep.bytes_before, "compacted DB should reclaim dead blob space");

        // Live record, its blob, and all linked keywords must remain untouched
        let kept = db.get_by_path(&keep_file).unwrap().expect("live file must remain");
        assert_eq!(kept.thumb_blob.as_deref(), Some(blob_keep.as_slice()));
        assert_eq!(kept.keywords, vec!["keep_only".to_string(), "shared".to_string()]);
        assert!(db.get_by_path(&deleted_file).unwrap().is_none());

        // Verify raw keyword table contains ONLY linked keywords
        let all_kws: Vec<String> = db.data.keywords.values().cloned().collect();
        assert_eq!(all_kws.len(), 2);
        assert!(all_kws.contains(&"shared".to_string()));
        assert!(all_kws.contains(&"keep_only".to_string()));
        assert!(!all_kws.contains(&"orphan_only".to_string()));

        // Gallery should keep only the live file
        let gal_paths = db.get_gallery_paths("TestGal").unwrap();
        assert_eq!(gal_paths, vec![keep_file]);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
