//! What the service and the app say over the pipe.
//!
//! The app writes one request line; the service answers and closes. A
//! snapshot lists each volume's names in tree order: every entry's parent
//! comes before it, so the app appends them to its own table as they
//! arrive, with no second pass and no copy of the whole held twice.
//!
//! ```text
//! request   "PFIX1 SNAPSHOT\n" | "PFIX1 HELLO\n"
//! snapshot  "PFIX" u32 version, u32 volumes, then per volume:
//!             u16 root length, root (UTF-8, "C:"), u64 entries, then per entry:
//!               u32 parent (u32::MAX for the root, the first entry)
//!               u64 size, u32 last write (seconds since 1970), u8 flags (1 = folder)
//!               u16 name length, name (UTF-8)
//! hello     "PFIX1 OK <version>\n"
//! error     "PFER" u16 length, message (UTF-8)
//! ```
//!
//! Numbers are little-endian. Changing any of this changes [`VERSION`], and
//! the round-trip test below with it (CLAUDE.md, rule 7).

use std::io::{self, Read, Write};

/// The snapshot format's version.
pub const VERSION: u32 = 1;
/// Asks for every volume's names.
pub const SNAPSHOT: &[u8] = b"PFIX1 SNAPSHOT\n";
/// Asks which version the service speaks.
pub const HELLO: &[u8] = b"PFIX1 HELLO\n";
/// The root's parent.
pub const NO_PARENT: u32 = u32::MAX;

const MAGIC: &[u8; 4] = b"PFIX";
const ERROR: &[u8; 4] = b"PFER";
const FOLDER: u8 = 1;
// No real name is longer (255 UTF-16 units, at most 765 bytes of UTF-8).
const LONGEST_NAME: usize = 1024;
const MOST_ENTRIES: u64 = 200_000_000;

/// One volume's names, in tree order.
#[derive(Default)]
pub struct Volume {
    /// "C:".
    pub root: String,
    names: String,
    entries: Vec<Entry>,
}

#[derive(Clone, Copy)]
struct Entry {
    parent: u32,
    start: u32,
    len: u16,
    dir: bool,
    size: u64,
    modified: u32,
}

impl Volume {
    /// A volume whose first entry, the root, is named `root`.
    pub fn new(root: &str) -> Self {
        let mut volume = Self {
            root: root.to_string(),
            ..Self::default()
        };
        volume.push(NO_PARENT, root, true, 0, 0);
        volume
    }

    /// Adds an entry under `parent` (an index this returned before) and
    /// returns its own index.
    pub fn push(&mut self, parent: u32, name: &str, dir: bool, size: u64, modified: u32) -> u32 {
        let mut end = name.len().min(LONGEST_NAME);
        while !name.is_char_boundary(end) {
            end -= 1;
        }
        let name = &name[..end];
        self.entries.push(Entry {
            parent,
            start: self.names.len() as u32,
            len: name.len() as u16,
            dir,
            size,
            modified,
        });
        self.names.push_str(name);
        (self.entries.len() - 1) as u32
    }

    /// How many entries it holds, the root included.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether it holds only nothing (not even a root).
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Receives a snapshot as it is read.
pub trait Sink {
    /// A volume starts; its entries follow.
    fn volume(&mut self, root: &str);
    /// One entry of the current volume, numbered from 0 in arrival order;
    /// `parent` is an earlier number, or [`NO_PARENT`] for the root.
    fn entry(&mut self, parent: u32, name: &str, dir: bool, size: u64, modified: u32);
}

/// Writes a snapshot of `volumes`.
pub fn write(out: &mut impl Write, volumes: &[Volume]) -> io::Result<()> {
    out.write_all(MAGIC)?;
    out.write_all(&VERSION.to_le_bytes())?;
    out.write_all(&(volumes.len() as u32).to_le_bytes())?;
    for volume in volumes {
        out.write_all(&(volume.root.len() as u16).to_le_bytes())?;
        out.write_all(volume.root.as_bytes())?;
        out.write_all(&(volume.entries.len() as u64).to_le_bytes())?;
        for entry in &volume.entries {
            let name = volume
                .names
                .get(entry.start as usize..entry.start as usize + entry.len as usize)
                .unwrap_or("");
            out.write_all(&entry.parent.to_le_bytes())?;
            out.write_all(&entry.size.to_le_bytes())?;
            out.write_all(&entry.modified.to_le_bytes())?;
            out.write_all(&[if entry.dir { FOLDER } else { 0 }])?;
            out.write_all(&(name.len() as u16).to_le_bytes())?;
            out.write_all(name.as_bytes())?;
        }
    }
    out.flush()
}

/// Writes an error answer.
pub fn write_error(out: &mut impl Write, message: &str) -> io::Result<()> {
    let message = &message.as_bytes()[..message.len().min(4096)];
    out.write_all(ERROR)?;
    out.write_all(&(message.len() as u16).to_le_bytes())?;
    out.write_all(message)?;
    out.flush()
}

fn invalid(what: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("indexer snapshot: {what}"),
    )
}

fn read_array<const N: usize>(input: &mut impl Read) -> io::Result<[u8; N]> {
    let mut bytes = [0u8; N];
    input.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn read_text(input: &mut impl Read, length: usize, buffer: &mut Vec<u8>) -> io::Result<()> {
    buffer.resize(length, 0);
    input.read_exact(buffer)
}

/// Reads a snapshot into `sink`. Anything out of shape (a parent that is not
/// an earlier entry, a name that is not UTF-8, absurd counts) is an error,
/// never a half-built table passed off as whole.
pub fn read(input: &mut impl Read, sink: &mut impl Sink) -> io::Result<()> {
    let magic = read_array::<4>(input)?;
    if &magic == ERROR {
        let length = u16::from_le_bytes(read_array(input)?) as usize;
        let mut message = Vec::new();
        read_text(input, length, &mut message)?;
        return Err(io::Error::other(
            String::from_utf8_lossy(&message).into_owned(),
        ));
    }
    if &magic != MAGIC {
        return Err(invalid("not a snapshot"));
    }
    let version = u32::from_le_bytes(read_array(input)?);
    if version != VERSION {
        return Err(invalid(&format!(
            "version {version}, this app reads {VERSION}"
        )));
    }
    let volumes = u32::from_le_bytes(read_array(input)?);
    if volumes > 64 {
        return Err(invalid("too many volumes"));
    }
    let mut buffer = Vec::new();
    for _ in 0..volumes {
        let length = u16::from_le_bytes(read_array(input)?) as usize;
        read_text(input, length, &mut buffer)?;
        let root = std::str::from_utf8(&buffer)
            .map_err(|_| invalid("root is not UTF-8"))?
            .to_string();
        sink.volume(&root);
        let count = u64::from_le_bytes(read_array(input)?);
        if count > MOST_ENTRIES {
            return Err(invalid("too many entries"));
        }
        for index in 0..count {
            let parent = u32::from_le_bytes(read_array(input)?);
            let size = u64::from_le_bytes(read_array(input)?);
            let modified = u32::from_le_bytes(read_array(input)?);
            let flags = read_array::<1>(input)?[0];
            let length = u16::from_le_bytes(read_array(input)?) as usize;
            if length > LONGEST_NAME {
                return Err(invalid("name too long"));
            }
            let in_order = if index == 0 {
                parent == NO_PARENT
            } else {
                u64::from(parent) < index
            };
            if !in_order {
                return Err(invalid("entry before its parent"));
            }
            read_text(input, length, &mut buffer)?;
            let name = std::str::from_utf8(&buffer).map_err(|_| invalid("name is not UTF-8"))?;
            sink.entry(parent, name, flags & FOLDER != 0, size, modified);
        }
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Collect(Vec<String>);

    impl Sink for Collect {
        fn volume(&mut self, root: &str) {
            self.0.push(format!("volume {root}"));
        }
        fn entry(&mut self, parent: u32, name: &str, dir: bool, size: u64, modified: u32) {
            self.0
                .push(format!("{parent} {name} {dir} {size} {modified}"));
        }
    }

    #[test]
    fn a_snapshot_reads_back_as_written() {
        let mut c = Volume::new("C:");
        let users = c.push(0, "Users", true, 0, 10);
        c.push(users, "ção.wav", false, 1234, 20);
        let d = Volume::new("D:");
        let mut bytes = Vec::new();
        write(&mut bytes, &[c, d]).unwrap();
        let mut seen = Collect::default();
        read(&mut bytes.as_slice(), &mut seen).unwrap();
        assert_eq!(
            seen.0,
            [
                "volume C:",
                "4294967295 C: true 0 0",
                "0 Users true 0 10",
                "1 ção.wav false 1234 20",
                "volume D:",
                "4294967295 D: true 0 0",
            ]
        );
    }

    #[test]
    fn a_snapshot_out_of_shape_is_refused() {
        let mut c = Volume::new("C:");
        c.push(5, "orphan", false, 0, 0);
        let mut bytes = Vec::new();
        write(&mut bytes, &[c]).unwrap();
        assert!(read(&mut bytes.as_slice(), &mut Collect::default()).is_err());
        assert!(read(&mut &b"nope"[..], &mut Collect::default()).is_err());
        let mut error = Vec::new();
        write_error(&mut error, "access denied").unwrap();
        let refused = read(&mut error.as_slice(), &mut Collect::default()).unwrap_err();
        assert_eq!(refused.to_string(), "access denied");
    }
}
