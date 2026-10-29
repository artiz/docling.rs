//! Minimal read-only Compound File Binary (CFB / OLE2) reader.
//!
//! The container behind the legacy binary Office formats (`.doc`, `.xls`,
//! `.ppt`, issue #127): a FAT filesystem-in-a-file holding named streams.
//! This reader does exactly what the `doc`/`ppt` backends need — open the
//! container and extract a named stream's bytes — with the hostile-input
//! guards the rest of the crate applies to archive formats: chain walks are
//! bounded by the sector count (no cycle can loop forever) and stream sizes
//! are capped by the same per-part budget as OOXML parts.
//!
//! Layout ([MS-CFB]): a 512-byte header (zero-padded to a whole 4096-byte
//! sector in a version 4 file) names the first sectors of the DIFAT (which
//! locates the FAT), the directory chain, and the mini FAT. Streams ≥
//! `mini_stream_cutoff` (4096) chain through the FAT; smaller ones live in
//! the *mini stream* (the root entry's stream) and chain through the mini FAT
//! in 64-byte mini sectors.

use crate::backend::ooxml;

const HEADER_MAGIC: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
const ENDOFCHAIN: u32 = 0xFFFF_FFFE;
const FREESECT: u32 = 0xFFFF_FFFF;
/// Sector numbers ≥ this are special markers (DIFSECT/FATSECT/…), never data.
const NOSTREAM: u32 = 0xFFFF_FFFF;
const MAXREGSECT: u32 = 0xFFFF_FFFA;

/// One directory entry we care about: a named stream (or storage).
struct DirEntry {
    name: String,
    object_type: u8,
    /// Left/right siblings and first child in the directory's red-black
    /// tree, as entry indices (`NOSTREAM` = none). Office streams are looked
    /// up by name among the root's children ([`CompoundFile::stream`]);
    /// .msg storages repeat stream names per recipient/attachment, so those
    /// walk the tree themselves.
    left: u32,
    right: u32,
    child: u32,
    start_sector: u32,
    size: u64,
}

/// An opened compound file: parsed FAT/directory, ready to extract streams.
pub(crate) struct CompoundFile<'a> {
    data: &'a [u8],
    sector_size: usize,
    fat: Vec<u32>,
    mini_fat: Vec<u32>,
    /// The root entry's stream, read eagerly: it *is* the mini stream.
    mini_stream: Vec<u8>,
    entries: Vec<DirEntry>,
}

fn u16_at(d: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(d.get(o..o + 2)?.try_into().ok()?))
}

fn u32_at(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(o..o + 4)?.try_into().ok()?))
}

impl<'a> CompoundFile<'a> {
    /// `true` when `data` starts with the CFB signature.
    pub(crate) fn detect(data: &[u8]) -> bool {
        data.get(..8) == Some(&HEADER_MAGIC)
    }

    pub(crate) fn open(data: &'a [u8]) -> Option<Self> {
        if !Self::detect(data) {
            return None;
        }
        let sector_shift = u16_at(data, 30)?; // 9 (512) for v3, 12 (4096) for v4
        if !(7..=16).contains(&sector_shift) {
            return None;
        }
        let sector_size = 1usize << sector_shift;
        // [MS-CFB] 2.6.3: a version 3 (512-byte sector) file's stream sizes
        // fit 32 bits, and some older writers left the high half of the
        // 64-bit size uninitialized — parsers should ignore it, or a valid
        // stream reads as exabytes and is dropped over the part budget.
        let size_mask = if sector_shift == 9 {
            u32::MAX as u64
        } else {
            u64::MAX
        };
        let sector_count = data.len() / sector_size; // bound for every chain walk

        // DIFAT: 109 entries in the header, then a chain of DIFAT sectors.
        let mut fat_sectors: Vec<u32> = Vec::new();
        for i in 0..109 {
            let s = u32_at(data, 76 + i * 4)?;
            if s < MAXREGSECT {
                fat_sectors.push(s);
            }
        }
        let mut difat_sector = u32_at(data, 68)?;
        let mut difat_walked = 0usize;
        while difat_sector < MAXREGSECT && difat_walked <= sector_count {
            difat_walked += 1;
            let base = sector_offset(difat_sector, sector_size);
            let per = sector_size / 4 - 1;
            for i in 0..per {
                let s = u32_at(data, base + i * 4)?;
                if s < MAXREGSECT {
                    fat_sectors.push(s);
                }
            }
            difat_sector = u32_at(data, base + per * 4)?;
        }

        // FAT: the concatenated entries of every FAT sector.
        let mut fat: Vec<u32> = Vec::with_capacity(fat_sectors.len() * (sector_size / 4));
        for s in fat_sectors {
            let base = sector_offset(s, sector_size);
            for i in 0..sector_size / 4 {
                fat.push(u32_at(data, base + i * 4).unwrap_or(FREESECT));
            }
        }

        // Directory: walk its FAT chain, parse 128-byte entries.
        let dir_start = u32_at(data, 48)?;
        let dir_bytes = read_chain(data, &fat, dir_start, sector_size, u64::MAX)?;
        let mut entries = Vec::new();
        for chunk in dir_bytes.chunks_exact(128) {
            let name_len = u16_at(chunk, 64)? as usize; // bytes incl. terminator
            if !(2..=64).contains(&name_len) {
                // Keep a placeholder so sibling/child ids (which are indices
                // into this table) stay aligned — .msg storage walks need them.
                entries.push(DirEntry {
                    name: String::new(),
                    object_type: 0,
                    left: NOSTREAM,
                    right: NOSTREAM,
                    child: NOSTREAM,
                    start_sector: 0,
                    size: 0,
                });
                continue;
            }
            let name: String = chunk[..name_len - 2]
                .chunks_exact(2)
                .map(|b| u16::from_le_bytes([b[0], b[1]]))
                .map(|u| char::from_u32(u as u32).unwrap_or('\u{FFFD}'))
                .collect();
            entries.push(DirEntry {
                name,
                object_type: chunk[66],
                left: u32_at(chunk, 68)?,
                right: u32_at(chunk, 72)?,
                child: u32_at(chunk, 76)?,
                start_sector: u32_at(chunk, 116)?,
                size: (u32_at(chunk, 120)? as u64 | ((u32_at(chunk, 124)? as u64) << 32))
                    & size_mask,
            });
        }

        // Mini FAT + mini stream (the root entry's chain).
        let mini_fat_start = u32_at(data, 60)?;
        let mini_fat_bytes =
            read_chain(data, &fat, mini_fat_start, sector_size, u64::MAX).unwrap_or_default();
        let mini_fat: Vec<u32> = mini_fat_bytes
            .chunks_exact(4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect();
        let mini_stream = entries
            .iter()
            .find(|e| e.object_type == 5)
            .and_then(|root| read_chain(data, &fat, root.start_sector, sector_size, root.size))
            .unwrap_or_default();

        Some(Self {
            data,
            sector_size,
            fat,
            mini_fat,
            mini_stream,
            entries,
        })
    }

    /// Extract a stream's bytes by name (exact match). The Office streams the
    /// backends ask for (`WordDocument`, `0Table`/`1Table`, `Data`,
    /// `PowerPoint Document`, …) are the root storage's, so its direct
    /// children are searched first: an embedded OLE object — a Word document
    /// pasted into a Word document, stored under `ObjectPool/_<id>` — brings
    /// its own `WordDocument`/`1Table`/`Data`, and whichever sorts first in
    /// the directory used to win, pairing the outer FIB with the inner table
    /// stream (#512). Only when the root has no such stream is the whole
    /// directory searched, as before, for producers that nest it. `None`
    /// for a missing stream or one over the per-part budget.
    pub(crate) fn stream(&self, name: &str) -> Option<Vec<u8>> {
        self.stream_by_index(self.find_stream(name)?)
    }

    /// Whether the directory names this stream — telling a missing stream
    /// apart from one [`stream`](Self::stream) cannot read (cut off by a
    /// truncated file, over the part budget) for the error message.
    pub(crate) fn has_stream(&self, name: &str) -> bool {
        self.find_stream(name).is_some()
    }

    fn find_stream(&self, name: &str) -> Option<usize> {
        let is_match = |i: &usize| {
            self.entries
                .get(*i)
                .is_some_and(|e| e.object_type == 2 && e.name == name)
        };
        self.children_of(None)
            .into_iter()
            .find(is_match)
            .or_else(|| (0..self.entries.len()).find(is_match))
    }

    /// The entry indices of a storage's children (`None` = the root storage),
    /// collected by exhaustively walking the sibling tree and sorted by index
    /// — directory order, which for .msg matches the writer's creation order
    /// (`__attach_…_#00000000` before `#00000001`).
    pub(crate) fn children_of(&self, parent: Option<usize>) -> Vec<usize> {
        let start = match parent {
            Some(i) => self.entries.get(i).map(|e| e.child),
            None => self
                .entries
                .iter()
                .find(|e| e.object_type == 5)
                .map(|e| e.child),
        };
        let mut out = Vec::new();
        let mut stack = vec![start.unwrap_or(NOSTREAM)];
        while let Some(id) = stack.pop() {
            let Some(e) = self.entries.get(id as usize) else {
                continue;
            };
            // Bounded: each index is pushed at most once from its parent link,
            // and a malformed cycle is cut by the visited check.
            if out.contains(&(id as usize)) {
                continue;
            }
            out.push(id as usize);
            stack.push(e.left);
            stack.push(e.right);
        }
        out.sort_unstable();
        out
    }

    /// A directory entry's name (empty for placeholder/invalid entries).
    pub(crate) fn entry_name(&self, idx: usize) -> &str {
        self.entries.get(idx).map_or("", |e| e.name.as_str())
    }

    /// Whether entry `idx` is a storage (a directory, e.g. one .msg
    /// recipient/attachment).
    pub(crate) fn is_storage(&self, idx: usize) -> bool {
        self.entries.get(idx).is_some_and(|e| e.object_type == 1)
    }

    /// Extract a stream's bytes by directory-entry index. `None` for a
    /// non-stream entry or one over the per-part budget.
    pub(crate) fn stream_by_index(&self, idx: usize) -> Option<Vec<u8>> {
        let entry = self.entries.get(idx)?;
        if entry.object_type != 2 {
            return None;
        }
        if entry.size > ooxml::max_part_bytes() {
            return None;
        }
        if entry.size < 4096 {
            // Mini stream: 64-byte sectors chained through the mini FAT.
            read_mini_chain(
                &self.mini_stream,
                &self.mini_fat,
                entry.start_sector,
                entry.size,
            )
        } else {
            read_chain(
                self.data,
                &self.fat,
                entry.start_sector,
                self.sector_size,
                entry.size,
            )
        }
    }

    /// A stream of the root storage by name — root only, for structures
    /// that must not be confused with an embedded document's (#512), such
    /// as an encrypted package's `EncryptionInfo` (#625).
    pub(crate) fn root_stream(&self, name: &str) -> Option<usize> {
        self.children_of(None).into_iter().find(|&i| {
            self.entries
                .get(i)
                .is_some_and(|e| e.object_type == 2 && e.name == name)
        })
    }

    /// Where stream `idx`'s bytes sit in the file, in stream order — one
    /// range per sector (64-byte mini sectors map through the root entry's
    /// chain). Decryption (#625) writes a stream's plaintext back over these
    /// ranges, so the decrypted file is the original container with the
    /// same layout, read by the same backend. `None` where
    /// [`stream_by_index`](Self::stream_by_index) fails, or when the ranges
    /// run past the end of the file.
    pub(crate) fn stream_spans(&self, idx: usize) -> Option<Vec<std::ops::Range<usize>>> {
        let entry = self.entries.get(idx)?;
        if entry.object_type != 2 || entry.size > ooxml::max_part_bytes() {
            return None;
        }
        // (file offset, unit) per sector of the stream, in order.
        let mut sectors: Vec<usize> = Vec::new();
        let unit;
        if entry.size < 4096 {
            unit = 64;
            // The mini stream is the root entry's FAT chain.
            let root = self.entries.iter().find(|e| e.object_type == 5)?;
            let root_sectors = fat_chain(&self.fat, root.start_sector);
            let mut m = entry.start_sector;
            while sectors.len() * unit < entry.size as usize {
                if m >= MAXREGSECT || sectors.len() > self.mini_fat.len() {
                    return None;
                }
                let off = m as usize * 64;
                let sector = *root_sectors.get(off / self.sector_size)?;
                sectors.push(sector_offset(sector, self.sector_size) + off % self.sector_size);
                m = *self.mini_fat.get(m as usize)?;
            }
        } else {
            unit = self.sector_size;
            let chain = fat_chain(&self.fat, entry.start_sector);
            for s in chain {
                if sectors.len() * unit >= entry.size as usize {
                    break;
                }
                sectors.push(sector_offset(s, self.sector_size));
            }
        }
        let mut left = entry.size as usize;
        let mut spans = Vec::with_capacity(sectors.len());
        for base in sectors {
            let n = left.min(unit);
            if base + n > self.data.len() {
                return None;
            }
            spans.push(base..base + n);
            left -= n;
        }
        (left == 0).then_some(spans)
    }

    /// The error for a file [`open`](Self::open) rejected or whose `name`
    /// stream [`stream`](Self::stream) could not read, prefixed `fmt:` — a
    /// damaged compound file (the signature is there) reads differently from
    /// a file of another kind, and a stream the directory names but the
    /// sectors cannot supply from one that is absent.
    pub(crate) fn open_error(fmt: &str, data: &[u8]) -> String {
        if Self::detect(data) {
            format!("{fmt}: damaged compound file (truncated or corrupt)")
        } else {
            format!("{fmt}: not a compound file")
        }
    }

    pub(crate) fn stream_error(&self, fmt: &str, name: &str) -> String {
        if self.has_stream(name) {
            format!("{fmt}: {name} stream unreadable (file truncated or corrupt)")
        } else {
            format!("{fmt}: no {name} stream")
        }
    }

    /// Names of all stream entries.
    pub(crate) fn stream_names(&self) -> impl Iterator<Item = &str> {
        self.entries
            .iter()
            .filter(|e| e.object_type == 2)
            .map(|e| e.name.as_str())
    }
}

/// Byte offset of sector `n`. The header occupies the file's first sector
/// ([MS-CFB] 2.2): 512 bytes in a version 3 file, and in a version 4 file
/// (4096-byte sectors) the same 512 bytes zero-padded to a whole sector, so
/// sector 0 starts at 4096 there — `512 + n * 4096` read every v4 structure
/// 3584 bytes early and rejected valid files as damaged (#623; PowerPoint
/// writes v4 when an edited .ppt is saved in place).
fn sector_offset(n: u32, sector_size: usize) -> usize {
    sector_size.max(512) + n as usize * sector_size
}

/// The sector numbers of the FAT chain from `start`, cut at the first
/// special marker and bounded by the FAT length (a cycle ends the walk).
fn fat_chain(fat: &[u32], start: u32) -> Vec<u32> {
    let mut out = Vec::new();
    let mut s = start;
    while s < MAXREGSECT && out.len() <= fat.len() {
        out.push(s);
        match fat.get(s as usize) {
            Some(&next) => s = next,
            None => break,
        }
    }
    out
}

/// Follow a FAT chain from `start`, concatenating sectors, truncated to `size`
/// (`u64::MAX` for the structural chains — directory, mini FAT — which run to
/// `ENDOFCHAIN`). Bounded by the FAT length — a cyclic chain terminates
/// instead of spinning.
///
/// The file's last sector may be short (#521): pre-97 writers (Word 6.0/95
/// among them) did not pad the file to a whole sector, against [MS-CFB] 2.3
/// but common in the wild. Its bytes are read as far as the file goes, as
/// long as only the unused tail is missing — the stream ends inside them, or
/// a structural chain ends on that sector (a directory entry cut off by the
/// end of file is dropped by the 128-byte chunking). A sector that *starts*
/// past the end of the file, or a short one the chain still needs more bytes
/// from, is a truncated file ([MS-CFB] 5) and fails as before.
fn read_chain(
    data: &[u8],
    fat: &[u32],
    start: u32,
    sector_size: usize,
    size: u64,
) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut sector = start;
    let mut walked = 0usize;
    while sector < MAXREGSECT {
        walked += 1;
        if walked > fat.len() + 1 {
            return None; // cycle
        }
        let base = sector_offset(sector, sector_size);
        if base >= data.len() {
            return None;
        }
        let end = data.len().min(base + sector_size);
        out.extend_from_slice(&data[base..end]);
        if out.len() as u64 >= size {
            break;
        }
        let next = *fat.get(sector as usize)?;
        if end - base < sector_size && (next != ENDOFCHAIN || size != u64::MAX) {
            return None; // the missing bytes were data
        }
        sector = next;
    }
    if sector == ENDOFCHAIN || out.len() as u64 >= size {
        out.truncate(out.len().min(size.try_into().unwrap_or(usize::MAX)));
        Some(out)
    } else {
        None
    }
}

/// Follow a mini-FAT chain through the mini stream (64-byte sectors). A mini
/// sector past the mini stream's end fails the read like
/// [`read_chain`]'s sector past the end of file — notably when the mini
/// stream itself could not be read (a truncated file, #521), which used to
/// hand every small stream back empty instead of missing.
fn read_mini_chain(mini_stream: &[u8], mini_fat: &[u32], start: u32, size: u64) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut sector = start;
    let mut walked = 0usize;
    while sector < MAXREGSECT {
        walked += 1;
        if walked > mini_fat.len() + 1 {
            return None; // cycle
        }
        let base = sector as usize * 64;
        if base >= mini_stream.len() {
            return None;
        }
        out.extend_from_slice(&mini_stream[base..(base + 64).min(mini_stream.len())]);
        if out.len() as u64 >= size {
            break;
        }
        sector = *mini_fat.get(sector as usize)?;
    }
    out.truncate(out.len().min(size.try_into().unwrap_or(usize::MAX)));
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_cfb_data() {
        assert!(!CompoundFile::detect(b"PK\x03\x04"));
        assert!(CompoundFile::open(b"not a compound file").is_none());
        assert!(CompoundFile::open(&[]).is_none());
    }

    #[test]
    fn opens_real_word_file_and_reads_streams() {
        let data = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/data/doc/sources/docx_lists.doc"
        ))
        .unwrap();
        let cfb = CompoundFile::open(&data).expect("valid CFB");
        let names: Vec<&str> = cfb.stream_names().collect();
        assert!(names.contains(&"WordDocument"), "streams: {names:?}");
        let word = cfb.stream("WordDocument").expect("WordDocument stream");
        assert_eq!(&word[..2], &[0xEC, 0xA5], "FIB wIdent magic");
        assert!(cfb.stream("NoSuchStream").is_none());
    }

    #[test]
    fn root_streams_win_over_an_embedded_documents_streams() {
        // A Word document embedding another Word document (#512): the
        // embedded one's `WordDocument`/`1Table`/`Data` sit under
        // `ObjectPool/_<id>/` (and one level deeper for its own embed), and
        // the outer document's must be the ones returned.
        let data = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/data/doc/sources/embedded_word_object.doc"
        ))
        .unwrap();
        let cfb = CompoundFile::open(&data).expect("valid CFB");
        let tables = cfb.stream_names().filter(|n| *n == "1Table").count();
        assert_eq!(tables, 2, "fixture must carry a nested 1Table");
        assert_eq!(cfb.stream("1Table").map(|s| s.len()), Some(11349));
        assert_eq!(cfb.stream("WordDocument").map(|s| s.len()), Some(4165));
        // Only nested: still found by the directory-wide fallback.
        assert_eq!(cfb.stream("\u{1}Ole10Native").map(|s| s.len()), Some(80821));
    }

    #[test]
    fn v3_stream_size_ignores_an_uninitialized_high_half() {
        // [MS-CFB] 2.6.3: older writers left the high 32 bits of a v3
        // stream size as garbage (Apache POI's Bug51944.doc); reading them
        // made `WordDocument` look exabytes long and get dropped.
        let mut data = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/data/doc/sources/docx_lists.doc"
        ))
        .unwrap();
        let clean = CompoundFile::open(&data).unwrap().stream("WordDocument");
        assert!(clean.is_some());
        // Poison the high half of every directory entry's size.
        let dir_start = u32_at(&data, 48).unwrap();
        let base = sector_offset(dir_start, 512);
        for e in 0..4 {
            data[base + e * 128 + 124..base + e * 128 + 128].copy_from_slice(&[0x9F; 4]);
        }
        let cfb = CompoundFile::open(&data).expect("valid CFB");
        assert_eq!(cfb.stream("WordDocument"), clean);
    }

    /// Every named stream of `cfb` with its bytes, in directory order (a
    /// directory entry cut off by the end of file is an unused one here).
    fn all_streams(cfb: &CompoundFile) -> Vec<(String, Option<Vec<u8>>)> {
        (0..cfb.entries.len())
            .filter(|&i| cfb.entries[i].object_type == 2)
            .map(|i| (cfb.entries[i].name.clone(), cfb.stream_by_index(i)))
            .collect()
    }

    /// #521: pre-97 writers (Word 6.0/95) leave the file's last sector short.
    /// Cutting into the unused tail of whichever structure owns that sector
    /// — the directory, the root's mini stream, a FAT-chained stream — must
    /// read every stream exactly as the padded file does; one byte further
    /// cuts into data and must fail, never return a short stream.
    #[test]
    fn short_last_sector_reads_like_the_padded_file() {
        let own = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/doc/sources/");
        // (fixture, owner of the last sector, bytes of it the owner doesn't
        // use, a stream that loses data when the cut goes one byte further)
        for (file, owner, unused, lost) in [
            ("poi_word95.doc", "directory", 8, None),
            // The root entry's chain *is* the mini stream: every stream under
            // 4096 bytes lives in it.
            (
                "poi_word6_sections2.doc",
                "mini stream",
                128,
                Some("\u{1}CompObj"),
            ),
            (
                "embedded_word_object.doc",
                "WordDocument",
                443,
                Some("WordDocument"),
            ),
        ] {
            let data = std::fs::read(format!("{own}{file}")).unwrap();
            let padded = CompoundFile::open(&data).expect("valid CFB");
            let want = all_streams(&padded);
            for cut in [1, 8, unused] {
                let short = &data[..data.len() - cut];
                let cfb = CompoundFile::open(short)
                    .unwrap_or_else(|| panic!("{file} ({owner}) cut by {cut}: open failed"));
                assert!(
                    all_streams(&cfb) == want,
                    "{file} ({owner}) cut by {cut}: streams differ"
                );
            }
            if let Some(lost) = lost {
                let short = &data[..data.len() - unused - 1];
                let cfb = CompoundFile::open(short).expect("directory intact");
                assert!(cfb.stream(lost).is_none(), "{file}: {lost} lost data");
                assert_eq!(
                    cfb.stream_error("doc", lost),
                    format!("doc: {lost} stream unreadable (file truncated or corrupt)")
                );
            }
        }
    }

    /// A chain sector that starts past the end of the file is a truncated
    /// file, not a short final sector ([MS-CFB] 5).
    #[test]
    fn sector_past_the_end_of_file_is_rejected() {
        let data = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/data/doc/sources/poi_word95.doc"
        ))
        .unwrap();
        // The directory chain is sectors 197, 198 — the last two.
        let short = &data[..data.len() - 512];
        assert!(CompoundFile::open(short).is_none());
        assert_eq!(
            CompoundFile::open_error("doc", short),
            "doc: damaged compound file (truncated or corrupt)"
        );
        assert_eq!(
            CompoundFile::open_error("doc", b"PK\x03\x04"),
            "doc: not a compound file"
        );
    }

    /// Rewrite a version 3 compound file as version 4 (#623): a 4096-byte
    /// header sector, 4096-byte sectors, every FAT-chained stream, the mini
    /// stream, the mini FAT and the directory re-chained in that order, the
    /// FAT sector last. The directory gets a padding sector of unused entries
    /// so its chain spans two sectors, and with `difat_sector` the FAT
    /// sector is listed in a DIFAT sector instead of the header — a layout
    /// only files of >109 FAT sectors (≈450 MB at 4 KiB) need, so it is the
    /// only way a test reaches that read. Mini sectors stay 64 bytes in both
    /// versions: the mini stream and mini FAT are copied verbatim.
    fn to_v4(v3: &[u8], difat_sector: bool) -> Vec<u8> {
        const SZ: usize = 4096;
        let cfb = CompoundFile::open(v3).expect("valid v3 CFB");
        assert_eq!(cfb.sector_size, 512, "source must be version 3");
        let mut dir = read_chain(v3, &cfb.fat, u32_at(v3, 48).unwrap(), 512, u64::MAX).unwrap();
        let mut body: Vec<u8> = Vec::new();
        let mut fat: Vec<u32> = Vec::new();
        let mut put = |bytes: &[u8], fat: &mut Vec<u32>| -> (u32, u32) {
            let (start, n) = (fat.len() as u32, bytes.len().div_ceil(SZ).max(1) as u32);
            fat.extend((1..=n).map(|i| if i == n { ENDOFCHAIN } else { start + i }));
            body.extend_from_slice(bytes);
            body.resize(fat.len() * SZ, 0);
            (start, n)
        };
        let set = |dir: &mut Vec<u8>, i: usize, start: u32| {
            dir[i * 128 + 116..i * 128 + 120].copy_from_slice(&start.to_le_bytes());
            dir[i * 128 + 124..i * 128 + 128].fill(0); // v4 sizes are 64-bit
        };
        for (i, e) in cfb.entries.iter().enumerate() {
            if e.object_type == 2 && e.size >= 4096 {
                let (start, _) = put(&cfb.stream_by_index(i).unwrap(), &mut fat);
                set(&mut dir, i, start);
            } else if e.object_type == 5 && !cfb.mini_stream.is_empty() {
                let (start, _) = put(&cfb.mini_stream, &mut fat);
                set(&mut dir, i, start);
            } else {
                set(&mut dir, i, e.start_sector);
            }
        }
        let mini_fat: Vec<u8> = cfb.mini_fat.iter().flat_map(|v| v.to_le_bytes()).collect();
        let (mfat_start, mfat_n) = put(&mini_fat, &mut fat);
        let entries = dir.len() / 128;
        dir.resize((entries.div_ceil(32) + 1) * SZ, 0); // unused entries: name length 0
        let (dir_start, dir_n) = put(&dir, &mut fat);
        // The FAT itself: its own sector (and the DIFAT sector) marked, one
        // 1024-entry sector covers every test file.
        let fat_sector = fat.len() as u32;
        fat.push(0xFFFF_FFFD); // FATSECT
        if difat_sector {
            fat.push(0xFFFF_FFFC); // DIFSECT
        }
        assert!(
            fat.len() <= SZ / 4,
            "test file too large for one FAT sector"
        );
        let mut fat_bytes: Vec<u8> = fat.iter().flat_map(|v| v.to_le_bytes()).collect();
        fat_bytes.resize(SZ, 0xFF);
        body.extend_from_slice(&fat_bytes);
        let mut header = vec![0u8; SZ];
        header[..8].copy_from_slice(&HEADER_MAGIC);
        header[24..26].copy_from_slice(&0x003Eu16.to_le_bytes());
        header[26..28].copy_from_slice(&4u16.to_le_bytes()); // major 4
        header[28..30].copy_from_slice(&0xFFFEu16.to_le_bytes());
        header[30..32].copy_from_slice(&12u16.to_le_bytes()); // 4096-byte sectors
        header[32..34].copy_from_slice(&6u16.to_le_bytes());
        header[40..44].copy_from_slice(&dir_n.to_le_bytes()); // v4 only
        header[44..48].copy_from_slice(&1u32.to_le_bytes());
        header[48..52].copy_from_slice(&dir_start.to_le_bytes());
        header[56..60].copy_from_slice(&4096u32.to_le_bytes());
        header[60..64].copy_from_slice(&mfat_start.to_le_bytes());
        header[64..68].copy_from_slice(&mfat_n.to_le_bytes());
        header[76..512].fill(0xFF);
        if difat_sector {
            let mut difat = vec![0xFFu8; SZ];
            difat[..4].copy_from_slice(&fat_sector.to_le_bytes());
            difat[SZ - 4..].copy_from_slice(&ENDOFCHAIN.to_le_bytes());
            body.extend_from_slice(&difat);
            header[68..72].copy_from_slice(&(fat_sector + 1).to_le_bytes());
            header[72..76].copy_from_slice(&1u32.to_le_bytes());
        } else {
            header[68..72].copy_from_slice(&ENDOFCHAIN.to_le_bytes());
            header[76..80].copy_from_slice(&fat_sector.to_le_bytes());
        }
        header.extend_from_slice(&body);
        header
    }

    /// #623: version 4 files (4096-byte sectors, the header padded to one
    /// sector) read every stream exactly as their version 3 source — FAT-
    /// chained streams, the mini stream, a two-sector directory, a DIFAT
    /// sector — and convert to the same document through each CFB backend
    /// (.xls goes through calamine's own reader).
    #[test]
    fn v4_files_read_like_their_v3_source() {
        use crate::{DocumentConverter, InputFormat, SourceDocument};
        let own = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/");
        let mirror = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/data/");
        let md = |name: &str, bytes: Vec<u8>| {
            let ext = name.rsplit('.').next().unwrap();
            let fmt = InputFormat::from_extension(ext).unwrap();
            DocumentConverter::new()
                .convert(SourceDocument::from_bytes(name, fmt, bytes))
                .unwrap_or_else(|e| panic!("{name}: {e}"))
                .document
                .export_to_markdown()
        };
        for path in [
            format!("{mirror}doc/sources/docx_lists.doc"),
            format!("{own}doc/sources/embedded_word_object.doc"),
            format!("{mirror}ppt/sources/powerpoint_sample.ppt"),
            format!("{mirror}xls/sources/xlsx_05_table_with_title.xls"),
            format!("{own}quattro/sources/formatcorpus_test.qpw"),
            format!("{own}staroffice5/sources/Writer_3.1.sdw"),
        ] {
            let name = path.rsplit('/').next().unwrap();
            let v3 = std::fs::read(&path).unwrap();
            let want_streams = all_streams(&CompoundFile::open(&v3).unwrap());
            let want_md = md(name, v3.clone());
            assert!(!want_md.trim().is_empty(), "{name}: empty baseline");
            for difat in [false, true] {
                let v4 = to_v4(&v3, difat);
                let cfb = CompoundFile::open(&v4)
                    .unwrap_or_else(|| panic!("{name} v4 (difat {difat}): open failed"));
                assert_eq!(cfb.sector_size, 4096);
                assert!(
                    all_streams(&cfb) == want_streams,
                    "{name} v4 (difat {difat}): streams differ"
                );
                assert_eq!(md(name, v4), want_md, "{name} v4 (difat {difat})");
            }
        }
    }

    /// The PowerPoint-written version 4 file of #623 (an edited .ppt saved
    /// in place) opens, and losing its last sector — the end of the
    /// `\u{5}SummaryInformation` stream — is still reported as damage to
    /// that stream, not read as a shorter one.
    #[test]
    fn real_v4_file_opens_and_its_truncation_is_damage() {
        let data = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/data/ppt/sources/ppt_cfb_v4_edit_save.ppt"
        ))
        .unwrap();
        assert_eq!(u16_at(&data, 26), Some(4), "fixture must be version 4");
        let cfb = CompoundFile::open(&data).expect("valid v4 CFB");
        assert_eq!(cfb.sector_size, 4096);
        let len = |cfb: &CompoundFile, name| cfb.stream(name).map(|s| s.len());
        assert_eq!(len(&cfb, "PowerPoint Document"), Some(35685));
        assert_eq!(len(&cfb, "Current User"), Some(32)); // a mini stream entry
        assert_eq!(len(&cfb, "\u{5}SummaryInformation"), Some(43648));
        let short = &data[..data.len() - 4096];
        let cfb = CompoundFile::open(short).expect("directory and FAT intact");
        assert_eq!(len(&cfb, "PowerPoint Document"), Some(35685));
        assert_eq!(len(&cfb, "\u{5}SummaryInformation"), None);
        assert_eq!(
            cfb.stream_error("ppt", "\u{5}SummaryInformation"),
            "ppt: \u{5}SummaryInformation stream unreadable (file truncated or corrupt)"
        );
    }

    /// The spans of every stream — FAT-chained and mini — concatenate to
    /// exactly the bytes `stream_by_index` reads, in v3 and v4 files (#625
    /// rewrites decrypted streams through them).
    #[test]
    fn stream_spans_cover_exactly_the_stream() {
        let mirror = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/data/");
        for path in [
            format!("{mirror}doc/sources/docx_lists.doc"),
            format!("{mirror}ppt/sources/powerpoint_sample.ppt"),
        ] {
            let v3 = std::fs::read(&path).unwrap();
            for data in [v3.clone(), to_v4(&v3, false)] {
                let cfb = CompoundFile::open(&data).unwrap();
                let mut small_and_large = (0, 0);
                for i in 0..cfb.entries.len() {
                    let Some(want) = cfb.stream_by_index(i) else {
                        continue;
                    };
                    let got: Vec<u8> = cfb
                        .stream_spans(i)
                        .unwrap()
                        .into_iter()
                        .flat_map(|r| data[r].to_vec())
                        .collect();
                    assert_eq!(got, want, "{path}: {}", cfb.entries[i].name);
                    if want.len() < 4096 {
                        small_and_large.0 += 1;
                    } else {
                        small_and_large.1 += 1;
                    }
                }
                assert!(small_and_large.0 > 0 && small_and_large.1 > 0, "{path}");
            }
        }
    }

    #[test]
    fn truncated_header_is_rejected_not_panicked() {
        let data = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/data/doc/sources/docx_lists.doc"
        ))
        .unwrap();
        // Every truncation point must fail cleanly — `None`, never a panic.
        for cut in [8, 76, 512, 700] {
            assert!(
                CompoundFile::open(&data[..cut.min(data.len())]).is_none(),
                "cut at {cut}"
            );
        }
    }
}
