//! A whole NTFS volume listed from its master file table ($MFT).
//!
//! Instead of opening every folder (`walk`), read $MFT itself off the raw
//! volume, sequentially in big chunks, and parse each file record: names
//! and parent folders ($FILE_NAME), last write time and attributes
//! ($STANDARD_INFORMATION), size ($DATA), link type ($REPARSE_POINT). Then
//! rebuild the folder tree from the parent references. This is how the
//! fastest Windows search tools index: a few seconds for millions of files.
//!
//! Opening a volume for reading needs administrator rights; without them
//! (or on FAT/ReFS) `scan_volume` returns None and the caller crawls.

use crate::walk::{FLAG_HIDDEN, KIND_DIR, KIND_FILE, KIND_LINK, Listing, NONE, RawEnt};
use rayon::prelude::*;
use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_READ, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_BEGIN, FILE_FLAG_NO_BUFFERING, FlushFileBuffers, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, ReadFile,
    SetFilePointerEx,
};
use windows_sys::Win32::System::IO::DeviceIoControl;
use windows_sys::Win32::System::Ioctl::{FSCTL_GET_NTFS_VOLUME_DATA, NTFS_VOLUME_DATA_BUFFER};

const ROOT: u32 = 5;
/// Records below this are NTFS's own metafiles ($MFT, $LogFile, ... $Extend).
const FIRST_USER: u32 = 16;
const ATTR_HIDDEN: u32 = 0x2;
const ATTR_REPARSE: u32 = 0x400;
const TAG_MOUNT_POINT: u32 = 0xA000_0003;
const TAG_SYMLINK: u32 = 0xA000_000C;
const TAG_APPEXECLINK: u32 = 0x8000_001B;
/// Bytes read per request.
const CHUNK: usize = 8 << 20;

struct Volume(HANDLE);

impl Drop for Volume {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

/// Unbuffered reads need sector-aligned memory; 4 KB covers every disk.
const ALIGN: usize = 4096;

/// A read buffer whose `[at..at + len]` is ALIGN-aligned.
struct Buf {
    v: Vec<u8>,
    at: usize,
    len: usize,
}

impl Buf {
    fn new(len: usize) -> Buf {
        let v = vec![0u8; len + ALIGN];
        let at = v.as_ptr().align_offset(ALIGN);
        Buf { v, at, len }
    }
    fn bytes(&mut self) -> &mut [u8] {
        &mut self.v[self.at..self.at + self.len]
    }
}

impl Volume {
    /// Read `buf.len()` bytes at `off`; both sector multiples.
    fn read_at(&self, off: u64, buf: &mut [u8]) -> bool {
        read_raw(self.0, off, buf)
    }

    /// Any span, aligned to sectors around it (unbuffered reads must be).
    fn read_span(&self, off: u64, len: usize, sector: u64) -> Option<Vec<u8>> {
        let start = off / sector * sector;
        let end = (off + len as u64).div_ceil(sector) * sector;
        let mut b = Buf::new((end - start) as usize);
        self.read_at(start, b.bytes()).then(|| b.bytes()[(off - start) as usize..][..len].to_vec())
    }
}

fn read_raw(h: HANDLE, off: u64, buf: &mut [u8]) -> bool {
    unsafe {
        if SetFilePointerEx(h, off as i64, std::ptr::null_mut(), FILE_BEGIN) == 0 {
            return false;
        }
        let mut n = 0u32;
        ReadFile(h, buf.as_mut_ptr(), buf.len() as u32, &mut n, std::ptr::null_mut()) != 0 && n as usize == buf.len()
    }
}

/// The unnamed non-resident $DATA runs in a fixed-up record, if any, with
/// the VCN they start at.
fn data_runs(rec: &[u8]) -> Option<(u64, Vec<(i64, u64)>)> {
    let mut at = u16_at(rec, 0x14) as usize;
    while at + 16 <= rec.len() {
        let (ty, len) = (u32_at(rec, at), u32_at(rec, at + 4) as usize);
        if ty == 0xFFFF_FFFF || len < 16 || at + len > rec.len() {
            break;
        }
        let a = &rec[at..at + len];
        if ty == 0x80 && a[8] != 0 && a[9] == 0 && len >= 0x40 {
            return Some((u64_at(a, 0x10), runs(&a[u16_at(a, 0x20) as usize..])));
        }
        at += len;
    }
    None
}

/// The $ATTRIBUTE_LIST entries of a record that place $DATA in other
/// records: (starting VCN, record number).
fn data_elsewhere(vol: &Volume, rec: &[u8], cluster: u64, sector: u64) -> Vec<(u64, u64)> {
    let mut at = u16_at(rec, 0x14) as usize;
    let mut list = None;
    while at + 16 <= rec.len() {
        let (ty, len) = (u32_at(rec, at), u32_at(rec, at + 4) as usize);
        if ty == 0xFFFF_FFFF || len < 16 || at + len > rec.len() {
            break;
        }
        let a = &rec[at..at + len];
        if ty == 0x20 {
            list = if a[8] == 0 {
                let (vl, vo) = (u32_at(a, 0x10) as usize, u16_at(a, 0x14) as usize);
                a.get(vo..vo + vl).map(<[u8]>::to_vec)
            } else if len >= 0x40 {
                // A long list lives in clusters of its own.
                let size = u64_at(a, 0x30) as usize;
                let mut v = Vec::new();
                for (lcn, n) in runs(&a[u16_at(a, 0x20) as usize..]) {
                    if lcn < 0 {
                        break;
                    }
                    v.extend(vol.read_span(lcn as u64 * cluster, (n * cluster) as usize, sector).unwrap_or_default());
                }
                v.truncate(size);
                Some(v)
            } else {
                None
            };
        }
        at += len;
    }
    let (Some(l), mut out) = (list, Vec::new()) else { return Vec::new() };
    let mut i = 0;
    while i + 0x1A <= l.len() {
        let (ty, len) = (u32_at(&l, i), u16_at(&l, i + 4) as usize);
        if len == 0 {
            break;
        }
        let no = u64_at(&l, i + 0x10) & 0xFFFF_FFFF_FFFF;
        if ty == 0x80 && l[i + 6] == 0 && no != 0 {
            out.push((u64_at(&l, i + 8), no));
        }
        i += len;
    }
    out.sort();
    out.dedup();
    out
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}
fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}
fn u64_at(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().unwrap())
}

/// Undo the update sequence ("fixups") of one record in place: the last two
/// bytes of each sector hold a check value, the real bytes are in the array.
fn fixup(rec: &mut [u8], sector: usize) -> bool {
    if rec.len() < 48 || &rec[..4] != b"FILE" {
        return false;
    }
    let (off, count) = (u16_at(rec, 4) as usize, u16_at(rec, 6) as usize);
    if count < 2 || off + count * 2 > rec.len() || (count - 1) * sector > rec.len() {
        return false;
    }
    let check = u16_at(rec, off);
    for i in 1..count {
        let end = i * sector - 2;
        if u16_at(rec, end) != check {
            return false;
        }
        rec[end] = rec[off + i * 2];
        rec[end + 1] = rec[off + i * 2 + 1];
    }
    true
}

/// A data run list: (first cluster, cluster count) in file order; sparse
/// runs come back with cluster -1.
fn runs(b: &[u8]) -> Vec<(i64, u64)> {
    let (mut out, mut i, mut lcn) = (Vec::new(), 0usize, 0i64);
    while i < b.len() && b[i] != 0 {
        let (ln, on) = ((b[i] & 0xF) as usize, (b[i] >> 4) as usize);
        i += 1;
        if ln == 0 || ln > 8 || on > 8 || i + ln + on > b.len() {
            break;
        }
        let mut len = 0u64;
        for k in 0..ln {
            len |= (b[i + k] as u64) << (8 * k);
        }
        i += ln;
        if on == 0 {
            out.push((-1, len));
            continue;
        }
        let mut d = 0i64;
        for k in 0..on {
            d |= (b[i + k] as i64) << (8 * k);
        }
        // Sign-extend the delta.
        let shift = 64 - 8 * on as u32;
        d = (d << shift) >> shift;
        i += on;
        lcn += d;
        out.push((lcn, len));
    }
    out
}

/// What one record says about its file.
#[derive(Clone, Copy, Default)]
struct Rec {
    used: bool,
    dir: bool,
    /// walk::KIND_* | FLAG_HIDDEN, once known.
    kind: u8,
    size: u64,
    mtime: u32,
}

/// One name of a file (hard links have several): (record, parent, name).
struct Name {
    rec: u32,
    parent: u32,
    off: u32,
    len: u16,
}

#[derive(Default)]
struct Part {
    recs: Vec<(u32, Rec)>,
    names: Vec<Name>,
    bytes: Vec<u8>,
    /// From extension records: (base record, size of its unnamed $DATA).
    sizes: Vec<(u32, u64)>,
}

/// Parse the records in `chunk` (record numbers from `first`).
fn parse(chunk: &mut [u8], first: u32, rec_size: usize, sector: usize) -> Part {
    let mut p = Part::default();
    for (k, rec) in chunk.chunks_exact_mut(rec_size).enumerate() {
        let no = first + k as u32;
        if !fixup(rec, sector) {
            continue;
        }
        let flags = u16_at(rec, 0x16);
        if flags & 1 == 0 {
            continue;
        }
        let base = (u64_at(rec, 0x20) & 0xFFFF_FFFF_FFFF) as u32;
        let owner = if base != 0 { base } else { no };
        let mut r = Rec { used: base == 0, dir: flags & 2 != 0, ..Rec::default() };
        let (mut attrs, mut tag, mut data_size, mut fn_size) = (None::<u32>, 0u32, None::<u64>, 0u64);
        let mut at = u16_at(rec, 0x14) as usize;
        let used_len = (u32_at(rec, 0x18) as usize).min(rec.len());
        while at + 16 <= used_len {
            let ty = u32_at(rec, at);
            let len = u32_at(rec, at + 4) as usize;
            if ty == 0xFFFF_FFFF || len < 16 || at + len > used_len {
                break;
            }
            let a = &rec[at..at + len];
            let resident = a[8] == 0;
            let named = a[9] != 0;
            let value = if resident {
                let (vl, vo) = (u32_at(a, 0x10) as usize, u16_at(a, 0x14) as usize);
                a.get(vo..vo + vl)
            } else {
                None
            };
            match ty {
                0x10 => {
                    if let Some(v) = value.filter(|v| v.len() >= 0x24) {
                        let t = u64_at(v, 0x08);
                        r.mtime = crate::sys::filetime_secs(t as u32, (t >> 32) as u32);
                        attrs = Some(u32_at(v, 0x20));
                    }
                }
                0x30 => {
                    if let Some(v) = value.filter(|v| v.len() >= 0x42) {
                        let n = v[0x40] as usize;
                        // Skip the 8.3 short name; Win32, POSIX, Win32+DOS stay.
                        if v[0x41] != 2 && v.len() >= 0x42 + 2 * n {
                            let parent = (u64_at(v, 0) & 0xFFFF_FFFF_FFFF) as u32;
                            let w: Vec<u16> = (0..n).map(|i| u16_at(v, 0x42 + 2 * i)).collect();
                            let name = crate::sys::from_wide(&w);
                            if !name.is_empty() && name.len() <= u16::MAX as usize {
                                p.names.push(Name { rec: owner, parent, off: p.bytes.len() as u32, len: name.len() as u16 });
                                p.bytes.extend_from_slice(&name);
                            }
                            fn_size = u64_at(v, 0x30);
                        }
                    }
                }
                0x80 if !named => {
                    if resident {
                        data_size = Some(u32_at(a, 0x10) as u64);
                    } else if a.len() >= 0x40 && u64_at(a, 0x10) == 0 {
                        data_size = Some(u64_at(a, 0x30));
                    }
                }
                0xC0 => {
                    if let Some(v) = value.filter(|v| v.len() >= 4) {
                        tag = u32_at(v, 0);
                    }
                }
                _ => {}
            }
            at += len;
        }
        if base != 0 {
            if let Some(s) = data_size {
                p.sizes.push((base, s));
            }
            continue;
        }
        let attrs = attrs.unwrap_or(0);
        r.kind = if attrs & ATTR_REPARSE != 0 && matches!(tag, TAG_SYMLINK | TAG_MOUNT_POINT | TAG_APPEXECLINK) {
            KIND_LINK
        } else if r.dir {
            KIND_DIR
        } else {
            KIND_FILE
        };
        if attrs & ATTR_HIDDEN != 0 {
            r.kind |= FLAG_HIDDEN;
        }
        r.size = if r.kind & 3 == KIND_FILE { data_size.unwrap_or(fn_size) } else { 0 };
        p.recs.push((no, r));
    }
    p
}

/// Every entry of the NTFS volume `drive` (`/C:`) as `walk` listings:
/// listing 0 is the drive's root. None when the volume can't be read raw.
pub fn scan_volume(drive: &[u8]) -> Option<Vec<Listing>> {
    let (parts, total) = read_parts(drive)?;
    Some(build(parts, total, drive))
}

/// The whole table, parsed: the parts, and the record count.
fn read_parts(drive: &[u8]) -> Option<(Vec<Part>, usize)> {
    let letter = *drive.get(1)? as char;
    let path: Vec<u16> = format!(r"\\.\{letter}:").encode_utf16().chain([0]).collect();
    // Unbuffered: 2 GB of table read once shouldn't push the page cache out.
    let h = unsafe {
        CreateFileW(
            path.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_NO_BUFFERING,
            std::ptr::null_mut(),
        )
    };
    if h == INVALID_HANDLE_VALUE {
        return None;
    }
    let vol = Volume(h);
    let mut vd: NTFS_VOLUME_DATA_BUFFER = unsafe { std::mem::zeroed() };
    let mut n = 0u32;
    let ok = unsafe {
        DeviceIoControl(vol.0, FSCTL_GET_NTFS_VOLUME_DATA, std::ptr::null(), 0, (&raw mut vd).cast(), size_of::<NTFS_VOLUME_DATA_BUFFER>() as u32, &mut n, std::ptr::null_mut())
    };
    if ok == 0 {
        return None;
    }
    // NTFS writes changed file records to disk lazily; reading the disk
    // raw would miss what changed in the last while (busy caches: thousands
    // of files). Have it write them out first. Changes during the scan
    // reach the index through the watcher, which starts before it.
    unsafe { FlushFileBuffers(vol.0) };
    let cluster = vd.BytesPerCluster as u64;
    let sector = vd.BytesPerSector as usize;
    let rec_size = vd.BytesPerFileRecordSegment as usize;
    if cluster == 0 || sector == 0 || rec_size == 0 || rec_size % sector != 0 || ALIGN % sector != 0 {
        return None;
    }
    // Record 0 is $MFT itself: its $DATA runs say where the table lies. A
    // fragmented table carries more runs in extension records, which its
    // $ATTRIBUTE_LIST names; they're read through the runs known so far.
    let mut r0 = vol.read_span(vd.MftStartLcn as u64 * cluster, rec_size, sector as u64)?;
    if !fixup(&mut r0, sector) {
        return None;
    }
    let (_, mut extents) = data_runs(&r0)?;
    for (vcn, no) in data_elsewhere(&vol, &r0, cluster, sector as u64) {
        if vcn == 0 {
            continue;
        }
        let Some(off) = offset_of(&extents, no * rec_size as u64, cluster) else { continue };
        let Some(mut rec) = vol.read_span(off, rec_size, sector as u64) else { continue };
        if fixup(&mut rec, sector)
            && let Some((start, more)) = data_runs(&rec)
            && start == vcn
        {
            extents.extend(more);
        }
    }
    if extents.is_empty() {
        return None;
    }
    let total = (vd.MftValidDataLength as u64 / rec_size as u64) as usize;
    // One thread reads the table in CHUNK pieces, in order; the pool parses
    // pieces as they come.
    let (tx, rx) = std::sync::mpsc::sync_channel::<(u32, Buf)>(rayon::current_num_threads().max(2));
    let hv = vol.0 as usize;
    let mut read_all = true;
    let parts: Vec<Part> = std::thread::scope(|s| {
        let reader = s.spawn(move || {
            let per = CHUNK / rec_size;
            let mut next_rec = 0usize;
            for &(lcn, clusters) in &extents {
                let (mut left, mut off) = (clusters * cluster, 0u64);
                while left > 0 && next_rec < total {
                    let want = (left as usize).min(per * rec_size).min((total - next_rec) * rec_size) / rec_size * rec_size;
                    if want == 0 {
                        break;
                    }
                    let mut b = Buf::new(want.div_ceil(sector) * sector);
                    if lcn >= 0 && !read_raw(hv as HANDLE, lcn as u64 * cluster + off, b.bytes()) {
                        return false;
                    }
                    b.len = want;
                    if tx.send((next_rec as u32, b)).is_err() {
                        return false;
                    }
                    next_rec += want / rec_size;
                    off += want as u64;
                    left -= want as u64;
                }
            }
            true
        });
        let parts = rx.into_iter().par_bridge().map(|(first, mut b)| parse(b.bytes(), first, rec_size, sector)).collect();
        read_all = reader.join().unwrap_or(false);
        parts
    });
    drop(vol);
    read_all.then_some((parts, total))
}

/// Where byte `pos` of the table lies on the volume, through its runs.
fn offset_of(extents: &[(i64, u64)], mut pos: u64, cluster: u64) -> Option<u64> {
    for &(lcn, n) in extents {
        let len = n * cluster;
        if pos < len {
            return (lcn >= 0).then(|| lcn as u64 * cluster + pos);
        }
        pos -= len;
    }
    None
}

/// The folder tree from the parsed records, as listings (0: the root).
fn build(parts: Vec<Part>, n: usize, drive: &[u8]) -> Vec<Listing> {
    let mut recs = vec![Rec::default(); n.max(ROOT as usize + 1)];
    for p in &parts {
        for &(no, r) in &p.recs {
            if let Some(slot) = recs.get_mut(no as usize) {
                *slot = r;
            }
        }
    }
    for p in &parts {
        for &(base, size) in &p.sizes {
            if let Some(r) = recs.get_mut(base as usize).filter(|r| r.kind & 3 == KIND_FILE && r.size == 0) {
                r.size = size;
            }
        }
    }
    // Children per folder record, by counting sort: (part, name index).
    let mut count = vec![0u32; recs.len() + 1];
    let keep = |nm: &Name| {
        nm.rec >= FIRST_USER
            && (nm.rec as usize) < recs.len()
            && recs[nm.rec as usize].used
            && recs.get(nm.parent as usize).is_some_and(|p| p.used && p.dir)
            && nm.rec != nm.parent
    };
    for p in &parts {
        for nm in p.names.iter().filter(|nm| keep(nm)) {
            count[nm.parent as usize + 1] += 1;
        }
    }
    for i in 1..count.len() {
        count[i] += count[i - 1];
    }
    let mut fill = count.clone();
    let mut kids = vec![(0u32, 0u32); *count.last().unwrap() as usize];
    for (pi, p) in parts.iter().enumerate() {
        for (ni, nm) in p.names.iter().enumerate().filter(|(_, nm)| keep(nm)) {
            let s = &mut fill[nm.parent as usize];
            kids[*s as usize] = (pi as u32, ni as u32);
            *s += 1;
        }
    }
    // Walk down from the root, a listing per folder reached.
    let blocking = crate::walk::SKIP.get().is_some_and(|v| !v.is_empty());
    let mut out = Vec::new();
    let mut seen = vec![false; recs.len()];
    let mut stack: Vec<(u32, u32, Vec<u8>)> = vec![(ROOT, 0, drive.to_vec())];
    let mut next_id = 1u32;
    seen[ROOT as usize] = true;
    while let Some((dir, id, path)) = stack.pop() {
        let range = count[dir as usize] as usize..count[dir as usize + 1] as usize;
        let mut l = Listing { id, names: Vec::new(), ents: Vec::with_capacity(range.len()) };
        for &(pi, ni) in &kids[range] {
            let p = &parts[pi as usize];
            let nm = &p.names[ni as usize];
            let name = &p.bytes[nm.off as usize..nm.off as usize + nm.len as usize];
            let r = recs[nm.rec as usize];
            let mut child = NONE;
            if r.kind & 3 == KIND_DIR && !seen[nm.rec as usize] {
                let cp = if blocking { crate::live::join(&path, name) } else { Vec::new() };
                if !(blocking && crate::walk::blocked(&cp)) {
                    seen[nm.rec as usize] = true;
                    child = next_id;
                    next_id += 1;
                    stack.push((nm.rec, child, cp));
                }
            }
            l.ents.push(RawEnt { name_off: l.names.len() as u32, name_len: name.len() as u16, kind: r.kind, size: r.size, mtime: r.mtime, child });
            l.names.extend_from_slice(name);
        }
        out.push(l);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_lists() {
        // 0x21: 1-byte length, 2-byte offset. Then a negative delta, then sparse.
        let b = [0x21, 0x10, 0x00, 0x01, 0x11, 0x05, 0xF0, 0x01, 0x08, 0x00];
        assert_eq!(runs(&b), vec![(0x100, 0x10), (0x100 - 16, 5), (-1, 8)]);
    }
}
