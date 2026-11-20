//! Read one file out of an installed XPI (a zip archive) without unpacking it,
//! so the inspector can hash and code-stamp the worker Firefox actually loads
//! (`<profile>\extensions\<gecko id>.xpi`). Supports the stored (0) and
//! deflate (8) methods `web-ext` and AMO signing produce; zip64, encryption and
//! multi-disk archives are refused.

const EOCD_SIGNATURE: u32 = 0x0605_4b50;
const CENTRAL_SIGNATURE: u32 = 0x0201_4b50;
const LOCAL_SIGNATURE: u32 = 0x0403_4b50;
const EOCD_LEN: usize = 22;
const CENTRAL_HEADER_LEN: usize = 46;
const LOCAL_HEADER_LEN: usize = 30;
/// Longest zip comment the end-of-central-directory scan must skip.
const MAX_COMMENT_LEN: usize = 0xFFFF;
const METHOD_STORED: u16 = 0;
const METHOD_DEFLATE: u16 = 8;
const FLAG_ENCRYPTED: u16 = 0x0001;
/// Upper bound for one inflated entry (the worker is under 1 MiB today).
const MAX_ENTRY_BYTES: usize = 64 * 1024 * 1024;

/// Central-directory facts of one entry.
struct CentralEntry {
    flags: u16,
    method: u16,
    compressed_len: usize,
    uncompressed_len: usize,
    local_offset: usize,
}

/// The bytes of entry `name` in `archive`.
///
/// # Errors
/// `xpi_entry_absent`, or a reason naming the structural defect.
pub fn read_entry(archive: &[u8], name: &str) -> Result<Vec<u8>, String> {
    let (central_offset, count) = end_of_central_directory(archive)?;
    let entry = find_central_entry(archive, central_offset, count, name)?;
    if entry.flags & FLAG_ENCRYPTED != 0 {
        return Err(format!("xpi_entry_encrypted name={name}"));
    }
    let data = local_entry_data(archive, &entry)?;
    match entry.method {
        METHOD_STORED if data.len() == entry.uncompressed_len => Ok(data.to_vec()),
        METHOD_STORED => Err(format!("xpi_entry_size_mismatch name={name}")),
        METHOD_DEFLATE => inflate(data, entry.uncompressed_len, name),
        other => Err(format!(
            "xpi_entry_method_unsupported name={name} method={other}"
        )),
    }
}

fn inflate(data: &[u8], expected_len: usize, name: &str) -> Result<Vec<u8>, String> {
    if expected_len > MAX_ENTRY_BYTES {
        return Err(format!(
            "xpi_entry_too_large name={name} bytes={expected_len}"
        ));
    }
    let inflated = miniz_oxide::inflate::decompress_to_vec_with_limit(data, MAX_ENTRY_BYTES)
        .map_err(|error| format!("xpi_entry_inflate_failed name={name} error={error:?}"))?;
    if inflated.len() == expected_len {
        Ok(inflated)
    } else {
        Err(format!("xpi_entry_size_mismatch name={name}"))
    }
}

fn end_of_central_directory(archive: &[u8]) -> Result<(usize, usize), String> {
    let last_start = archive
        .len()
        .checked_sub(EOCD_LEN)
        .ok_or_else(|| "xpi_too_short".to_owned())?;
    let first_start = last_start.saturating_sub(MAX_COMMENT_LEN);
    let eocd = (first_start..=last_start)
        .rev()
        .find(|&start| u32_at(archive, start) == Some(EOCD_SIGNATURE))
        .ok_or_else(|| "xpi_end_of_central_directory_absent".to_owned())?;
    let disk = u16_at(archive, eocd + 4);
    let central_disk = u16_at(archive, eocd + 6);
    if disk != Some(0) || central_disk != Some(0) {
        return Err("xpi_multi_disk_unsupported".to_owned());
    }
    let count = u16_at(archive, eocd + 10).ok_or_else(|| "xpi_eocd_truncated".to_owned())?;
    let offset = u32_at(archive, eocd + 16).ok_or_else(|| "xpi_eocd_truncated".to_owned())?;
    if count == u16::MAX || offset == u32::MAX {
        return Err("xpi_zip64_unsupported".to_owned());
    }
    Ok((to_usize(offset)?, usize::from(count)))
}

fn find_central_entry(
    archive: &[u8],
    central_offset: usize,
    count: usize,
    name: &str,
) -> Result<CentralEntry, String> {
    let mut cursor = central_offset;
    for _ in 0..count {
        let (entry, entry_name, next) = central_entry_at(archive, cursor)
            .ok_or_else(|| "xpi_central_directory_corrupt".to_owned())?;
        if entry_name == name.as_bytes() {
            return Ok(entry);
        }
        cursor = next;
    }
    Err(format!("xpi_entry_absent name={name}"))
}

/// The central-directory record at `cursor`, its name and the next cursor.
fn central_entry_at(archive: &[u8], cursor: usize) -> Option<(CentralEntry, &[u8], usize)> {
    if u32_at(archive, cursor)? != CENTRAL_SIGNATURE {
        return None;
    }
    let name_len = usize::from(u16_at(archive, cursor + 28)?);
    let extra_len = usize::from(u16_at(archive, cursor + 30)?);
    let comment_len = usize::from(u16_at(archive, cursor + 32)?);
    let name_start = cursor + CENTRAL_HEADER_LEN;
    let name = archive.get(name_start..name_start + name_len)?;
    let entry = CentralEntry {
        flags: u16_at(archive, cursor + 8)?,
        method: u16_at(archive, cursor + 10)?,
        compressed_len: usize::try_from(u32_at(archive, cursor + 20)?).ok()?,
        uncompressed_len: usize::try_from(u32_at(archive, cursor + 24)?).ok()?,
        local_offset: usize::try_from(u32_at(archive, cursor + 42)?).ok()?,
    };
    Some((entry, name, name_start + name_len + extra_len + comment_len))
}

fn local_entry_data<'a>(archive: &'a [u8], entry: &CentralEntry) -> Result<&'a [u8], String> {
    let start = entry.local_offset;
    if u32_at(archive, start) != Some(LOCAL_SIGNATURE) {
        return Err("xpi_local_header_corrupt".to_owned());
    }
    let header_len = u16_at(archive, start + 26)
        .zip(u16_at(archive, start + 28))
        .map(|(name_len, extra_len)| usize::from(name_len) + usize::from(extra_len))
        .ok_or_else(|| "xpi_local_header_truncated".to_owned())?;
    let data_start = start + LOCAL_HEADER_LEN + header_len;
    data_start
        .checked_add(entry.compressed_len)
        .and_then(|end| archive.get(data_start..end))
        .ok_or_else(|| "xpi_entry_data_truncated".to_owned())
}

fn u16_at(bytes: &[u8], offset: usize) -> Option<u16> {
    let slice = bytes.get(offset..offset.checked_add(2)?)?;
    slice.try_into().ok().map(u16::from_le_bytes)
}

fn u32_at(bytes: &[u8], offset: usize) -> Option<u32> {
    let slice = bytes.get(offset..offset.checked_add(4)?)?;
    slice.try_into().ok().map(u32::from_le_bytes)
}

fn to_usize(value: u32) -> Result<usize, String> {
    usize::try_from(value).map_err(|_| "xpi_offset_overflow".to_owned())
}
