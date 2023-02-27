use std::io::{Read, Write};

use byteorder::{ReadBytesExt, WriteBytesExt, LE};

use crate::error::PakError;

pub trait BufReaderExt {
    fn read_fstring(&mut self) -> Result<String, PakError>;
}
pub trait BufWriterExt {
    fn write_fstring(&mut self, string: Option<&str>) -> Result<(), PakError>;
}

impl<R> BufReaderExt for R
where
    R: Read,
{
    fn read_fstring(&mut self) -> Result<String, PakError> {
        // todo: unicode encoding
        let len = self.read_u32::<LE>()?;

        if len == 0 {
            return Ok(String::new());
        }

        let mut buf = vec![0u8; len as usize - 1];
        self.read_exact(&mut buf)?;

        // skip null terminator
        self.read_exact(&mut [0])?;

        Ok(String::from_utf8(buf).unwrap_or_else(|_| String::from("None")))
    }
}

impl<W> BufWriterExt for W
where
    W: Write,
{
    fn write_fstring(&mut self, string: Option<&str>) -> Result<(), PakError> {
        if string.is_none() {
            self.write_i32::<LE>(0)?;
            return Ok(());
        }
        let string = string.unwrap();
        let is_unicode = string.len() != string.chars().count();
        match is_unicode {
            true => self.write_i32::<LE>(-(string.len() as i32) + 1)?,
            false => self.write_i32::<LE>(string.len() as i32 + 1)?,
        };
        let bytes = string.as_bytes();
        self.write_all(bytes)?;
        self.write_all(&[0u8; 1])?;
        Ok(())
    }
}
