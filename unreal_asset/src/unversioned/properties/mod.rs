//! Usmap properties

use byteorder::LE;
use enum_dispatch::enum_dispatch;
use num_enum::{IntoPrimitive, TryFromPrimitive};
use std::{fmt::Debug, hash::Hash};

use crate::{
    error::Error,
    reader::{archive_reader::ArchiveReader, archive_writer::ArchiveWriter},
};

use self::{
    array_property::UsmapArrayPropertyData, enum_property::UsmapEnumPropertyData,
    map_property::UsmapMapPropertyData, set_property::UsmapSetPropertyData,
    shallow_property::UsmapShallowPropertyData, struct_property::UsmapStructPropertyData,
};

use super::{usmap_reader::UsmapReader, usmap_writer::UsmapWriter};

pub mod array_property;
pub mod enum_property;
pub mod map_property;
pub mod set_property;
pub mod shallow_property;
pub mod struct_property;

/// Usmap property type
#[derive(Debug, Copy, Clone, Hash, PartialEq, Eq, IntoPrimitive, TryFromPrimitive)]
#[repr(u8)]
pub enum EPropertyType {
    /// Byte
    ByteProperty,
    /// Boolean
    BoolProperty,
    /// Int
    IntProperty,
    /// Float
    FloatProperty,
    /// Object
    ObjectProperty,
    /// Name
    NameProperty,
    /// Delegate
    DelegateProperty,
    /// Double
    DoubleProperty,
    /// Array
    ArrayProperty,
    /// Struct
    StructProperty,
    /// String
    StrProperty,
    /// Text
    TextProperty,
    /// Interface
    InterfaceProperty,
    /// MulticastDelegate
    MulticastDelegateProperty,
    /// WeakObject
    WeakObjectProperty, //
    /// LazyObject
    LazyObjectProperty, // When deserialized, these 3 properties will be SoftObjects
    /// AssetObject
    AssetObjectProperty, //
    /// SoftObject
    SoftObjectProperty,
    /// UInt64
    UInt64Property,
    /// UInt32
    UInt32Property,
    /// UInt16
    UInt16Property,
    /// Int64
    Int64Property,
    /// Int16
    Int16Property,
    /// Int8
    Int8Property,
    /// Map
    MapProperty,
    /// Set
    SetProperty,
    /// Enum
    EnumProperty,
    /// FieldPath
    FieldPathProperty,

    /// Unknown
    Unknown = 0xFF,
}

impl ToString for EPropertyType {
    fn to_string(&self) -> String {
        match *self {
            EPropertyType::ByteProperty => "ByteProperty".to_string(),
            EPropertyType::BoolProperty => "BoolProperty".to_string(),
            EPropertyType::IntProperty => "IntProperty".to_string(),
            EPropertyType::FloatProperty => "FloatProperty".to_string(),
            EPropertyType::ObjectProperty => "ObjectProperty".to_string(),
            EPropertyType::NameProperty => "NameProperty".to_string(),
            EPropertyType::DelegateProperty => "DelegateProperty".to_string(),
            EPropertyType::DoubleProperty => "DoubleProperty".to_string(),
            EPropertyType::ArrayProperty => "ArrayProperty".to_string(),
            EPropertyType::StructProperty => "StructProperty".to_string(),
            EPropertyType::StrProperty => "StrProperty".to_string(),
            EPropertyType::TextProperty => "TextProperty".to_string(),
            EPropertyType::InterfaceProperty => "InterfaceProperty".to_string(),
            EPropertyType::MulticastDelegateProperty => "MulticastDelegateProperty".to_string(),
            EPropertyType::WeakObjectProperty => "WeakObjectProperty".to_string(),
            EPropertyType::LazyObjectProperty => "LazyObjectProperty".to_string(),
            EPropertyType::AssetObjectProperty => "AssetObjectProperty".to_string(),
            EPropertyType::SoftObjectProperty => "SoftObjectProperty".to_string(),
            EPropertyType::UInt64Property => "UInt64Property".to_string(),
            EPropertyType::UInt32Property => "UInt32Property".to_string(),
            EPropertyType::UInt16Property => "UInt16Property".to_string(),
            EPropertyType::Int64Property => "Int64Property".to_string(),
            EPropertyType::Int16Property => "Int16Property".to_string(),
            EPropertyType::Int8Property => "Int8Property".to_string(),
            EPropertyType::MapProperty => "MapProperty".to_string(),
            EPropertyType::SetProperty => "SetProperty".to_string(),
            EPropertyType::EnumProperty => "EnumProperty".to_string(),
            EPropertyType::FieldPathProperty => "FieldPathProperty".to_string(),
            EPropertyType::Unknown => "Unknown".to_string(),
        }
    }
}

/// This must be implemented for all UsmapPropertyDatas
#[enum_dispatch]
pub trait UsmapPropertyDataTrait: Debug + Hash + Clone + PartialEq + Eq {
    /// Get `UsmapPropertyData` property type
    fn get_property_type(&self) -> EPropertyType;
    /// Write `UsmapPropertyData` to an asset
    fn write<W: ArchiveWriter>(&self, writer: &mut UsmapWriter<'_, '_, W>) -> Result<usize, Error>;
}

/// UsmapPropertyData
#[enum_dispatch(UsmapPropertyDataTrait)]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum UsmapPropertyData {
    /// Enum
    UsmapEnumPropertyData,
    /// Struct
    UsmapStructPropertyData,
    /// Set
    UsmapSetPropertyData,
    /// Array
    UsmapArrayPropertyData,
    /// Map
    UsmapMapPropertyData,

    /// Shallow
    UsmapShallowPropertyData,
}

impl UsmapPropertyData {
    /// Read an `UsmapPropertyData` from an asset
    pub fn new<R: ArchiveReader>(asset: &mut UsmapReader<'_, '_, R>) -> Result<Self, Error> {
        let prop_type: EPropertyType = EPropertyType::try_from(asset.read_u8()?)?;

        let res: UsmapPropertyData = match prop_type {
            EPropertyType::ArrayProperty => UsmapArrayPropertyData::new(asset)?.into(),
            EPropertyType::StructProperty => UsmapStructPropertyData::new(asset)?.into(),
            EPropertyType::MapProperty => UsmapMapPropertyData::new(asset)?.into(),
            EPropertyType::SetProperty => UsmapSetPropertyData::new(asset)?.into(),
            EPropertyType::EnumProperty => UsmapEnumPropertyData::new(asset)?.into(),
            _ => UsmapShallowPropertyData {
                property_type: prop_type,
            }
            .into(),
        };

        Ok(res)
    }
}

/// UsmapProperty
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct UsmapProperty {
    /// Name
    pub name: String,
    /// Schema index
    pub schema_index: u16,
    /// Array size
    pub array_size: u8,
    /// Array index (not serialized)
    pub array_index: u16,
    /// Property data
    pub property_data: UsmapPropertyData,
}

impl UsmapProperty {
    /// Read an `UsmapProperty` from an asset
    pub fn new<R: ArchiveReader>(asset: &mut UsmapReader<'_, '_, R>) -> Result<Self, Error> {
        let schema_index = asset.read_u16::<LE>()?;
        let array_size = asset.read_u8()?;
        let name = asset.read_name()?.unwrap_or_default();

        let property_data = UsmapPropertyData::new(asset)?;
        Ok(UsmapProperty {
            name,
            schema_index,
            array_size,
            array_index: 0,
            property_data,
        })
    }
}
