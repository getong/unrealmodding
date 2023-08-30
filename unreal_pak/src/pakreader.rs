//! PakFile data structure for reading large pak files

use std::collections::BTreeMap;
use std::io::{Read, Seek};

use crate::compression::CompressionMethods;
use crate::entry::read_entry;
use crate::error::PakError;
use crate::header::Header;
use crate::index::Index;
use crate::pakversion::PakVersion;

/// An Unreal pak file reader with it's data kept on disk and only read on demand.
#[derive(Debug)]
pub struct PakReader<R>
where
    R: Read + Seek,
{
    /// version of the pak file format this one is using
    pak_version: PakVersion,
    /// mount point (Unreal stuff)
    pub mount_point: String,
    compression: CompressionMethods,
    entries: BTreeMap<String, Header>,
    reader: R,
}

impl<R> PakReader<R>
where
    R: Read + Seek,
{
    /// Creates a new `PakReader` that reads from the provided reader.
    /// When using a reader that uses syscalls like a `File` it is recommended to wrap it in a
    /// [`std::io::BufReader`] to avoid unnecessary syscalls.
    pub fn new(reader: R) -> Self {
        Self {
            pak_version: PakVersion::Invalid,
            mount_point: "".to_owned(),
            compression: Default::default(),
            entries: BTreeMap::new(),
            reader,
        }
    }

    /// Load the entry info contained in the footer into memory to start reading individual entries.
    pub fn load_index(&mut self) -> Result<(), PakError> {
        let index = Index::read(&mut self.reader)?;

        self.pak_version = index.footer.pak_version;
        self.mount_point = index.mount_point.clone();
        self.compression = index.footer.compression_methods;

        for (name, header) in index.entries {
            self.entries.insert(name, header);
        }

        Ok(())
    }

    /// Returns the names of all entries which have been found.
    pub fn get_entry_names(&self) -> Vec<&String> {
        self.entries.keys().collect()
    }

    /// Checks if the pak file contains an entry with the given name
    pub fn contains_entry(&self, name: &String) -> bool {
        self.entries.contains_key(name)
    }

    /// Reads an entry from the pak on disk into memory and returns it's data.
    pub fn read_entry(&mut self, name: &String) -> Result<Vec<u8>, PakError> {
        let header = self
            .entries
            .get(name)
            .ok_or_else(|| PakError::entry_not_found(name.clone()))?;
        self.read_entry_at_offset(header.offset)
    }

    fn read_entry_at_offset(&mut self, offset: u64) -> Result<Vec<u8>, PakError> {
        read_entry(
            &mut self.reader,
            self.pak_version,
            &self.compression,
            offset,
        )
    }

    /// Iterate over the entries in the PakReader
    pub fn iter(&mut self) -> PakReaderIter<R> {
        PakReaderIter {
            reader: &mut self.reader,
            pak_version: self.pak_version,
            compression: self.compression,
            iter: self.entries.iter(),
        }
    }

    /// Consumes the `PakReader`, returning the wrapped reader.
    /// There are no guarantees for what state the reader might be in.
    pub fn into_inner(self) -> R {
        self.reader
    }
}

/// An iterator over the entries of a PakReader
pub struct PakReaderIter<'a, R>
where
    R: Read + Seek,
{
    reader: &'a mut R,
    pak_version: PakVersion,
    compression: CompressionMethods,
    iter: std::collections::btree_map::Iter<'a, String, Header>,
}

impl<'a, R> Iterator for PakReaderIter<'a, R>
where
    R: Read + Seek,
{
    type Item = (&'a String, Result<Vec<u8>, PakError>);

    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next().map(|(name, header)| {
            (
                name,
                read_entry(
                    &mut self.reader,
                    self.pak_version,
                    &self.compression,
                    header.offset,
                ),
            )
        })
    }
}

impl<'a, R> IntoIterator for &'a mut PakReader<R>
where
    R: Read + Seek,
{
    type Item = (&'a String, Result<Vec<u8>, PakError>);

    type IntoIter = PakReaderIter<'a, R>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}
