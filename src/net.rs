use std::borrow::Cow;

use raylib::ffi::Vector3;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::common::{
    AssetId, BlockInfo, EntityInfo, Face, Heading, ItemInfo, PlayerMovement, Registries,
};

#[derive(Serialize, Deserialize)]
pub enum MessageS2C<'a> {
    GameData {
        world_x: usize,
        world_y: usize,
        world_z: usize,
        registries: Cow<'a, Registries>,
    },
    SetPlayerEntity {
        kind: AssetId,
        id: Uuid,
    },
    SetBlock {
        x: usize,
        y: usize,
        z: usize,
        block: AssetId,
    },
    SpawnEntity {
        id: Uuid,
        position: [f32; 3],
        kind: AssetId,
        name: String,
        heading: Heading,
    },
    MoveEntity {
        id: Uuid,
        position: [f32; 3],
        heading: Heading,
    },
    RemoveEntity(Uuid),
    Teleport {
        position: [f32; 3],
        heading: Option<Heading>,
        nonce: usize,
    },
    LoadMap {
        offset: usize,
        data: Vec<AssetId>,
        finished: bool,
    },
    SetMovement(PlayerMovement),
    SetSlot {
        slot: usize,
        item: Option<AssetId>,
        lock: bool,
    },
    SetItemCount {
        item: AssetId,
        count: u32,
    },
}
#[derive(Serialize, Deserialize)]
pub enum MessageC2S {
    Login {
        name: String,
        cookie: String,
    },
    UpdatePosition {
        position: [f32; 3],
        heading: Heading,
        nonce: usize,
    },
    Click {
        is_right: bool,
        heading: Heading,
        item: Option<AssetId>,
        block: Option<([usize; 3], Face)>,
        entity: Option<Uuid>,
    },
    MapChunkLoaded,
}
