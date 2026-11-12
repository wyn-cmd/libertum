//! The files an installer carries, appended to the end of its own `.exe`.
//!
//! An executable can have bytes after its last section; Windows ignores them when it loads
//! the program. The installer is built once as a plain program (the "stub"), and packing
//! appends the files to a copy of it:
//!
//! ```text
//! stub | file 1 | file 2 | ... | index | index length (u64 LE) | MAGIC
//! index: file count (u32 LE), then per file: name length (u16 LE), name (UTF-8),
//!        data length (u64 LE)
//! ```
//!
//! Reading checks every length against the bytes really present, and accepts only plain
//! file names (no folders, no `..`), so a damaged or tampered installer cannot write
//! outside the folder it extracts into.

use std::fmt;

/// Marks the end of a packed installer.
pub const MAGIC: &[u8; 8] = b"LBTSETUP";

/// A file carried by the installer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct File {
    pub name: String,
    pub data: Vec<u8>,
}

/// Why the carried files could not be read or written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PayloadError {
    /// The executable carries no files (it is the plain stub).
    Missing,
    /// The trailer or index does not fit the bytes present.
    Damaged(&'static str),
    /// A file name is not a plain file name.
    BadName(String),
}

impl fmt::Display for PayloadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing => write!(f, "this installer carries no program files"),
            Self::Damaged(what) => write!(f, "the installer is damaged ({what})"),
            Self::BadName(name) => write!(f, "the installer names an unsafe file: {name:?}"),
        }
    }
}

impl std::error::Error for PayloadError {}

/// True for a plain file name: no folders, no drive, not `.` or `..`, nothing Windows
/// refuses in a name.
pub fn is_plain_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 200
        && name != "."
        && name != ".."
        && !name.chars().any(|c| {
            matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control()
        })
}

/// `stub` followed by `files` and the trailer.
pub fn pack(stub: &[u8], files: &[File]) -> Result<Vec<u8>, PayloadError> {
    let mut out =
        Vec::with_capacity(stub.len() + files.iter().map(|f| f.data.len()).sum::<usize>() + 256);
    out.extend_from_slice(stub);
    let mut index = Vec::new();
    let count = u32::try_from(files.len()).map_err(|_| PayloadError::Damaged("too many files"))?;
    index.extend_from_slice(&count.to_le_bytes());
    for file in files {
        if !is_plain_name(&file.name) {
            return Err(PayloadError::BadName(file.name.clone()));
        }
        out.extend_from_slice(&file.data);
        let name = file.name.as_bytes();
        let name_len =
            u16::try_from(name.len()).map_err(|_| PayloadError::BadName(file.name.clone()))?;
        index.extend_from_slice(&name_len.to_le_bytes());
        index.extend_from_slice(name);
        index.extend_from_slice(&(file.data.len() as u64).to_le_bytes());
    }
    out.extend_from_slice(&index);
    out.extend_from_slice(&(index.len() as u64).to_le_bytes());
    out.extend_from_slice(MAGIC);
    Ok(out)
}

/// Where the carried files start, if `exe` carries any (the length of the stub).
pub fn stub_len(exe: &[u8]) -> Result<usize, PayloadError> {
    Ok(locate(exe)?.0)
}

/// The files carried at the end of `exe`.
pub fn read(exe: &[u8]) -> Result<Vec<File>, PayloadError> {
    let (data_start, entries) = locate(exe)?;
    let mut at = data_start;
    let mut files = Vec::with_capacity(entries.len());
    for (name, len) in entries {
        let end = at
            .checked_add(len)
            .ok_or(PayloadError::Damaged("file length"))?;
        let data = exe.get(at..end).ok_or(PayloadError::Damaged("file data"))?;
        files.push(File {
            name,
            data: data.to_vec(),
        });
        at = end;
    }
    Ok(files)
}

/// The start of the file data and each file's name and length.
fn locate(exe: &[u8]) -> Result<(usize, Vec<(String, usize)>), PayloadError> {
    if exe.len() < 16 || &exe[exe.len() - 8..] != MAGIC {
        return Err(PayloadError::Missing);
    }
    let len_at = exe.len() - 16;
    let index_len = read_u64(exe, len_at).ok_or(PayloadError::Damaged("index length"))?;
    let index_len =
        usize::try_from(index_len).map_err(|_| PayloadError::Damaged("index length"))?;
    let index_start = len_at
        .checked_sub(index_len)
        .ok_or(PayloadError::Damaged("index length"))?;
    let index = &exe[index_start..len_at];
    let count = index
        .get(0..4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .ok_or(PayloadError::Damaged("file count"))?;
    let mut at = 4usize;
    let mut entries = Vec::new();
    let mut total = 0usize;
    for _ in 0..count {
        let name_len = index
            .get(at..at + 2)
            .map(|b| usize::from(u16::from_le_bytes([b[0], b[1]])))
            .ok_or(PayloadError::Damaged("name length"))?;
        at += 2;
        let name = index
            .get(at..at + name_len)
            .and_then(|b| std::str::from_utf8(b).ok())
            .ok_or(PayloadError::Damaged("file name"))?
            .to_owned();
        at += name_len;
        if !is_plain_name(&name) {
            return Err(PayloadError::BadName(name));
        }
        let len = read_u64(index, at).ok_or(PayloadError::Damaged("file length"))?;
        let len = usize::try_from(len).map_err(|_| PayloadError::Damaged("file length"))?;
        at += 8;
        total = total
            .checked_add(len)
            .ok_or(PayloadError::Damaged("file length"))?;
        entries.push((name, len));
    }
    if at != index.len() {
        return Err(PayloadError::Damaged("index"));
    }
    let data_start = index_start
        .checked_sub(total)
        .ok_or(PayloadError::Damaged("file data"))?;
    Ok((data_start, entries))
}

fn read_u64(bytes: &[u8], at: usize) -> Option<u64> {
    let b = bytes.get(at..at.checked_add(8)?)?;
    Some(u64::from_le_bytes([
        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str, data: &[u8]) -> File {
        File {
            name: name.to_owned(),
            data: data.to_vec(),
        }
    }

    #[test]
    fn files_round_trip_after_the_stub() {
        let stub = b"MZ pretend program".to_vec();
        let files = vec![
            file("libertum.exe", &[0u8, 1, 2, 3, 255]),
            file("install.ps1", b"Write-Host 'hi'"),
            file("README.md", b""),
        ];
        let packed = pack(&stub, &files).expect("pack");
        assert!(packed.starts_with(&stub));
        assert_eq!(stub_len(&packed), Ok(stub.len()));
        assert_eq!(read(&packed), Ok(files.clone()));
        // Packing a packed installer again starts from its stub.
        let again = pack(&packed[..stub_len(&packed).expect("stub")], &files).expect("pack");
        assert_eq!(again, packed);
    }

    #[test]
    fn a_plain_stub_carries_nothing() {
        assert_eq!(read(b"MZ just a program"), Err(PayloadError::Missing));
        assert_eq!(read(b""), Err(PayloadError::Missing));
    }

    #[test]
    fn damage_is_an_error_not_a_panic() {
        let packed = pack(b"stub", &[file("a.txt", b"hello")]).expect("pack");
        // Cut bytes out of the middle, flip the index length, truncate: never a panic.
        for cut in 0..packed.len() {
            let mut damaged = packed.clone();
            damaged.remove(cut);
            let _ = read(&damaged);
        }
        let mut wrong = packed.clone();
        let at = wrong.len() - 16;
        wrong[at] = 0xFF;
        assert!(read(&wrong).is_err());
    }

    #[test]
    fn only_plain_file_names_are_packed_or_read() {
        for bad in ["../evil.exe", "a/b", "C:x", "..", "", "a\\b", "x?"] {
            assert!(!is_plain_name(bad), "{bad:?}");
            assert!(matches!(
                pack(b"s", &[file(bad, b"x")]),
                Err(PayloadError::BadName(_))
            ));
        }
        assert!(is_plain_name("libertum.exe"));
        // A name smuggled into the index by hand is refused when read.
        let mut packed = pack(b"s", &[file("aa.txt", b"x")]).expect("pack");
        let name_at = packed
            .windows(6)
            .rposition(|w| w == b"aa.txt")
            .expect("name in index");
        packed[name_at..name_at + 3].copy_from_slice(b"../");
        assert!(matches!(read(&packed), Err(PayloadError::BadName(_))));
    }
}
