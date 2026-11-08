use std::time::{SystemTime, UNIX_EPOCH};

/// Current UTC time as RFC 3339 with seconds, e.g. `2026-11-03T17:20:05Z`.
pub fn now() -> String {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    format_ts(secs)
}

pub fn format_ts(secs: i64) -> String {
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    // civil-from-days (Howard Hinnant)
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, (rem / 60) % 60, rem % 60)
}

/// Time-ordered unique id with a short type prefix, e.g. `a-01936f0e-...`.
pub fn new_id(prefix: &str) -> String {
    format!("{prefix}-{}", uuid::Uuid::now_v7())
}

pub fn sha256_bytes(b: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(b))
}

pub fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let t = out.trim_matches('-');
    let t: String = t.chars().take(40).collect();
    if t.is_empty() { "page".into() } else { t }
}

/// Write via a temporary file and rename, so a crash never leaves a half-written file.
pub fn atomic_write(path: &std::path::Path, data: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    {
        use std::io::Write;
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}

pub fn rel_path(root: &std::path::Path, p: &std::path::Path) -> String {
    p.strip_prefix(root).map(|r| r.to_string_lossy().replace('\\', "/")).unwrap_or_else(|_| p.to_string_lossy().to_string())
}

#[cfg(test)]
mod tests {
    #[test]
    fn timestamps() {
        assert_eq!(super::format_ts(0), "1970-01-01T00:00:00Z");
        assert_eq!(super::format_ts(1793722136), "2026-11-03T16:08:56Z");
        assert_eq!(super::slug("Ch 01 / p.003"), "ch-01-p-003");
    }
}

/// JPEG XL files start with a bare codestream marker or an ISO-BMFF container box.
pub fn is_jxl(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0xFF, 0x0A]) || bytes.starts_with(&[0, 0, 0, 0x0C, b'J', b'X', b'L', b' '])
}

/// Decode a picture from memory: everything the `image` crate reads, plus JPEG XL.
pub fn decode_image(bytes: &[u8]) -> Result<image::DynamicImage, String> {
    if is_jxl(bytes) {
        let dec = jxl_oxide::integration::JxlDecoder::new(std::io::Cursor::new(bytes)).map_err(|e| format!("JPEG XL: {e}"))?;
        return image::DynamicImage::from_decoder(dec).map_err(|e| format!("JPEG XL: {e}"));
    }
    image::load_from_memory(bytes).map_err(|e| e.to_string())
}

/// Open a picture file (PNG, JPEG, WebP, BMP, TIFF if supported, JPEG XL).
pub fn open_image(path: &std::path::Path) -> Result<image::DynamicImage, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    decode_image(&bytes).map_err(|e| format!("{}: {e}", path.display()))
}
