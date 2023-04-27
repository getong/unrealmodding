#![deny(missing_docs)]
//! This crate is used for parsing Unreal Engine uasset files
//!
//! # Examples
//!
//! ## Reading an asset that doesn't use bulk data
//!
//! ```no_run
//! use std::fs::File;
//!
//! use unreal_asset::{
//!     Asset,
//!     engine_version::EngineVersion,
//! };
//!
//! let mut file = File::open("asset.uasset").unwrap();
//! let mut asset = Asset::new(file, None, EngineVersion::VER_UE4_23).unwrap();
//!
//! println!("{:#?}", asset);
//! ```
//!
//! ## Reading an asset that uses bulk data
//!
//! ```no_run
//! use std::fs::File;
//!
//! use unreal_asset::{
//!     Asset,
//!     engine_version::EngineVersion,
//! };
//!
//! let mut file = File::open("asset.uasset").unwrap();
//! let mut bulk_file = File::open("asset.uexp").unwrap();
//! let mut asset = Asset::new(file, Some(bulk_file), EngineVersion::VER_UE4_23).unwrap();
//!
//! println!("{:#?}", asset);
//! ```
use std::collections::hash_map::DefaultHasher;
use std::collections::HashSet;
use std::fmt::{Debug, Formatter};
use std::hash::{Hash, Hasher};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::mem::size_of;

use bitvec::prelude::*;
use byteorder::{BigEndian, LittleEndian, ReadBytesExt, WriteBytesExt};

use properties::Property;
use unreal_helpers::{UnrealReadExt, UnrealWriteExt};

pub mod ac7;
pub mod containers;
mod crc;
pub mod custom_version;
pub mod engine_version;
pub mod enums;
pub mod error;
pub mod exports;
pub mod flags;
pub mod fproperty;
pub mod kismet;
pub mod object_version;
pub mod properties;
pub mod reader;
pub mod registry;
pub mod types;
pub mod unversioned;
pub mod uproperty;

use containers::chain::Chain;
use containers::indexed_map::IndexedMap;
use custom_version::{CustomVersion, CustomVersionTrait};
use engine_version::{get_object_versions, guess_engine_version, EngineVersion};
use error::{Error, PropertyError};
use exports::{
    base_export::BaseExport, class_export::ClassExport, data_table_export::DataTableExport,
    enum_export::EnumExport, function_export::FunctionExport, level_export::LevelExport,
    normal_export::NormalExport, property_export::PropertyExport, raw_export::RawExport,
    string_table_export::StringTableExport, Export, ExportBaseTrait, ExportNormalTrait,
    ExportTrait,
};
use flags::EPackageFlags;
use fproperty::FProperty;
use object_version::{ObjectVersion, ObjectVersionUE5};
use properties::{world_tile_property::FWorldTileInfo, PropertyDataTrait};
use reader::{asset_reader::AssetReader, asset_trait::AssetTrait, asset_writer::AssetWriter};
use types::{FName, GenerationInfo, Guid, PackageIndex};
use unversioned::header::UnversionedHeaderFragment;
use unversioned::{header::UnversionedHeader, Usmap};

/// Cast a Property/Export to a more specific type
///
/// # Examples
///
/// ```no_run,ignore
/// use unreal_asset::{
///     cast,
///     properties::{
///         Property,
///         int_property::DoubleProperty,
///     },
/// };
/// let a: Property = ...;
/// let b: &DoubleProperty = cast!(Property, DoubleProperty, &a).unwrap();
/// ```
#[macro_export]
macro_rules! cast {
    ($namespace:ident, $type:ident, $field:expr) => {
        match $field {
            $namespace::$type(e) => Some(e),
            _ => None,
        }
    };
}

/// Import struct for an Asset
///
/// This is used for referencing other assets
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Import {
    /// Class package
    pub class_package: FName,
    /// Class name
    pub class_name: FName,
    /// Outer index
    pub outer_index: PackageIndex,
    /// Object name
    pub object_name: FName,
}

impl Import {
    /// Create a new `Import` instance
    pub fn new(
        class_package: FName,
        class_name: FName,
        outer_index: PackageIndex,
        object_name: FName,
    ) -> Self {
        Import {
            class_package,
            class_name,
            object_name,
            outer_index,
        }
    }
}

/// Parent Class Info
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ParentClassInfo {
    /// Parent classpath
    pub parent_class_path: FName,
    /// Parent class export name
    pub parent_class_export_name: FName,
}

const UE4_ASSET_MAGIC: u32 = u32::from_be_bytes([0xc1, 0x83, 0x2a, 0x9e]);

/// Asset header
struct AssetHeader {
    /// Name map offset
    name_offset: i32,
    /// Imports offset
    import_offset: i32,
    /// Exports offset
    export_offset: i32,
    /// Dependencies offset
    depends_offset: i32,
    /// Soft package references offset
    soft_package_reference_offset: i32,
    /// Asset registry data offset
    asset_registry_data_offset: i32,
    /// World tile info offset
    world_tile_info_offset: i32,
    /// Preload dependency count
    preload_dependency_count: i32,
    /// Preload dependency offset
    preload_dependency_offset: i32,
    /// Header offset
    header_offset: i32,
    /// Bulk data start offset
    bulk_data_start_offset: i64,
}

//#[derive(Debug)]
/// Unreal Engine uasset
pub struct Asset<C: Read + Seek> {
    // raw data
    cursor: Chain<C>,

    // parsed data
    /// Asset info
    pub info: String,
    /// Does asset use .uexp files
    pub use_separate_bulk_data_files: bool,
    /// Object version
    pub object_version: ObjectVersion,
    /// UE5 object version
    pub object_version_ue5: ObjectVersionUE5,
    /// Legacy file version
    pub legacy_file_version: i32,
    /// Is asset unversioned
    pub unversioned: bool,
    /// File license version
    pub file_license_version: i32,
    /// Custom versions
    pub custom_versions: Vec<CustomVersion>,

    // imports
    // exports
    // depends map
    // soft package reference list
    // asset registry data
    // world tile info
    // preload dependencies
    /// Generations
    pub generations: Vec<GenerationInfo>,
    /// Asset guid
    pub package_guid: Guid,
    /// Recorded engine version
    pub engine_version_recorded: FEngineVersion,
    /// Compatible engine version
    pub engine_version_compatible: FEngineVersion,
    /// Chunk ids
    chunk_ids: Vec<i32>,
    /// Asset flags
    pub package_flags: EPackageFlags,
    /// Asset source
    pub package_source: u32,
    /// Folder name
    pub folder_name: String,

    // map struct type override
    // override name map hashes
    // todo: isn't this just AssetHeader?
    /// Header offset
    header_offset: i32,
    /// Name count
    name_count: i32,
    /// Name offset
    name_offset: i32,
    /// Gatherable text data count
    gatherable_text_data_count: i32,
    /// Gatherable text data offset
    gatherable_text_data_offset: i32,
    /// Export count
    export_count: i32,
    /// Exports offset
    export_offset: i32,
    /// Import count
    import_count: i32,
    /// Imports offset
    import_offset: i32,
    /// Depends offset
    depends_offset: i32,
    /// Soft package reference count
    soft_package_reference_count: i32,
    /// Soft package reference offset
    soft_package_reference_offset: i32,
    /// Searchable names offset
    searchable_names_offset: i32,
    /// Thumbnail table offset
    thumbnail_table_offset: i32,
    /// Compression flags
    compression_flags: u32,
    /// Asset registry data offset
    asset_registry_data_offset: i32,
    /// Bulk data start offset
    bulk_data_start_offset: i64,
    /// World tile info offset
    world_tile_info_offset: i32,
    /// Preload dependency count
    preload_dependency_count: i32,
    /// Preload dependency offset
    preload_dependency_offset: i32,

    /// Overriden name map hashes
    pub override_name_map_hashes: IndexedMap<String, u32>,
    /// Name map index list
    name_map_index_list: Vec<String>,
    /// Name map lookup
    name_map_lookup: IndexedMap<u64, i32>,
    /// Imports
    pub imports: Vec<Import>,
    /// Exports
    pub exports: Vec<Export>,
    /// Depends map
    depends_map: Option<Vec<Vec<i32>>>,
    /// Soft package reference list
    soft_package_reference_list: Option<Vec<String>>,
    /// World tile info
    pub world_tile_info: Option<FWorldTileInfo>,

    /// Array struct type overrides
    pub array_struct_type_override: IndexedMap<String, String>,
    /// Map key overrides
    pub map_key_override: IndexedMap<String, String>,
    /// Map value overrides
    pub map_value_override: IndexedMap<String, String>,
    /// .usmap mappings
    pub mappings: Option<Usmap>,

    /// Parent class
    parent_class: Option<ParentClassInfo>,
}

struct AssetSerializer<'asset, 'cursor, W: Read + Seek + Write, C: Read + Seek> {
    asset: &'asset Asset<C>,
    cursor: &'cursor mut W,
    cached_parent_info: Option<ParentClassInfo>,
}

impl<'asset, 'cursor, W: Read + Seek + Write, C: Read + Seek>
    AssetSerializer<'asset, 'cursor, W, C>
{
    pub fn new(asset: &'asset Asset<C>, cursor: &'cursor mut W) -> Self {
        AssetSerializer {
            asset,
            cursor,
            cached_parent_info: None,
        }
    }
}

impl<'asset, 'cursor, W: Read + Seek + Write, C: Read + Seek> AssetTrait
    for AssetSerializer<'asset, 'cursor, W, C>
{
    fn get_custom_version<T>(&self) -> CustomVersion
    where
        T: CustomVersionTrait + Into<i32>,
    {
        self.asset.get_custom_version::<T>()
    }

    fn position(&mut self) -> u64 {
        self.cursor.stream_position().unwrap_or_default()
    }

    fn set_position(&mut self, pos: u64) {
        self.cursor.seek(SeekFrom::Start(pos)).unwrap_or_default();
    }

    fn seek(&mut self, style: SeekFrom) -> io::Result<u64> {
        self.cursor.seek(style)
    }

    fn get_name_map_index_list(&self) -> &[String] {
        self.asset.get_name_map_index_list()
    }

    fn get_name_reference(&self, index: i32) -> String {
        self.asset.get_name_reference(index)
    }

    fn get_array_struct_type_override(&self) -> &IndexedMap<String, String> {
        self.asset.get_array_struct_type_override()
    }

    fn get_map_key_override(&self) -> &IndexedMap<String, String> {
        self.asset.get_map_key_override()
    }

    fn get_map_value_override(&self) -> &IndexedMap<String, String> {
        self.asset.get_map_value_override()
    }

    fn get_parent_class(&self) -> Option<ParentClassInfo> {
        self.asset.get_parent_class()
    }

    fn get_parent_class_cached(&mut self) -> Option<&ParentClassInfo> {
        if let Some(ref cached_info) = self.cached_parent_info {
            return Some(cached_info);
        }

        self.cached_parent_info = self.get_parent_class();
        self.cached_parent_info.as_ref()
    }

    #[inline(always)]
    fn get_engine_version(&self) -> EngineVersion {
        self.asset.get_engine_version()
    }

    #[inline(always)]
    fn get_object_version(&self) -> ObjectVersion {
        self.asset.get_object_version()
    }

    #[inline(always)]
    fn get_object_version_ue5(&self) -> ObjectVersionUE5 {
        self.asset.get_object_version_ue5()
    }

    fn get_import(&self, index: PackageIndex) -> Option<&Import> {
        self.asset.get_import(index)
    }

    fn get_export_class_type(&self, index: PackageIndex) -> Option<FName> {
        self.asset.get_export_class_type(index)
    }

    fn add_fname(&mut self, _value: &str) -> FName {
        // todo: assetserializer should never add fname?
        panic!("AssetSerializer added fname");
    }

    fn add_fname_with_number(&mut self, _value: &str, _number: i32) -> FName {
        // todo: assetserializer should never add fname?
        panic!("AssetSerializer added fname");
    }

    fn get_mappings(&self) -> Option<&Usmap> {
        self.asset.get_mappings()
    }

    fn has_unversioned_properties(&self) -> bool {
        self.asset.has_unversioned_properties()
    }
}

impl<'asset, 'cursor, W: Seek + Read + Write, C: Read + Seek> AssetWriter
    for AssetSerializer<'asset, 'cursor, W, C>
{
    fn write_property_guid(&mut self, guid: &Option<Guid>) -> Result<(), Error> {
        if self.asset.object_version >= ObjectVersion::VER_UE4_PROPERTY_GUID_IN_PROPERTY_TAG {
            self.cursor.write_bool(guid.is_some())?;
            if let Some(ref data) = guid {
                self.cursor.write_all(data)?;
            }
        }
        Ok(())
    }

    fn write_fname(&mut self, fname: &FName) -> Result<(), Error> {
        self.cursor.write_i32::<LittleEndian>(
            self.asset
                .search_name_reference(&fname.content)
                .ok_or_else(|| {
                    Error::no_data(format!(
                        "name reference for {} not found, you might want to rebuild the name map",
                        fname.content.to_owned()
                    ))
                })?,
        )?;
        self.cursor.write_i32::<LittleEndian>(fname.index)?;
        Ok(())
    }

    fn write_u8(&mut self, value: u8) -> io::Result<()> {
        self.cursor.write_u8(value)
    }

    fn write_i8(&mut self, value: i8) -> io::Result<()> {
        self.cursor.write_i8(value)
    }

    fn write_u16<T: byteorder::ByteOrder>(&mut self, value: u16) -> io::Result<()> {
        self.cursor.write_u16::<T>(value)
    }

    fn write_i16<T: byteorder::ByteOrder>(&mut self, value: i16) -> io::Result<()> {
        self.cursor.write_i16::<T>(value)
    }

    fn write_u32<T: byteorder::ByteOrder>(&mut self, value: u32) -> io::Result<()> {
        self.cursor.write_u32::<T>(value)
    }

    fn write_i32<T: byteorder::ByteOrder>(&mut self, value: i32) -> io::Result<()> {
        self.cursor.write_i32::<T>(value)
    }

    fn write_u64<T: byteorder::ByteOrder>(&mut self, value: u64) -> io::Result<()> {
        self.cursor.write_u64::<T>(value)
    }

    fn write_i64<T: byteorder::ByteOrder>(&mut self, value: i64) -> io::Result<()> {
        self.cursor.write_i64::<T>(value)
    }

    fn write_f32<T: byteorder::ByteOrder>(&mut self, value: f32) -> io::Result<()> {
        self.cursor.write_f32::<T>(value)
    }

    fn write_f64<T: byteorder::ByteOrder>(&mut self, value: f64) -> io::Result<()> {
        self.cursor.write_f64::<T>(value)
    }

    fn write_fstring(&mut self, value: Option<&str>) -> Result<usize, Error> {
        Ok(self.cursor.write_fstring(value)?)
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        self.cursor.write_all(buf)
    }

    fn write_bool(&mut self, value: bool) -> io::Result<()> {
        self.cursor.write_bool(value)
    }

    fn generate_unversioned_header(
        &mut self,
        properties: &[properties::Property],
        parent_name: &FName,
    ) -> Result<Option<(UnversionedHeader, Vec<properties::Property>)>, Error> {
        self.asset
            .generate_unversioned_header(properties, parent_name)
    }
}

/// FName collector is used for rebuilding the name map
/// This is useful when it's hard to keep track of asset changes
/// And you want to ensure the name map contains all the new FNames
struct FNameCollector<'asset, C: Read + Seek> {
    /// Asset
    asset: &'asset Asset<C>,
    /// Position
    position: u64,
    /// Name map index list
    name_map_index_list: Vec<String>,
    /// Name map lookup
    name_map_lookup: IndexedMap<u64, i32>,
    /// Cached parent info
    cached_parent_info: Option<ParentClassInfo>,
}

impl<'asset, C: Read + Seek> FNameCollector<'asset, C> {
    /// Create a new `FNameCollector` instance
    pub fn new(asset: &'asset Asset<C>) -> Self {
        FNameCollector {
            asset,
            position: 0,
            name_map_index_list: asset.get_name_map_index_list().to_vec(),
            name_map_lookup: asset.name_map_lookup.clone(),
            cached_parent_info: None,
        }
    }

    /// Search an FName reference
    pub fn search_name_reference(&self, name: &String) -> Option<i32> {
        let mut s = DefaultHasher::new();
        name.hash(&mut s);

        self.name_map_lookup.get_by_key(&s.finish()).copied()
    }

    /// Add an FName reference
    pub fn add_name_reference(&mut self, name: String, force_add_duplicates: bool) -> i32 {
        if !force_add_duplicates {
            let existing = self.search_name_reference(&name);
            if let Some(existing) = existing {
                return existing;
            }
        }

        let mut s = DefaultHasher::new();
        name.hash(&mut s);

        let hash = s.finish();
        self.name_map_index_list.push(name.clone());
        self.name_map_lookup
            .insert(hash, (self.name_map_index_list.len() - 1) as i32);
        (self.name_map_lookup.len() - 1) as i32
    }
}

impl<'asset, C: Read + Seek> AssetTrait for FNameCollector<'asset, C> {
    fn get_custom_version<T>(&self) -> CustomVersion
    where
        T: CustomVersionTrait + Into<i32>,
    {
        self.asset.get_custom_version::<T>()
    }

    fn position(&mut self) -> u64 {
        self.position
    }

    fn set_position(&mut self, position: u64) {
        self.position = position;
    }

    fn seek(&mut self, style: SeekFrom) -> io::Result<u64> {
        match style {
            SeekFrom::Start(e) => self.position = e,
            SeekFrom::Current(e) => self.position += e as u64,
            SeekFrom::End(_) => {}
        };

        Ok(self.position)
    }

    fn add_fname(&mut self, value: &str) -> FName {
        let name = FName::from_slice(value);
        self.add_name_reference(name.content.clone(), false);
        name
    }

    fn add_fname_with_number(&mut self, value: &str, number: i32) -> FName {
        let name = FName::new(value.to_string(), number);
        self.add_name_reference(value.to_string(), false);
        name
    }

    fn get_name_map_index_list(&self) -> &[String] {
        &self.name_map_index_list
    }

    fn get_name_reference(&self, index: i32) -> String {
        self.asset.get_name_reference(index)
    }

    fn get_array_struct_type_override(&self) -> &IndexedMap<String, String> {
        self.asset.get_array_struct_type_override()
    }

    fn get_map_key_override(&self) -> &IndexedMap<String, String> {
        self.asset.get_map_key_override()
    }

    fn get_map_value_override(&self) -> &IndexedMap<String, String> {
        self.asset.get_map_value_override()
    }

    fn get_parent_class(&self) -> Option<ParentClassInfo> {
        self.asset.get_parent_class()
    }

    fn get_parent_class_cached(&mut self) -> Option<&ParentClassInfo> {
        if let Some(ref cached_info) = self.cached_parent_info {
            return Some(cached_info);
        }

        self.cached_parent_info = self.get_parent_class();
        self.cached_parent_info.as_ref()
    }

    #[inline(always)]
    fn get_engine_version(&self) -> EngineVersion {
        self.asset.get_engine_version()
    }

    #[inline(always)]
    fn get_object_version(&self) -> ObjectVersion {
        self.asset.get_object_version()
    }

    #[inline(always)]
    fn get_object_version_ue5(&self) -> ObjectVersionUE5 {
        self.asset.get_object_version_ue5()
    }

    fn get_mappings(&self) -> Option<&Usmap> {
        self.asset.get_mappings()
    }

    fn get_import(&self, index: PackageIndex) -> Option<&Import> {
        self.asset.get_import(index)
    }

    fn get_export_class_type(&self, index: PackageIndex) -> Option<FName> {
        self.asset.get_export_class_type(index)
    }

    fn has_unversioned_properties(&self) -> bool {
        self.asset.has_unversioned_properties()
    }
}

impl<'asset, C: Read + Seek> AssetWriter for FNameCollector<'asset, C> {
    fn write_property_guid(&mut self, guid: &Option<Guid>) -> Result<(), Error> {
        if self.asset.object_version >= ObjectVersion::VER_UE4_PROPERTY_GUID_IN_PROPERTY_TAG {
            self.position += size_of::<bool>() as u64;
            if let Some(ref data) = guid {
                self.position += data.len() as u64;
            }
        }
        Ok(())
    }

    fn write_fname(&mut self, fname: &FName) -> Result<(), Error> {
        self.position += size_of::<u32>() as u64 * 2;
        if self.search_name_reference(&fname.content).is_none() {
            self.add_name_reference(fname.content.clone(), false);
        }
        Ok(())
    }

    fn write_u8(&mut self, _: u8) -> io::Result<()> {
        self.position += size_of::<u8>() as u64;
        Ok(())
    }

    fn write_i8(&mut self, _: i8) -> io::Result<()> {
        self.position += size_of::<i8>() as u64;
        Ok(())
    }

    fn write_u16<T: byteorder::ByteOrder>(&mut self, _: u16) -> io::Result<()> {
        self.position += size_of::<u16>() as u64;
        Ok(())
    }

    fn write_i16<T: byteorder::ByteOrder>(&mut self, _: i16) -> io::Result<()> {
        self.position += size_of::<i16>() as u64;
        Ok(())
    }

    fn write_u32<T: byteorder::ByteOrder>(&mut self, _: u32) -> io::Result<()> {
        self.position += size_of::<u32>() as u64;
        Ok(())
    }

    fn write_i32<T: byteorder::ByteOrder>(&mut self, _: i32) -> io::Result<()> {
        self.position += size_of::<i32>() as u64;
        Ok(())
    }

    fn write_u64<T: byteorder::ByteOrder>(&mut self, _: u64) -> io::Result<()> {
        self.position += size_of::<u64>() as u64;
        Ok(())
    }

    fn write_i64<T: byteorder::ByteOrder>(&mut self, _: i64) -> io::Result<()> {
        self.position += size_of::<i64>() as u64;
        Ok(())
    }

    fn write_f32<T: byteorder::ByteOrder>(&mut self, _: f32) -> io::Result<()> {
        self.position += size_of::<f32>() as u64;
        Ok(())
    }

    fn write_f64<T: byteorder::ByteOrder>(&mut self, _: f64) -> io::Result<()> {
        self.position += size_of::<f64>() as u64;
        Ok(())
    }

    fn write_fstring(&mut self, string: Option<&str>) -> Result<usize, Error> {
        let length = if let Some(string) = string {
            let is_unicode = string.len() != string.chars().count();

            if is_unicode {
                let utf16 = string.encode_utf16().collect::<Vec<_>>();
                size_of::<i32>() + utf16.len() * 2 /* multiplying by 2 to get size in bytes */
            } else {
                let bytes = string.as_bytes();
                size_of::<i32>() + bytes.len() + 1
            }
        } else {
            size_of::<i32>()
        };

        self.position += length as u64;
        Ok(length)
    }

    fn write_all(&mut self, data: &[u8]) -> io::Result<()> {
        self.position += data.len() as u64;
        Ok(())
    }

    fn write_bool(&mut self, _: bool) -> io::Result<()> {
        self.position += size_of::<u8>() as u64;
        Ok(())
    }

    fn generate_unversioned_header(
        &mut self,
        properties: &[properties::Property],
        parent_name: &FName,
    ) -> Result<Option<(UnversionedHeader, Vec<Property>)>, Error> {
        self.asset
            .generate_unversioned_header(properties, parent_name)
    }
}

impl<C: Read + Seek> AssetTrait for Asset<C> {
    fn get_custom_version<T>(&self) -> CustomVersion
    where
        T: CustomVersionTrait + Into<i32>,
    {
        self.custom_versions
            .iter()
            .find(|e| {
                e.friendly_name
                    .as_ref()
                    .map(|name| name == T::FRIENDLY_NAME)
                    .unwrap_or(false)
            })
            .cloned()
            .unwrap_or_else(|| CustomVersion::new(T::GUID, 0))
    }

    fn position(&mut self) -> u64 {
        self.cursor.stream_position().unwrap_or_default()
    }

    fn set_position(&mut self, pos: u64) {
        self.cursor.seek(SeekFrom::Start(pos)).unwrap_or_default();
    }

    fn seek(&mut self, style: SeekFrom) -> io::Result<u64> {
        self.cursor.seek(style)
    }

    fn add_fname(&mut self, value: &str) -> FName {
        let name = FName::new(value.to_string(), 0);
        self.add_name_reference(value.to_string(), false);
        name
    }

    fn add_fname_with_number(&mut self, value: &str, number: i32) -> FName {
        let name = FName::new(value.to_string(), number);
        self.add_name_reference(value.to_string(), false);
        name
    }

    fn get_name_map_index_list(&self) -> &[String] {
        self.get_name_map_index_list()
    }

    fn get_name_reference(&self, index: i32) -> String {
        self.get_name_reference(index)
    }

    fn get_array_struct_type_override(&self) -> &IndexedMap<String, String> {
        &self.array_struct_type_override
    }

    fn get_map_key_override(&self) -> &IndexedMap<String, String> {
        &self.map_key_override
    }

    fn get_map_value_override(&self) -> &IndexedMap<String, String> {
        &self.map_value_override
    }

    fn get_parent_class(&self) -> Option<ParentClassInfo> {
        self.get_parent_class()
    }

    fn get_parent_class_cached(&mut self) -> Option<&ParentClassInfo> {
        self.get_parent_class_cached()
    }

    #[inline(always)]
    fn get_engine_version(&self) -> EngineVersion {
        guess_engine_version(
            self.object_version,
            self.object_version_ue5,
            &self.custom_versions,
        )
    }

    #[inline(always)]
    fn get_object_version(&self) -> ObjectVersion {
        self.object_version
    }

    #[inline(always)]
    fn get_object_version_ue5(&self) -> ObjectVersionUE5 {
        self.object_version_ue5
    }

    fn get_import(&self, index: PackageIndex) -> Option<&Import> {
        if !index.is_import() {
            return None;
        }

        let index = -index.index - 1;
        if index < 0 || index > self.imports.len() as i32 {
            return None;
        }

        Some(&self.imports[index as usize])
    }

    fn get_export_class_type(&self, index: PackageIndex) -> Option<FName> {
        match index.is_import() {
            true => self.get_import(index).map(|e| e.object_name.clone()),
            false => Some(FName::new(index.index.to_string(), 0)),
        }
    }

    fn get_mappings(&self) -> Option<&Usmap> {
        self.mappings.as_ref()
    }

    fn has_unversioned_properties(&self) -> bool {
        self.package_flags
            .contains(EPackageFlags::PKG_UNVERSIONED_PROPERTIES)
    }
}

impl<C: Read + Seek> AssetReader for Asset<C> {
    fn read_property_guid(&mut self) -> Result<Option<Guid>, Error> {
        if self.object_version >= ObjectVersion::VER_UE4_PROPERTY_GUID_IN_PROPERTY_TAG {
            let has_property_guid = self.cursor.read_bool()?;
            if has_property_guid {
                let mut guid = [0u8; 16];
                self.cursor.read_exact(&mut guid)?;
                return Ok(Some(guid));
            }
        }
        Ok(None)
    }

    fn read_fname(&mut self) -> Result<FName, Error> {
        let name_map_pointer = self.cursor.read_i32::<LittleEndian>()?;
        let number = self.cursor.read_i32::<LittleEndian>()?;

        if name_map_pointer < 0 || name_map_pointer >= self.name_map_index_list.len() as i32 {
            return Err(Error::fname(
                name_map_pointer,
                self.name_map_index_list.len(),
            ));
        }

        Ok(FName::new(
            self.get_name_reference(name_map_pointer),
            number,
        ))
    }

    fn read_array_with_length<T>(
        &mut self,
        length: i32,
        getter: impl Fn(&mut Self) -> Result<T, Error>,
    ) -> Result<Vec<T>, Error> {
        let mut array = Vec::with_capacity(length as usize);
        for _ in 0..length {
            array.push(getter(self)?);
        }
        Ok(array)
    }

    fn read_array<T>(
        &mut self,
        getter: impl Fn(&mut Self) -> Result<T, Error>,
    ) -> Result<Vec<T>, Error> {
        let length = self.cursor.read_i32::<LittleEndian>()?;
        self.read_array_with_length(length, getter)
    }

    fn read_u8(&mut self) -> io::Result<u8> {
        self.cursor.read_u8()
    }

    fn read_i8(&mut self) -> io::Result<i8> {
        self.cursor.read_i8()
    }

    fn read_u16<T: byteorder::ByteOrder>(&mut self) -> io::Result<u16> {
        self.cursor.read_u16::<T>()
    }

    fn read_i16<T: byteorder::ByteOrder>(&mut self) -> io::Result<i16> {
        self.cursor.read_i16::<T>()
    }

    fn read_u32<T: byteorder::ByteOrder>(&mut self) -> io::Result<u32> {
        self.cursor.read_u32::<T>()
    }

    fn read_i32<T: byteorder::ByteOrder>(&mut self) -> io::Result<i32> {
        self.cursor.read_i32::<T>()
    }

    fn read_u64<T: byteorder::ByteOrder>(&mut self) -> io::Result<u64> {
        self.cursor.read_u64::<T>()
    }

    fn read_i64<T: byteorder::ByteOrder>(&mut self) -> io::Result<i64> {
        self.cursor.read_i64::<T>()
    }

    fn read_f32<T: byteorder::ByteOrder>(&mut self) -> io::Result<f32> {
        self.cursor.read_f32::<T>()
    }

    fn read_f64<T: byteorder::ByteOrder>(&mut self) -> io::Result<f64> {
        self.cursor.read_f64::<T>()
    }

    fn read_fstring(&mut self) -> Result<Option<String>, Error> {
        Ok(self.cursor.read_fstring()?)
    }

    fn read_exact(&mut self, buf: &mut [u8]) -> io::Result<()> {
        self.cursor.read_exact(buf)
    }

    fn read_bool(&mut self) -> io::Result<bool> {
        self.cursor.read_bool()
    }
}

impl<'a, C: Read + Seek> Asset<C> {
    /// Create an asset from a binary file
    pub fn new(
        asset_data: C,
        bulk_data: Option<C>,
        engine_version: EngineVersion,
    ) -> Result<Self, Error> {
        let mut asset = Asset {
            use_separate_bulk_data_files: bulk_data.is_some(),
            cursor: Chain::new(asset_data, bulk_data),
            info: String::from("Serialized with unrealmodding/uasset"),
            object_version: ObjectVersion::UNKNOWN,
            object_version_ue5: ObjectVersionUE5::UNKNOWN,
            legacy_file_version: 0,
            unversioned: true,
            file_license_version: 0,
            custom_versions: Vec::new(),
            generations: Vec::new(),
            package_guid: [0; 16],
            engine_version_recorded: FEngineVersion::unknown(),
            engine_version_compatible: FEngineVersion::unknown(),
            chunk_ids: Vec::new(),
            package_flags: EPackageFlags::PKG_NONE,
            package_source: 0,
            folder_name: String::from(""),
            header_offset: 0,
            name_count: 0,
            name_offset: 0,
            gatherable_text_data_count: 0,
            gatherable_text_data_offset: 0,
            export_count: 0,
            export_offset: 0,
            import_count: 0,
            import_offset: 0,
            depends_offset: 0,
            soft_package_reference_count: 0,
            soft_package_reference_offset: 0,
            searchable_names_offset: 0,
            thumbnail_table_offset: 0,
            compression_flags: 0,
            asset_registry_data_offset: 0,
            bulk_data_start_offset: 0,
            world_tile_info_offset: 0,
            preload_dependency_count: 0,
            preload_dependency_offset: 0,

            override_name_map_hashes: IndexedMap::new(),
            name_map_index_list: Vec::new(),
            name_map_lookup: IndexedMap::new(),
            imports: Vec::new(),
            exports: Vec::new(),
            depends_map: None,
            soft_package_reference_list: None,
            world_tile_info: None,

            array_struct_type_override: IndexedMap::from([(
                "Keys".to_string(),
                "RichCurveKey".to_string(),
            )]),

            map_key_override: IndexedMap::from([
                ("PlayerCharacterIDs".to_string(), "Guid".to_string()),
                (
                    "m_PerConditionValueToNodeMap".to_string(),
                    "Guid".to_string(),
                ),
                ("BindingIdToReferences".to_string(), "Guid".to_string()),
                (
                    "UserParameterRedirects".to_string(),
                    "NiagaraVariable".to_string(),
                ),
                (
                    "Tracks".to_string(),
                    "MovieSceneTrackIdentifier".to_string(),
                ),
                (
                    "SubSequences".to_string(),
                    "MovieSceneSequenceID".to_string(),
                ),
                ("Hierarchy".to_string(), "MovieSceneSequenceID".to_string()),
                (
                    "TrackSignatureToTrackIdentifier".to_string(),
                    "Guid".to_string(),
                ),
                ("ItemsToRefund".to_string(), "Guid".to_string()),
                ("PlayerCharacterIDMap".to_string(), "Guid".to_string()),
            ]),
            map_value_override: IndexedMap::from([
                ("ColorDatabase".to_string(), "LinearColor".to_string()),
                (
                    "UserParameterRedirects".to_string(),
                    "NiagaraVariable".to_string(),
                ),
                (
                    "TrackSignatureToTrackIdentifier".to_string(),
                    "MovieSceneTrackIdentifier".to_string(),
                ),
                (
                    "RainChanceMinMaxPerWeatherState".to_string(),
                    "FloatRange".to_string(),
                ),
            ]),
            mappings: None,
            parent_class: None,
        };
        asset.set_engine_version(engine_version);
        asset.parse_data()?;
        Ok(asset)
    }

    /// Set asset engine version
    fn set_engine_version(&mut self, engine_version: EngineVersion) {
        if engine_version == EngineVersion::UNKNOWN {
            return;
        }

        let (object_version, object_version_ue5) = get_object_versions(engine_version);

        self.object_version = object_version;
        self.object_version_ue5 = object_version_ue5;
        self.custom_versions = CustomVersion::get_default_custom_version_container(engine_version);
    }

    /// Parse asset header
    fn parse_header(&mut self) -> Result<(), Error> {
        // reuseable buffers for reading

        // seek to start
        self.cursor.rewind()?;

        // read and check magic
        if self.cursor.read_u32::<BigEndian>()? != UE4_ASSET_MAGIC {
            return Err(Error::invalid_file(
                "File is not a valid uasset file".to_string(),
            ));
        }

        // read legacy version
        self.legacy_file_version = self.cursor.read_i32::<LittleEndian>()?;
        if self.legacy_file_version != -4 {
            // LegacyUE3Version for backwards-compatibility with UE3 games: always 864 in versioned assets, always 0 in unversioned assets
            self.cursor.read_exact(&mut [0u8; 4])?;
        }

        // read unreal version
        let file_version = self.cursor.read_i32::<LittleEndian>()?.try_into()?;

        self.unversioned = file_version == ObjectVersion::UNKNOWN;

        if self.unversioned {
            if self.object_version == ObjectVersion::UNKNOWN {
                return Err(Error::invalid_file("Cannot begin serialization of an unversioned asset before an engine version is manually specified".to_string()));
            }
        } else {
            self.object_version = file_version;
        }

        // read file license version
        self.file_license_version = self.cursor.read_i32::<LittleEndian>()?;

        // read custom versions container
        if self.legacy_file_version <= -2 {
            // TODO: support for enum-based custom versions

            // read custom version count
            let custom_versions_count = self.cursor.read_i32::<LittleEndian>()?;

            for _ in 0..custom_versions_count {
                // read guid
                let mut guid = [0u8; 16];
                self.cursor.read_exact(&mut guid)?;
                // read version
                let version = self.cursor.read_i32::<LittleEndian>()?;

                self.custom_versions.push(CustomVersion::new(guid, version));
            }
        }

        // read header offset
        self.header_offset = self.cursor.read_i32::<LittleEndian>()?;

        // read folder name
        self.folder_name = self
            .cursor
            .read_fstring()?
            .ok_or_else(|| Error::no_data("folder_name is None".to_string()))?;

        // read package flags
        self.package_flags = EPackageFlags::from_bits(self.cursor.read_u32::<LittleEndian>()?)
            .ok_or_else(|| Error::invalid_file("Invalid package flags".to_string()))?;

        // read name count and offset
        self.name_count = self.cursor.read_i32::<LittleEndian>()?;
        self.name_offset = self.cursor.read_i32::<LittleEndian>()?;
        // read text gatherable data
        if self.object_version >= ObjectVersion::VER_UE4_SERIALIZE_TEXT_IN_PACKAGES {
            self.gatherable_text_data_count = self.cursor.read_i32::<LittleEndian>()?;
            self.gatherable_text_data_offset = self.cursor.read_i32::<LittleEndian>()?;
        }

        // read count and offset for exports, imports, depends, soft package references, searchable names, thumbnail table
        self.export_count = self.cursor.read_i32::<LittleEndian>()?;
        self.export_offset = self.cursor.read_i32::<LittleEndian>()?;
        self.import_count = self.cursor.read_i32::<LittleEndian>()?;
        self.import_offset = self.cursor.read_i32::<LittleEndian>()?;
        self.depends_offset = self.cursor.read_i32::<LittleEndian>()?;
        if self.object_version >= ObjectVersion::VER_UE4_ADD_STRING_ASSET_REFERENCES_MAP {
            self.soft_package_reference_count = self.cursor.read_i32::<LittleEndian>()?;
            self.soft_package_reference_offset = self.cursor.read_i32::<LittleEndian>()?;
        }
        if self.object_version >= ObjectVersion::VER_UE4_ADDED_SEARCHABLE_NAMES {
            self.searchable_names_offset = self.cursor.read_i32::<LittleEndian>()?;
        }
        self.thumbnail_table_offset = self.cursor.read_i32::<LittleEndian>()?;

        // read guid
        self.cursor.read_exact(&mut self.package_guid)?;

        // raed generations
        let generations_count = self.cursor.read_i32::<LittleEndian>()?;
        for _ in 0..generations_count {
            let export_count = self.cursor.read_i32::<LittleEndian>()?;
            let name_count = self.cursor.read_i32::<LittleEndian>()?;
            self.generations.push(GenerationInfo {
                export_count,
                name_count,
            });
        }

        // read advanced engine version
        if self.object_version >= ObjectVersion::VER_UE4_ENGINE_VERSION_OBJECT {
            self.engine_version_recorded = FEngineVersion::read(&mut self.cursor)?;
        } else {
            self.engine_version_recorded =
                FEngineVersion::new(4, 0, 0, self.cursor.read_u32::<LittleEndian>()?, None);
        }
        if self.object_version
            >= ObjectVersion::VER_UE4_PACKAGE_SUMMARY_HAS_COMPATIBLE_ENGINE_VERSION
        {
            self.engine_version_compatible = FEngineVersion::read(&mut self.cursor)?;
        } else {
            self.engine_version_compatible = self.engine_version_recorded.clone();
        }

        // read compression data
        self.compression_flags = self.cursor.read_u32::<LittleEndian>()?;
        let compression_block_count = self.cursor.read_u32::<LittleEndian>()?;
        if compression_block_count > 0 {
            return Err(Error::invalid_file(
                "Compression block count is not zero".to_string(),
            ));
        }

        self.package_source = self.cursor.read_u32::<LittleEndian>()?;

        // some other old unsupported stuff
        let additional_to_cook = self.cursor.read_i32::<LittleEndian>()?;
        if additional_to_cook != 0 {
            return Err(Error::invalid_file(
                "Additional to cook is not zero".to_string(),
            ));
        }
        if self.legacy_file_version > -7 {
            let texture_allocations_count = self.cursor.read_i32::<LittleEndian>()?;
            if texture_allocations_count != 0 {
                return Err(Error::invalid_file(
                    "Texture allocations count is not zero".to_string(),
                ));
            }
        }

        self.asset_registry_data_offset = self.cursor.read_i32::<LittleEndian>()?;
        self.bulk_data_start_offset = self.cursor.read_i64::<LittleEndian>()?;

        if self.object_version >= ObjectVersion::VER_UE4_WORLD_LEVEL_INFO {
            self.world_tile_info_offset = self.cursor.read_i32::<LittleEndian>()?;
        }

        if self.object_version >= ObjectVersion::VER_UE4_CHANGED_CHUNKID_TO_BE_AN_ARRAY_OF_CHUNKIDS
        {
            let chunk_id_count = self.cursor.read_i32::<LittleEndian>()?;

            for _ in 0..chunk_id_count {
                let chunk_id = self.cursor.read_i32::<LittleEndian>()?;
                self.chunk_ids.push(chunk_id);
            }
        } else if self.object_version
            >= ObjectVersion::VER_UE4_ADDED_CHUNKID_TO_ASSETDATA_AND_UPACKAGE
        {
            self.chunk_ids = vec![];
            self.chunk_ids[0] = self.cursor.read_i32::<LittleEndian>()?;
        }

        if self.object_version >= ObjectVersion::VER_UE4_PRELOAD_DEPENDENCIES_IN_COOKED_EXPORTS {
            self.preload_dependency_count = self.cursor.read_i32::<LittleEndian>()?;
            self.preload_dependency_offset = self.cursor.read_i32::<LittleEndian>()?;
        }
        Ok(())
    }

    /// Asset data length
    pub fn data_length(&mut self) -> u64 {
        let pos = self.cursor.stream_position().unwrap_or_default();
        let len = self.cursor.seek(SeekFrom::End(0)).unwrap_or_default();
        self.cursor.seek(SeekFrom::Start(pos)).unwrap_or_default();
        len
    }

    /// Read a name map string
    fn read_name_map_string(&mut self) -> Result<(u32, String), Error> {
        let s = self
            .cursor
            .read_fstring()?
            .ok_or_else(|| Error::no_data("name_map_string is None".to_string()))?;
        let mut hashes = 0;
        if self.object_version >= ObjectVersion::VER_UE4_NAME_HASHES_SERIALIZED && !s.is_empty() {
            hashes = self.cursor.read_u32::<LittleEndian>()?;
        }
        Ok((hashes, s))
    }

    /// Search an FName reference
    pub fn search_name_reference(&self, name: &String) -> Option<i32> {
        let mut s = DefaultHasher::new();
        name.hash(&mut s);

        self.name_map_lookup.get_by_key(&s.finish()).copied()
    }

    /// Add an FName reference
    pub fn add_name_reference(&mut self, name: String, force_add_duplicates: bool) -> i32 {
        if !force_add_duplicates {
            let existing = self.search_name_reference(&name);
            if let Some(existing) = existing {
                return existing;
            }
        }

        let mut s = DefaultHasher::new();
        name.hash(&mut s);

        let hash = s.finish();
        self.name_map_index_list.push(name.clone());
        self.name_map_lookup
            .insert(hash, (self.name_map_index_list.len() - 1) as i32);
        (self.name_map_lookup.len() - 1) as i32
    }

    /// Get all FNames
    pub fn get_name_map_index_list(&self) -> &[String] {
        &self.name_map_index_list
    }

    /// Get a name reference by an FName map index
    pub fn get_name_reference(&self, index: i32) -> String {
        if index < 0 {
            return (-index).to_string(); // is this right even?
        }
        if index >= self.name_map_index_list.len() as i32 {
            return index.to_string();
        }
        self.name_map_index_list[index as usize].to_owned()
    }

    /// Get a mutable name reference by an FName map index
    pub fn get_name_reference_mut(&mut self, index: i32) -> &mut String {
        &mut self.name_map_index_list[index as usize]
    }

    /// Add an `FName`
    pub fn add_fname(&mut self, slice: &str) -> FName {
        let name = FName::from_slice(slice);
        self.add_name_reference(name.content.clone(), false);
        name
    }

    /// Add an `Import`
    pub fn add_import(&mut self, import: Import) -> PackageIndex {
        let index = -(self.imports.len() as i32) - 1;
        let import = import;
        self.imports.push(import);
        PackageIndex::new(index)
    }

    /// Searches for and returns this asset's CLassExport, if one exists
    pub fn get_class_export(&self) -> Option<&ClassExport> {
        self.exports
            .iter()
            .find_map(|e| cast!(Export, ClassExport, e))
    }

    /// Gets parent class info of this asset, if it exists
    pub fn get_parent_class(&self) -> Option<ParentClassInfo> {
        let class_export = self.get_class_export()?;

        let parent_class_import = self.get_import(class_export.struct_export.super_struct)?;
        let outer_parent_import = self.get_import(parent_class_import.outer_index)?;

        Some(ParentClassInfo {
            parent_class_path: parent_class_import.object_name.clone(),
            parent_class_export_name: outer_parent_import.object_name.clone(),
        })
    }

    /// Gets parent class info of this asset from cache if it exists,
    /// if it doesn't exist in cache, tries to compute it.
    pub fn get_parent_class_cached(&mut self) -> Option<&ParentClassInfo> {
        if let Some(ref parent_class) = self.parent_class {
            return Some(parent_class);
        }

        self.parent_class = self.get_parent_class();
        self.parent_class.as_ref()
    }

    /// Find an import
    pub fn find_import(
        &self,
        class_package: &FName,
        class_name: &FName,
        outer_index: PackageIndex,
        object_name: &FName,
    ) -> Option<i32> {
        for i in 0..self.imports.len() {
            let import = &self.imports[i];
            if import.class_package == *class_package
                && import.class_name == *class_name
                && import.outer_index == outer_index
                && import.object_name == *object_name
            {
                return Some(-(i as i32) - 1);
            }
        }
        None
    }

    /// Find an import without specifying outer index
    pub fn find_import_no_index(
        &self,
        class_package: &FName,
        class_name: &FName,
        object_name: &FName,
    ) -> Option<i32> {
        for i in 0..self.imports.len() {
            let import = &self.imports[i];
            if import.class_package == *class_package
                && import.class_name == *class_name
                && import.object_name == *object_name
            {
                return Some(-(i as i32) - 1);
            }
        }
        None
    }

    /// Get an export
    pub fn get_export(&'a self, index: PackageIndex) -> Option<&'a Export> {
        if !index.is_export() {
            return None;
        }

        let index = index.index - 1;

        if index < 0 || index >= self.exports.len() as i32 {
            return None;
        }

        Some(&self.exports[index as usize])
    }

    /// Get a mutable export reference
    pub fn get_export_mut(&'a mut self, index: PackageIndex) -> Option<&'a mut Export> {
        if !index.is_export() {
            return None;
        }

        let index = index.index - 1;

        if index < 0 || index >= self.exports.len() as i32 {
            return None;
        }

        Some(&mut self.exports[index as usize])
    }

    /// Parse asset data
    fn parse_data(&mut self) -> Result<(), Error> {
        self.parse_header()?;
        self.cursor.seek(SeekFrom::Start(self.name_offset as u64))?;

        for _ in 0..self.name_count {
            let name_map = self.read_name_map_string()?;
            if name_map.0 == 0 {
                self.override_name_map_hashes.insert(name_map.1.clone(), 0);
            }
            self.add_name_reference(name_map.1, true);
        }

        if self.import_offset > 0 {
            self.cursor
                .seek(SeekFrom::Start(self.import_offset as u64))?;
            for _i in 0..self.import_count {
                let import = Import::new(
                    self.read_fname()?,
                    self.read_fname()?,
                    PackageIndex::new(self.cursor.read_i32::<LittleEndian>()?),
                    self.read_fname()?,
                );
                self.imports.push(import);
            }
        }

        if self.export_offset > 0 {
            self.cursor
                .seek(SeekFrom::Start(self.export_offset as u64))?;
            for _i in 0..self.export_count {
                let mut export = BaseExport {
                    class_index: PackageIndex::new(self.cursor.read_i32::<LittleEndian>()?),
                    super_index: PackageIndex::new(self.cursor.read_i32::<LittleEndian>()?),
                    ..Default::default()
                };

                if self.object_version >= ObjectVersion::VER_UE4_TemplateIndex_IN_COOKED_EXPORTS {
                    export.template_index =
                        PackageIndex::new(self.cursor.read_i32::<LittleEndian>()?);
                }

                export.outer_index = PackageIndex::new(self.cursor.read_i32::<LittleEndian>()?);
                export.object_name = self.read_fname()?;
                export.object_flags = self.cursor.read_u32::<LittleEndian>()?;

                if self.object_version < ObjectVersion::VER_UE4_64BIT_EXPORTMAP_SERIALSIZES {
                    export.serial_size = self.cursor.read_i32::<LittleEndian>()? as i64;
                    export.serial_offset = self.cursor.read_i32::<LittleEndian>()? as i64;
                } else {
                    export.serial_size = self.cursor.read_i64::<LittleEndian>()?;
                    export.serial_offset = self.cursor.read_i64::<LittleEndian>()?;
                }

                export.forced_export = self.cursor.read_i32::<LittleEndian>()? == 1;
                export.not_for_client = self.cursor.read_i32::<LittleEndian>()? == 1;
                export.not_for_server = self.cursor.read_i32::<LittleEndian>()? == 1;
                self.cursor.read_exact(&mut export.package_guid)?;
                export.package_flags = self.cursor.read_u32::<LittleEndian>()?;

                if self.object_version >= ObjectVersion::VER_UE4_LOAD_FOR_EDITOR_GAME {
                    export.not_always_loaded_for_editor_game =
                        self.cursor.read_i32::<LittleEndian>()? == 1;
                }

                if self.object_version >= ObjectVersion::VER_UE4_COOKED_ASSETS_IN_EDITOR_SUPPORT {
                    export.is_asset = self.cursor.read_i32::<LittleEndian>()? == 1;
                }

                if self.object_version
                    >= ObjectVersion::VER_UE4_PRELOAD_DEPENDENCIES_IN_COOKED_EXPORTS
                {
                    export.first_export_dependency_offset =
                        self.cursor.read_i32::<LittleEndian>()?;
                    export.serialization_before_serialization_dependencies_size =
                        self.cursor.read_i32::<LittleEndian>()?;
                    export.create_before_serialization_dependencies_size =
                        self.cursor.read_i32::<LittleEndian>()?;
                    export.serialization_before_create_dependencies_size =
                        self.cursor.read_i32::<LittleEndian>()?;
                    export.create_before_create_dependencies_size =
                        self.cursor.read_i32::<LittleEndian>()?;
                }

                self.exports.push(export.into());
            }
        }

        if self.depends_offset > 0 {
            let mut depends_map = Vec::with_capacity(self.export_count as usize);

            self.cursor
                .seek(SeekFrom::Start(self.depends_offset as u64))?;

            for _i in 0..self.export_count as usize {
                let size = self.cursor.read_i32::<LittleEndian>()?;
                let mut data: Vec<i32> = Vec::new();
                for _j in 0..size {
                    data.push(self.cursor.read_i32::<LittleEndian>()?);
                }
                depends_map.push(data);
            }
            self.depends_map = Some(depends_map);
        }

        if self.soft_package_reference_offset > 0 {
            let mut soft_package_reference_list =
                Vec::with_capacity(self.soft_package_reference_count as usize);

            self.cursor
                .seek(SeekFrom::Start(self.soft_package_reference_offset as u64))?;

            for _i in 0..self.soft_package_reference_count as usize {
                if let Some(reference) = self.cursor.read_fstring()? {
                    soft_package_reference_list.push(reference);
                }
            }
            self.soft_package_reference_list = Some(soft_package_reference_list);
        }

        // TODO: Asset registry data parsing should be here

        if self.world_tile_info_offset > 0 {
            self.cursor
                .seek(SeekFrom::Start(self.world_tile_info_offset as u64))?;
            self.world_tile_info = Some(FWorldTileInfo::new(self)?);
        }

        if self.use_separate_bulk_data_files {
            for export in &mut self.exports {
                let unk_export = export.get_base_export_mut();

                self.cursor
                    .seek(SeekFrom::Start(self.preload_dependency_offset as u64))?;
                self.cursor.seek(SeekFrom::Current(
                    unk_export.first_export_dependency_offset as i64 * size_of::<i32>() as i64,
                ))?;

                let mut serialization_before_serialization_dependencies = Vec::with_capacity(
                    unk_export.serialization_before_serialization_dependencies_size as usize,
                );
                for _ in 0..unk_export.serialization_before_serialization_dependencies_size {
                    serialization_before_serialization_dependencies
                        .push(PackageIndex::new(self.cursor.read_i32::<LittleEndian>()?));
                }
                unk_export.serialization_before_serialization_dependencies =
                    serialization_before_serialization_dependencies;

                let mut create_before_serialization_dependencies = Vec::with_capacity(
                    unk_export.create_before_serialization_dependencies_size as usize,
                );
                for _ in 0..unk_export.create_before_serialization_dependencies_size {
                    create_before_serialization_dependencies
                        .push(PackageIndex::new(self.cursor.read_i32::<LittleEndian>()?));
                }
                unk_export.create_before_serialization_dependencies =
                    create_before_serialization_dependencies;

                let mut serialization_before_create_dependencies = Vec::with_capacity(
                    unk_export.serialization_before_create_dependencies_size as usize,
                );
                for _ in 0..unk_export.serialization_before_create_dependencies_size {
                    serialization_before_create_dependencies
                        .push(PackageIndex::new(self.cursor.read_i32::<LittleEndian>()?));
                }
                unk_export.serialization_before_create_dependencies =
                    serialization_before_create_dependencies;

                let mut create_before_create_dependencies =
                    Vec::with_capacity(unk_export.create_before_create_dependencies_size as usize);
                for _ in 0..unk_export.create_before_create_dependencies_size {
                    create_before_create_dependencies
                        .push(PackageIndex::new(self.cursor.read_i32::<LittleEndian>()?));
                }
                unk_export.create_before_create_dependencies = create_before_create_dependencies;
            }
            self.cursor
                .seek(SeekFrom::Start(self.preload_dependency_offset as u64))?;
        }

        if self.header_offset > 0 && !self.exports.is_empty() {
            for i in 0..self.exports.len() {
                let base_export = match &self.exports[i] {
                    Export::BaseExport(export) => Some(export.clone()),
                    _ => None,
                };

                if let Some(base_export) = base_export {
                    let export: Result<Export, Error> = match self.read_export(&base_export, i) {
                        Ok(e) => Ok(e),
                        Err(_e) => {
                            // todo: warning?
                            self.cursor
                                .seek(SeekFrom::Start(base_export.serial_offset as u64))?;
                            Ok(RawExport::from_base(base_export, self)?.into())
                        }
                    };
                    self.exports[i] = export?;
                }
            }
        }

        Ok(())
    }

    /// Read an `Export`
    fn read_export(&mut self, base_export: &BaseExport, i: usize) -> Result<Export, Error> {
        let next_starting = match i < (self.exports.len() - 1) {
            true => match &self.exports[i + 1] {
                Export::BaseExport(next_export) => next_export.serial_offset as u64,
                _ => self.data_length() - 4,
            },
            false => self.data_length() - 4,
        };

        self.cursor
            .seek(SeekFrom::Start(base_export.serial_offset as u64))?;

        //todo: manual skips
        let export_class_type = self
            .get_export_class_type(base_export.class_index)
            .ok_or_else(|| Error::invalid_package_index("Unknown class type".to_string()))?;
        let mut export: Export = match export_class_type.content.as_str() {
            "Level" => LevelExport::from_base(base_export, self, next_starting)?.into(),
            "StringTable" => StringTableExport::from_base(base_export, self)?.into(),
            "Enum" | "UserDefinedEnum" => EnumExport::from_base(base_export, self)?.into(),
            "Function" => FunctionExport::from_base(base_export, self)?.into(),
            _ => {
                if export_class_type.content.ends_with("DataTable") {
                    DataTableExport::from_base(base_export, self)?.into()
                } else if export_class_type.content.ends_with("StringTable") {
                    StringTableExport::from_base(base_export, self)?.into()
                } else if export_class_type
                    .content
                    .ends_with("BlueprintGeneratedClass")
                {
                    let class_export = ClassExport::from_base(base_export, self)?;

                    for entry in &class_export.struct_export.loaded_properties {
                        if let FProperty::FMapProperty(map) = entry {
                            let key_override = match &*map.key_prop {
                                FProperty::FStructProperty(struct_property) => {
                                    match struct_property.struct_value.is_import() {
                                        true => self
                                            .get_import(struct_property.struct_value)
                                            .map(|e| e.object_name.content.to_owned()),
                                        false => None,
                                    }
                                }
                                _ => None,
                            };
                            if let Some(key) = key_override {
                                self.map_key_override
                                    .insert(map.generic_property.name.content.to_owned(), key);
                            }

                            let value_override = match &*map.value_prop {
                                FProperty::FStructProperty(struct_property) => {
                                    match struct_property.struct_value.is_import() {
                                        true => self
                                            .get_import(struct_property.struct_value)
                                            .map(|e| e.object_name.content.to_owned()),
                                        false => None,
                                    }
                                }
                                _ => None,
                            };

                            if let Some(value) = value_override {
                                self.map_value_override
                                    .insert(map.generic_property.name.content.to_owned(), value);
                            }
                        }
                    }
                    class_export.into()
                } else if export_class_type.content.ends_with("Property") {
                    PropertyExport::from_base(base_export, self)?.into()
                } else {
                    NormalExport::from_base(base_export, self)?.into()
                }
            }
        };

        let extras_len =
            next_starting as i64 - self.cursor.stream_position().unwrap_or_default() as i64;
        if extras_len < 0 {
            // todo: warning?

            self.cursor
                .seek(SeekFrom::Start(base_export.serial_offset as u64))?;
            return Ok(RawExport::from_base(base_export.clone(), self)?.into());
        } else if let Some(normal_export) = export.get_normal_export_mut() {
            let mut extras = vec![0u8; extras_len as usize];
            self.cursor.read_exact(&mut extras)?;
            normal_export.extras = extras;
        }

        Ok(export)
    }

    /// Write asset header
    fn write_header<Writer: AssetWriter>(
        &self,
        cursor: &mut Writer,
        asset_header: &AssetHeader,
    ) -> Result<(), Error> {
        cursor.write_u32::<BigEndian>(UE4_ASSET_MAGIC)?;
        cursor.write_i32::<LittleEndian>(self.legacy_file_version)?;

        if self.legacy_file_version != 4 {
            match self.unversioned {
                true => cursor.write_i32::<LittleEndian>(0)?,
                false => cursor.write_i32::<LittleEndian>(864)?,
            };
        }

        match self.unversioned {
            true => cursor.write_i32::<LittleEndian>(0)?,
            false => cursor.write_i32::<LittleEndian>(self.object_version as i32)?,
        };

        cursor.write_i32::<LittleEndian>(self.file_license_version)?;
        if self.legacy_file_version <= -2 {
            match self.unversioned {
                true => cursor.write_i32::<LittleEndian>(0)?,
                false => {
                    cursor.write_i32::<LittleEndian>(self.custom_versions.len() as i32)?;
                    for custom_version in &self.custom_versions {
                        cursor.write_all(&custom_version.guid)?;
                        cursor.write_i32::<LittleEndian>(custom_version.version)?;
                    }
                }
            };
        }

        cursor.write_i32::<LittleEndian>(asset_header.header_offset)?;
        cursor.write_fstring(Some(&self.folder_name))?;
        cursor.write_u32::<LittleEndian>(self.package_flags.bits())?;
        cursor.write_i32::<LittleEndian>(self.name_map_index_list.len() as i32)?;
        cursor.write_i32::<LittleEndian>(asset_header.name_offset)?;

        if self.object_version >= ObjectVersion::VER_UE4_SERIALIZE_TEXT_IN_PACKAGES {
            cursor.write_i32::<LittleEndian>(self.gatherable_text_data_count)?;
            cursor.write_i32::<LittleEndian>(self.gatherable_text_data_offset)?;
        }

        cursor.write_i32::<LittleEndian>(self.exports.len() as i32)?;
        cursor.write_i32::<LittleEndian>(asset_header.export_offset)?;
        cursor.write_i32::<LittleEndian>(self.imports.len() as i32)?;
        cursor.write_i32::<LittleEndian>(asset_header.import_offset)?;
        cursor.write_i32::<LittleEndian>(asset_header.depends_offset)?;

        if self.object_version >= ObjectVersion::VER_UE4_ADD_STRING_ASSET_REFERENCES_MAP {
            cursor.write_i32::<LittleEndian>(self.soft_package_reference_count)?;
            cursor.write_i32::<LittleEndian>(asset_header.soft_package_reference_offset)?;
        }

        if self.object_version >= ObjectVersion::VER_UE4_ADDED_SEARCHABLE_NAMES {
            cursor.write_i32::<LittleEndian>(self.searchable_names_offset)?;
        }

        cursor.write_i32::<LittleEndian>(self.thumbnail_table_offset)?;
        cursor.write_all(&self.package_guid)?;
        cursor.write_i32::<LittleEndian>(self.generations.len() as i32)?;

        for _ in 0..self.generations.len() {
            cursor.write_i32::<LittleEndian>(self.exports.len() as i32)?;
            cursor.write_i32::<LittleEndian>(self.name_map_index_list.len() as i32)?;
        }

        if self.object_version >= ObjectVersion::VER_UE4_ENGINE_VERSION_OBJECT {
            self.engine_version_recorded.write(cursor)?;
        } else {
            cursor.write_u32::<LittleEndian>(self.engine_version_recorded.build)?;
        }

        if self.object_version
            >= ObjectVersion::VER_UE4_PACKAGE_SUMMARY_HAS_COMPATIBLE_ENGINE_VERSION
        {
            self.engine_version_recorded.write(cursor)?;
        }

        cursor.write_u32::<LittleEndian>(self.compression_flags)?;
        cursor.write_i32::<LittleEndian>(0)?; // numCompressedChunks
        cursor.write_u32::<LittleEndian>(self.package_source)?;
        cursor.write_i32::<LittleEndian>(0)?; // numAdditionalPackagesToCook

        if self.legacy_file_version > -7 {
            cursor.write_i32::<LittleEndian>(0)?; // numTextureallocations
        }

        cursor.write_i32::<LittleEndian>(asset_header.asset_registry_data_offset)?;
        cursor.write_i64::<LittleEndian>(asset_header.bulk_data_start_offset)?;

        if self.object_version >= ObjectVersion::VER_UE4_WORLD_LEVEL_INFO {
            cursor.write_i32::<LittleEndian>(asset_header.world_tile_info_offset)?;
        }

        if self.object_version >= ObjectVersion::VER_UE4_CHANGED_CHUNKID_TO_BE_AN_ARRAY_OF_CHUNKIDS
        {
            cursor.write_i32::<LittleEndian>(self.chunk_ids.len() as i32)?;
            for chunk_id in &self.chunk_ids {
                cursor.write_i32::<LittleEndian>(*chunk_id)?;
            }
        } else if self.object_version
            >= ObjectVersion::VER_UE4_ADDED_CHUNKID_TO_ASSETDATA_AND_UPACKAGE
        {
            cursor.write_i32::<LittleEndian>(self.chunk_ids[0])?;
        }

        if self.object_version >= ObjectVersion::VER_UE4_PRELOAD_DEPENDENCIES_IN_COOKED_EXPORTS {
            cursor.write_i32::<LittleEndian>(asset_header.preload_dependency_count)?;
            cursor.write_i32::<LittleEndian>(asset_header.preload_dependency_offset)?;
        }

        Ok(())
    }

    /// Write `Export` header
    fn write_export_header<Writer: AssetWriter>(
        &self,
        unk: &BaseExport,
        cursor: &mut Writer,
        serial_size: i64,
        serial_offset: i64,
        first_export_dependency_offset: i32,
    ) -> Result<(), Error> {
        cursor.write_i32::<LittleEndian>(unk.class_index.index)?;
        cursor.write_i32::<LittleEndian>(unk.super_index.index)?;

        if self.object_version >= ObjectVersion::VER_UE4_TemplateIndex_IN_COOKED_EXPORTS {
            cursor.write_i32::<LittleEndian>(unk.template_index.index)?;
        }

        cursor.write_i32::<LittleEndian>(unk.outer_index.index)?;
        cursor.write_fname(&unk.object_name)?;
        cursor.write_u32::<LittleEndian>(unk.object_flags)?;

        if self.object_version < ObjectVersion::VER_UE4_64BIT_EXPORTMAP_SERIALSIZES {
            cursor.write_i32::<LittleEndian>(serial_size as i32)?;
            cursor.write_i32::<LittleEndian>(serial_offset as i32)?;
        } else {
            cursor.write_i64::<LittleEndian>(serial_size)?;
            cursor.write_i64::<LittleEndian>(serial_offset)?;
        }

        cursor.write_i32::<LittleEndian>(match unk.forced_export {
            true => 1,
            false => 0,
        })?;
        cursor.write_i32::<LittleEndian>(match unk.not_for_client {
            true => 1,
            false => 0,
        })?;
        cursor.write_i32::<LittleEndian>(match unk.not_for_server {
            true => 1,
            false => 0,
        })?;
        cursor.write_all(&unk.package_guid)?;
        cursor.write_u32::<LittleEndian>(unk.package_flags)?;

        if self.object_version >= ObjectVersion::VER_UE4_LOAD_FOR_EDITOR_GAME {
            cursor.write_i32::<LittleEndian>(match unk.not_always_loaded_for_editor_game {
                true => 1,
                false => 0,
            })?;
        }

        if self.object_version >= ObjectVersion::VER_UE4_COOKED_ASSETS_IN_EDITOR_SUPPORT {
            cursor.write_i32::<LittleEndian>(match unk.is_asset {
                true => 1,
                false => 0,
            })?;
        }

        if self.object_version >= ObjectVersion::VER_UE4_PRELOAD_DEPENDENCIES_IN_COOKED_EXPORTS {
            cursor.write_i32::<LittleEndian>(first_export_dependency_offset)?;
            cursor.write_i32::<LittleEndian>(
                unk.serialization_before_serialization_dependencies.len() as i32,
            )?;
            cursor.write_i32::<LittleEndian>(
                unk.create_before_serialization_dependencies.len() as i32
            )?;
            cursor.write_i32::<LittleEndian>(
                unk.serialization_before_create_dependencies.len() as i32
            )?;
            cursor.write_i32::<LittleEndian>(unk.create_before_create_dependencies.len() as i32)?;
        }
        Ok(())
    }

    /// Rebuild the FName map
    /// This can be used if it's too complicated to keep track of all FNames that were added into the asset
    /// This is useful when copying export from one asset into another
    /// This will automatically figure out every new FName and add them to the name map
    pub fn rebuild_name_map(&mut self) -> Result<(), Error> {
        let mut collector = FNameCollector::new(self);

        for import in &self.imports {
            collector.write_fname(&import.class_package)?;
            collector.write_fname(&import.class_name)?;
            collector.write_fname(&import.object_name)?;
        }

        for export in &self.exports {
            self.write_export_header(export.get_base_export(), &mut collector, 0, 0, 0)?;
            export.write(&mut collector)?;
        }

        let name_map_index_list = collector.name_map_index_list;
        let name_map_lookup = collector.name_map_lookup;

        self.name_map_index_list = name_map_index_list;
        self.name_map_lookup = name_map_lookup;

        Ok(())
    }

    /// Write asset data
    pub fn write_data<W: Read + Seek + Write>(
        &self,
        cursor: &mut W,
        uexp_cursor: Option<&mut W>,
    ) -> Result<(), Error> {
        if self.use_separate_bulk_data_files != uexp_cursor.is_some() {
            return Err(Error::no_data(format!(
                "use_separate_bulk_data_files is {} but uexp_cursor is {}",
                self.use_separate_bulk_data_files,
                match uexp_cursor.is_some() {
                    true => "Some(...)",
                    false => "None",
                }
            )));
        }

        let header = AssetHeader {
            name_offset: self.name_offset,
            import_offset: self.import_offset,
            export_offset: self.export_offset,
            depends_offset: self.depends_offset,
            soft_package_reference_offset: self.soft_package_reference_offset,
            asset_registry_data_offset: self.asset_registry_data_offset,
            world_tile_info_offset: self.world_tile_info_offset,
            preload_dependency_count: 0,
            preload_dependency_offset: self.preload_dependency_offset,
            header_offset: self.header_offset,
            bulk_data_start_offset: self.bulk_data_start_offset,
        };

        let mut serializer = AssetSerializer::new(self, cursor);

        self.write_header(&mut serializer, &header)?;

        let name_offset = match !self.name_map_index_list.is_empty() {
            true => serializer.position() as i32,
            false => 0,
        };

        for name in &self.name_map_index_list {
            serializer.write_fstring(Some(name))?;

            if self.object_version >= ObjectVersion::VER_UE4_NAME_HASHES_SERIALIZED {
                match self.override_name_map_hashes.get_by_key(name) {
                    Some(e) => serializer.write_u32::<LittleEndian>(*e)?,
                    None => serializer.write_u32::<LittleEndian>(crc::generate_hash(name))?,
                };
            }
        }

        let import_offset = match !self.imports.is_empty() {
            true => serializer.position() as i32,
            false => 0,
        };

        for import in &self.imports {
            serializer.write_fname(&import.class_package)?;
            serializer.write_fname(&import.class_name)?;
            serializer.write_i32::<LittleEndian>(import.outer_index.index)?;
            serializer.write_fname(&import.object_name)?;
        }

        let export_offset = match !self.exports.is_empty() {
            true => serializer.position() as i32,
            false => 0,
        };

        for export in &self.exports {
            let unk: &BaseExport = export.get_base_export();
            self.write_export_header(
                unk,
                &mut serializer,
                unk.serial_size,
                unk.serial_offset,
                unk.first_export_dependency_offset,
            )?;
        }

        let depends_offset = match self.depends_map {
            Some(_) => serializer.position() as i32,
            None => 0,
        };

        if let Some(ref map) = self.depends_map {
            for i in 0..self.exports.len() {
                let dummy = Vec::new();
                let current_data = match map.get(i) {
                    Some(e) => e,
                    None => &dummy,
                };
                serializer.write_i32::<LittleEndian>(current_data.len() as i32)?;
                for i in current_data {
                    serializer.write_i32::<LittleEndian>(*i)?;
                }
            }
        }

        let soft_package_reference_offset = match self.soft_package_reference_list {
            Some(_) => serializer.position() as i32,
            None => 0,
        };

        if let Some(ref package_references) = self.soft_package_reference_list {
            for reference in package_references {
                serializer.write_fstring(Some(reference))?;
            }
        }

        // todo: asset registry data support
        // we can support it now I think?
        let asset_registry_data_offset = match self.asset_registry_data_offset != 0 {
            true => serializer.position() as i32,
            false => 0,
        };
        if self.asset_registry_data_offset != 0 {
            serializer.write_i32::<LittleEndian>(0)?; // asset registry data length
        }

        let world_tile_info_offset = match self.world_tile_info {
            Some(_) => serializer.position() as i32,
            None => 0,
        };

        if let Some(ref world_tile_info) = self.world_tile_info {
            world_tile_info.write(&mut serializer)?;
        }

        let mut preload_dependency_count = 0;
        let preload_dependency_offset = serializer.position() as i32;

        if self.use_separate_bulk_data_files {
            for export in &self.exports {
                let unk_export = export.get_base_export();

                for element in &unk_export.serialization_before_serialization_dependencies {
                    serializer.write_i32::<LittleEndian>(element.index)?;
                }

                for element in &unk_export.create_before_serialization_dependencies {
                    serializer.write_i32::<LittleEndian>(element.index)?;
                }

                for element in &unk_export.serialization_before_create_dependencies {
                    serializer.write_i32::<LittleEndian>(element.index)?;
                }

                for element in &unk_export.create_before_create_dependencies {
                    serializer.write_i32::<LittleEndian>(element.index)?;
                }

                preload_dependency_count += unk_export
                    .serialization_before_serialization_dependencies
                    .len() as i32
                    + unk_export.create_before_serialization_dependencies.len() as i32
                    + unk_export.serialization_before_create_dependencies.len() as i32
                    + unk_export.create_before_create_dependencies.len() as i32;
            }
        } else {
            preload_dependency_count = -1;
        }

        let header_offset = match !self.exports.is_empty() {
            true => serializer.position() as i32,
            false => 0,
        };

        let mut category_starts = Vec::with_capacity(self.exports.len());

        let final_cursor_pos = serializer.position();

        let mut bulk_serializer = match self.use_separate_bulk_data_files {
            true => Some(AssetSerializer::new(self, uexp_cursor.unwrap())),
            false => None,
        };

        let bulk_serializer = match self.use_separate_bulk_data_files {
            true => bulk_serializer.as_mut().unwrap(),
            false => &mut serializer,
        };

        for export in &self.exports {
            category_starts.push(match self.use_separate_bulk_data_files {
                true => bulk_serializer.position() + final_cursor_pos,
                false => bulk_serializer.position(),
            });
            export.write(bulk_serializer)?;
            if let Some(normal_export) = export.get_normal_export() {
                bulk_serializer.write_all(&normal_export.extras)?;
            }
        }
        bulk_serializer.write_all(&[0xc1, 0x83, 0x2a, 0x9e])?;

        let bulk_data_start_offset = match self.use_separate_bulk_data_files {
            true => final_cursor_pos as i64 + bulk_serializer.position() as i64,
            false => serializer.position() as i64,
        } - 4;

        if !self.exports.is_empty() {
            serializer.seek(SeekFrom::Start(export_offset as u64))?;
            let mut first_export_dependency_offset = 0;
            for i in 0..self.exports.len() {
                let unk = &self.exports[i].get_base_export();
                let next_loc = match self.exports.len() - 1 > i {
                    true => category_starts[i + 1] as i64,
                    false => bulk_data_start_offset,
                };
                self.write_export_header(
                    unk,
                    &mut serializer,
                    next_loc - category_starts[i] as i64,
                    category_starts[i] as i64,
                    match self.use_separate_bulk_data_files {
                        true => first_export_dependency_offset,
                        false => -1,
                    },
                )?;
                first_export_dependency_offset +=
                    (unk.serialization_before_serialization_dependencies.len()
                        + unk.create_before_serialization_dependencies.len()
                        + unk.serialization_before_create_dependencies.len()
                        + unk.create_before_create_dependencies.len()) as i32;
            }
        }

        serializer.seek(SeekFrom::Start(0))?;

        let header = AssetHeader {
            name_offset,
            import_offset,
            export_offset,
            depends_offset,
            soft_package_reference_offset,
            asset_registry_data_offset,
            world_tile_info_offset,
            preload_dependency_count,
            preload_dependency_offset,
            header_offset,
            bulk_data_start_offset,
        };
        self.write_header(&mut serializer, &header)?;

        serializer.seek(SeekFrom::Start(0))?;
        Ok(())
    }

    /// Generate `UnversionedHeader` for properties and sort the properties into a new array
    fn generate_unversioned_header(
        &self,
        properties: &[Property],
        parent_name: &FName,
    ) -> Result<Option<(UnversionedHeader, Vec<Property>)>, Error> {
        if !self.has_unversioned_properties() {
            return Ok(None);
        }

        let Some(mappings) = self.get_mappings() else {
            return Ok(None);
        };

        let mut first_global_index = u32::MAX;
        let mut last_global_index = u32::MIN;

        let mut properties_to_process = HashSet::new();
        let mut zero_properties: HashSet<u32> = HashSet::new();

        for property in properties {
            let Some((_, global_index)) = mappings.get_property_with_duplication_index(
                &property.get_name(),
                property.get_ancestry(),
                property.get_duplication_index() as u32,
            ) else {
                return Err(PropertyError::no_mapping(&property.get_name().content, property.get_ancestry()).into());
            };

            if matches!(property, Property::EmptyProperty(_)) {
                zero_properties.insert(global_index);
            }

            first_global_index = first_global_index.min(global_index);
            last_global_index = last_global_index.max(global_index);
            properties_to_process.insert(global_index);
        }

        // Sort properties and generate header fragments
        let mut sorted_properties = Vec::new();

        let mut fragments: Vec<UnversionedHeaderFragment> = Vec::new();
        let mut last_num_before_fragment = 0;

        if !properties_to_process.is_empty() {
            loop {
                let mut has_zeros = false;

                // Find next contiguous properties chunk
                let mut start_index = last_num_before_fragment;
                while !properties_to_process.contains(&start_index)
                    && start_index <= last_global_index
                {
                    start_index += 1;
                }

                if start_index > last_global_index {
                    break;
                }

                // Process contiguous properties chunk
                let mut end_index = start_index;
                while properties_to_process.contains(&end_index) {
                    if zero_properties.contains(&end_index) {
                        has_zeros = true;
                    }

                    // todo: clone might not be needed
                    sorted_properties.push(properties[end_index as usize].clone());
                    end_index += 1;
                }

                // Create extra fragments for this chunk
                let mut skip_num = start_index - last_num_before_fragment - 1;
                let mut value_num = (end_index - 1) - start_index + 1;

                while skip_num > i8::MAX as u32 {
                    fragments.push(UnversionedHeaderFragment {
                        skip_num: i8::MAX as u8,
                        value_num: 0,
                        first_num: 0,
                        is_last: false,
                        has_zeros: false,
                    });
                    skip_num -= i8::MAX as u32;
                }
                while value_num > i8::MAX as u32 {
                    fragments.push(UnversionedHeaderFragment {
                        skip_num: 0,
                        value_num: i8::MAX as u8,
                        first_num: 0,
                        is_last: false,
                        has_zeros: false,
                    });
                    value_num -= i8::MAX as u32;
                }

                // Create the main fragment for this chunk
                let fragment = UnversionedHeaderFragment {
                    skip_num: skip_num as u8,
                    value_num: value_num as u8,
                    first_num: start_index as u8,
                    is_last: false,
                    has_zeros,
                };

                fragments.push(fragment);
                last_num_before_fragment = end_index - 1;
            }
        } else {
            fragments.push(UnversionedHeaderFragment {
                skip_num: usize::min(
                    mappings.get_all_properties(&parent_name.content).len(),
                    i8::MAX as usize,
                ) as u8,
                value_num: 0,
                first_num: 0,
                is_last: true,
                has_zeros: false,
            });
        }

        if let Some(fragment) = fragments.last_mut() {
            fragment.is_last = true;
        }

        let mut has_non_zero_values = false;
        let mut zero_mask = BitVec::<u8, Lsb0>::new();

        for fragment in fragments.iter().filter(|e| e.has_zeros) {
            for i in 0..fragment.value_num {
                let is_zero = zero_properties.contains(&((fragment.first_num + i) as u32));
                if !is_zero {
                    has_non_zero_values = true;
                }
                zero_mask.push(is_zero);
            }
        }

        let unversioned_property_index =
            fragments.first().map(|e| e.first_num).unwrap_or_default() as usize;

        let header = UnversionedHeader {
            fragments,
            zero_mask,
            has_non_zero_values,
            unversioned_property_index,
            current_fragment_index: 0,
            zero_mask_index: 0,
        };

        Ok(Some((header, sorted_properties)))
    }
}

// custom debug implementation to not print the whole data buffer
impl<C: Read + Seek> Debug for Asset<C> {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), std::fmt::Error> {
        f.debug_struct("Asset")
            .field("info", &self.info)
            .field(
                "use_separate_bulk_data_files",
                &self.use_separate_bulk_data_files,
            )
            .field("object_version", &self.object_version)
            .field("object_version_ue5", &self.object_version_ue5)
            .field("legacy_file_version", &self.legacy_file_version)
            .field("unversioned", &self.unversioned)
            .field("file_license_version", &self.file_license_version)
            .field("custom_version", &self.custom_versions)
            // imports
            // exports
            // depends map
            // soft package reference list
            // asset registry data
            // world tile info
            // preload dependencies
            .field("generations", &self.generations)
            .field("package_guid", &self.package_guid)
            .field("engine_version", &self.get_engine_version())
            .field("engine_version_recorded", &self.engine_version_recorded)
            .field("engine_version_compatible", &self.engine_version_compatible)
            .field("chunk_ids", &self.chunk_ids)
            .field("package_flags", &self.package_flags)
            .field("package_source", &self.package_source)
            .field("folder_name", &self.folder_name)
            // map struct type override
            // override name map hashes
            .field("header_offset", &self.header_offset)
            .field("name_count", &self.name_count)
            .field("name_offset", &self.name_offset)
            .field(
                "gatherable_text_data_count",
                &self.gatherable_text_data_count,
            )
            .field(
                "gatherable_text_data_offset",
                &self.gatherable_text_data_offset,
            )
            .field("export_count", &self.export_count)
            .field("export_offset", &self.export_offset)
            .field("import_count", &self.import_count)
            .field("import_offset", &self.import_offset)
            .field("depends_offset", &self.depends_offset)
            .field(
                "soft_package_reference_count",
                &self.soft_package_reference_count,
            )
            .field(
                "soft_package_reference_offset",
                &self.soft_package_reference_offset,
            )
            .field("searchable_names_offset", &self.searchable_names_offset)
            .field("thumbnail_table_offset", &self.thumbnail_table_offset)
            .field("compression_flags", &self.compression_flags)
            .field(
                "asset_registry_data_offset",
                &self.asset_registry_data_offset,
            )
            .field("bulk_data_start_offset", &self.bulk_data_start_offset)
            .field("world_tile_info_data_offset", &self.world_tile_info_offset)
            .field("preload_dependency_count", &self.preload_dependency_count)
            .field("preload_dependency_offset", &self.preload_dependency_offset)
            .field("exports", &self.exports)
            .finish()
    }
}

/// EngineVersion for an Asset
#[derive(Debug, Clone)]
pub struct FEngineVersion {
    major: u16,
    minor: u16,
    patch: u16,
    build: u32,
    branch: Option<String>,
}
impl FEngineVersion {
    fn new(major: u16, minor: u16, patch: u16, build: u32, branch: Option<String>) -> Self {
        Self {
            major,
            minor,
            patch,
            build,
            branch,
        }
    }

    fn read<C: Read + Seek>(cursor: &mut C) -> Result<Self, Error> {
        let major = cursor.read_u16::<LittleEndian>()?;
        let minor = cursor.read_u16::<LittleEndian>()?;
        let patch = cursor.read_u16::<LittleEndian>()?;
        let build = cursor.read_u32::<LittleEndian>()?;
        let branch = cursor.read_fstring()?;

        Ok(Self::new(major, minor, patch, build, branch))
    }

    fn write<Writer: AssetWriter>(&self, cursor: &mut Writer) -> Result<(), Error> {
        cursor.write_u16::<LittleEndian>(self.major)?;
        cursor.write_u16::<LittleEndian>(self.minor)?;
        cursor.write_u16::<LittleEndian>(self.patch)?;
        cursor.write_u32::<LittleEndian>(self.build)?;
        cursor.write_fstring(self.branch.as_deref())?;
        Ok(())
    }

    fn unknown() -> Self {
        Self::new(0, 0, 0, 0, None)
    }
}
