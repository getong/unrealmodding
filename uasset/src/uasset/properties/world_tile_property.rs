use std::{io::{Error, Cursor, ErrorKind}, str::FromStr};

use byteorder::{LittleEndian, ReadBytesExt};

use crate::uasset::{types::Vector, cursor_ext::CursorExt, ue4version::{VER_UE4_WORLD_LEVEL_INFO_UPDATED, VER_UE4_WORLD_LAYER_ENABLE_DISTANCE_STREAMING, VER_UE4_WORLD_LEVEL_INFO_LOD_LIST, VER_UE4_WORLD_LEVEL_INFO_ZORDER}, Asset, custom_version::FFortniteMainBranchObjectVersion, unreal_types::FName};

use super::vector_property::{IntPointProperty, BoxProperty};


pub struct FWorldTileLayer {
    name: String,
    reserved_0: i32,
    reserved_1: IntPointProperty,
    streaming_distance: Option<i32>,
    distance_streaming_enabled: Option<bool>
}

impl FWorldTileLayer {
    pub fn new(cursor: &mut Cursor<Vec<u8>>, engine_version: i32) -> Result<Self, Error> {
        let name = cursor.read_string()?;
        let reserved_0 = cursor.read_i32::<LittleEndian>()?;
        let reserved_1 = IntPointProperty::new(FName::default(), &mut cursor, false)?;
        
        let streaming_distance = match engine_version >= VER_UE4_WORLD_LEVEL_INFO_UPDATED {
            true => Some(cursor.read_i32::<LittleEndian>()?),
            false => None
        };

        let distance_streaming_enabled = match engine_version >= VER_UE4_WORLD_LAYER_ENABLE_DISTANCE_STREAMING {
            true => Some(cursor.read_i32::<LittleEndian>()? == 1),
            false => None
        };

        Ok(FWorldTileLayer {
            name, reserved_0, reserved_1, streaming_distance, distance_streaming_enabled
        })
    }
}

pub struct FWorldTileLODInfo {
    relative_streaming_distance: i32,
    reserved_0: f32,
    reserved_1: f32,
    reserved_2: i32,
    reserved_3: i32
}

impl FWorldTileLODInfo {
    pub fn new(cursor: &mut Cursor<Vec<u8>>) -> Result<Self, Error> {
        Ok(FWorldTileLODInfo {
            relative_streaming_distance: cursor.read_i32::<LittleEndian>()?,
            reserved_0: cursor.read_f32::<LittleEndian>()?,
            reserved_1: cursor.read_f32::<LittleEndian>()?,
            reserved_2: cursor.read_i32::<LittleEndian>()?,
            reserved_3: cursor.read_i32::<LittleEndian>()?
        })
    }
}

pub struct FWorldTileInfo {
    position: Vector<i32>,
    bounds: BoxProperty,
    //absolute_position: Vector<i32>, // not set in most recent version of uassetapi?
    layer: FWorldTileLayer,
    hide_in_tile_view: Option<bool>,
    parent_tile_package_name: Option<String>,
    lod_list: Option<Vec<FWorldTileLODInfo>>,
    z_order: Option<i32>
}

impl FWorldTileInfo {
    pub fn new(cursor: &mut Cursor<Vec<u8>>, engine_version: i32, asset: &mut Asset) -> Result<Self, Error> {
        let version = asset.get_custom_version("FFortniteMainBranchObjectVersion").ok_or(Error::new(ErrorKind::Other, "Unknown custom version"))?;

        let position = match version.version < FFortniteMainBranchObjectVersion::WorldCompositionTile3DOffset as i32 {
            true => Vector::new(cursor.read_i32::<LittleEndian>()?, cursor.read_i32::<LittleEndian>()?, 0),
            false => cursor.read_int_vector()?
        };

        let bounds = BoxProperty::new(FName::default(), cursor, false)?;
        let layer = FWorldTileLayer::new(cursor, engine_version)?;

        let mut hide_in_tile_view = None;
        let mut parent_tile_package_name = None;
        if engine_version >= VER_UE4_WORLD_LEVEL_INFO_UPDATED {
            hide_in_tile_view = Some(cursor.read_i32::<LittleEndian>()? == 1);
            parent_tile_package_name = Some(cursor.read_string()?);
        }

        let mut lod_list = None;
        if engine_version >= VER_UE4_WORLD_LEVEL_INFO_LOD_LIST {
            let num_entries = cursor.read_i32::<LittleEndian>()?;
            lod_list = Some(Vec::new());
            for i in 0..num_entries {
                lod_list.unwrap().push(FWorldTileLODInfo::new(cursor)?);
            }
        }

        let z_order = match engine_version >= VER_UE4_WORLD_LEVEL_INFO_ZORDER {
            true => Some(cursor.read_i32::<LittleEndian>()?),
            false => None
        };

        Ok(FWorldTileInfo {
            position, bounds, layer, hide_in_tile_view, parent_tile_package_name, lod_list, z_order
        })
    }
}
