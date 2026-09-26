use std::{cell::Cell, marker::PhantomData};

use image::DynamicImage;
use raylib::ffi::Vector3;
use serde::{Deserialize, Serialize};
use serialimage::DynamicSerialImage;

use crate::server::DT;

pub const BLOCK_TEXTURE_SIZE: u32 = 16;

#[derive(Serialize, Deserialize, Clone)]
pub struct Registries {
    pub blocks: Vec<BlockInfo>,
    pub items: Vec<ItemInfo>,
    pub entities: Vec<EntityInfo>,
    pub textures: Vec<TextureInfo>,
}
#[derive(Serialize, Deserialize, Clone)]
pub struct BlockInfo {
    pub collision: bool,
    pub selectable: bool,
    pub render_data: Option<[AssetId; 6]>,
}
#[derive(Serialize, Deserialize, Clone)]
pub struct ItemInfo(pub AssetId);
#[derive(Serialize, Deserialize, Clone)]
pub struct EntityInfo {
    pub texture: AssetId,
    pub size: f32,
    pub height: f32,
    pub eye_height: f32,
}
#[derive(Serialize, Deserialize, Clone)]
pub struct TextureInfo {
    pub image: DynamicSerialImage,
    pub is_block: bool,
}

pub type AssetId = u16;

pub struct BlockBuffer {
    pub dim_x: usize,
    pub dim_y: usize,
    pub dim_z: usize,
    pub blocks: Box<[Cell<AssetId>]>,
}
impl BlockBuffer {
    pub fn filled(block: AssetId, dim_x: usize, dim_y: usize, dim_z: usize) -> BlockBuffer {
        let mut blocks = Vec::new();
        blocks.resize(dim_x * dim_y * dim_z, Cell::new(block));
        BlockBuffer {
            dim_x,
            dim_y,
            dim_z,
            blocks: blocks.into_boxed_slice(),
        }
    }
    fn block_index(&self, x: usize, y: usize, z: usize) -> Result<usize, ()> {
        if x >= self.dim_x || y >= self.dim_y || z >= self.dim_z {
            return Err(());
        }
        Ok(x + y * self.dim_x + z * self.dim_x * self.dim_y)
    }
    pub fn set(&self, block: AssetId, x: usize, y: usize, z: usize) -> Result<(), ()> {
        self.blocks[self.block_index(x, y, z)?].set(block);
        Ok(())
    }
    pub fn get(&self, x: usize, y: usize, z: usize) -> Result<AssetId, ()> {
        Ok(self.blocks[self.block_index(x, y, z)?].get())
    }
    pub fn copy(&self, target: &BlockBuffer, x: usize, y: usize, z: usize) {
        for iz in 0..self.dim_z {
            for iy in 0..self.dim_y {
                for ix in 0..self.dim_x {
                    let block = self.get(x, y, z).unwrap();
                    let _ = target.set(block, x + ix, y + iy, z + iz);
                }
            }
        }
    }
    pub fn slice(
        &self,
        offset_x: usize,
        offset_y: usize,
        offset_z: usize,
        dim_x: usize,
        dim_y: usize,
        dim_z: usize,
    ) -> BlockBuffer {
        let dim_x = dim_x.min(self.dim_x.saturating_sub(offset_x));
        let dim_y = dim_y.min(self.dim_z.saturating_sub(offset_y));
        let dim_z = dim_z.min(self.dim_z.saturating_sub(offset_z));
        let mut new_buffer = BlockBuffer::filled(0, dim_x, dim_y, dim_z);
        for x in 0..dim_x {
            for y in 0..dim_y {
                for z in 0..dim_z {
                    new_buffer
                        .set(
                            self.get(x + offset_x, y + offset_y, z + offset_z).unwrap(),
                            x,
                            y,
                            z,
                        )
                        .unwrap();
                }
            }
        }
        new_buffer
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Face {
    X,
    NX,
    Z,
    NZ,
    Y,
    NY,
}
impl Face {
    pub fn all() -> [Face; 6] {
        [Face::X, Face::NX, Face::Z, Face::NZ, Face::Y, Face::NY]
    }
    pub fn offset_x(self) -> isize {
        match self {
            Face::X => 1,
            Face::NX => -1,
            _ => 0,
        }
    }
    pub fn offset_y(self) -> isize {
        match self {
            Face::Y => 1,
            Face::NY => -1,
            _ => 0,
        }
    }
    pub fn offset_z(self) -> isize {
        match self {
            Face::Z => 1,
            Face::NZ => -1,
            _ => 0,
        }
    }
}
#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct Heading {
    pub yaw: f32,
    pub pitch: f32,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct PlayerMovement {
    pub speed: f32,
    pub jump_height: f32,
    pub fly: bool,
    pub gravity: f32,
}
impl Default for PlayerMovement {
    fn default() -> Self {
        PlayerMovement {
            speed: 6.,
            jump_height: 1.1,
            fly: false,
            gravity: 25.,
        }
    }
}
pub struct CharacterController {
    pub velocity: Vector3,
    pub on_ground: bool,
    pub gravity: f32,
    pub drag: f32,
}
impl Default for CharacterController {
    fn default() -> Self {
        Self {
            velocity: Vector3::ZERO,
            on_ground: false,
            gravity: 25.,
            drag: 0.1,
        }
    }
}
impl CharacterController {
    pub fn tick(
        &mut self,
        position: &mut Vector3,
        world: &BlockBuffer,
        entity: AssetId,
        registries: &Registries,
        dt: f32,
    ) {
        let entity = &registries.entities[entity as usize];
        self.velocity *= (1_f32 - self.drag).powf(dt);
        self.velocity.y -= self.gravity * dt;
        self.on_ground = false;
        for (movement, vc) in [
            (Vector3::X * self.velocity.x, &mut self.velocity.x),
            (Vector3::Y * self.velocity.y, &mut self.velocity.y),
            (Vector3::Z * self.velocity.z, &mut self.velocity.z),
        ] {
            let movement = movement * dt;
            let bb = HitBox::from_entity(*position, entity);
            if bb.inflate(movement).block_iter().any(|block| {
                if bb.contains_block(block[0], block[1], block[2]) {
                    return false;
                }
                registries.blocks[world.get(block[0], block[1], block[2]).unwrap_or(0) as usize]
                    .collision
            }) {
                if movement.y < 0. {
                    self.on_ground = true;
                }
                *vc = 0.;
            } else {
                *position += movement;
            }
        }
    }
    pub fn input(&mut self, input: Vector3, accelerate_y: bool, acceleration: f32, dt: f32) {
        let mut error = input - self.velocity;
        if !accelerate_y {
            error.y = 0.;
        }
        if error.length() > 0. {
            self.velocity += error.normalize() * (error.length().min(acceleration * dt));
        }
    }
}
#[derive(Copy, Clone, Debug)]
pub struct HitBox {
    pub min: Vector3,
    pub max: Vector3,
}
impl HitBox {
    pub fn from_entity(position: Vector3, entity: &EntityInfo) -> Self {
        let xz = Vector3 {
            x: entity.size,
            z: entity.size,
            y: 0.,
        };
        HitBox {
            min: position - xz,
            max: position + xz + Vector3::Y * entity.height,
        }
    }
    pub fn inflate(mut self, by: Vector3) -> Self {
        if by.x < 0. {
            self.min.x += by.x;
        } else {
            self.max.x += by.x;
        }
        if by.y < 0. {
            self.min.y += by.y;
        } else {
            self.max.y += by.y;
        }
        if by.z < 0. {
            self.min.z += by.z;
        } else {
            self.max.z += by.z;
        }
        self
    }
    pub fn contains_block(self, x: usize, y: usize, z: usize) -> bool {
        let min_x = self.min.x.floor() as isize;
        let min_y = self.min.y.floor() as isize;
        let min_z = self.min.z.floor() as isize;
        let max_x = self.max.x.floor() as isize;
        let max_y = self.max.y.floor() as isize;
        let max_z = self.max.z.floor() as isize;
        let x = x as isize;
        let y = y as isize;
        let z = z as isize;
        return min_x <= x && max_x >= x && min_y <= y && max_y >= y && min_z <= z && max_z >= z;
    }
    pub fn block_iter(self) -> impl Iterator<Item = [usize; 3]> {
        let min_x = self.min.x.floor().max(0.) as usize;
        let min_y = self.min.y.floor().max(0.) as usize;
        let min_z = self.min.z.floor().max(0.) as usize;
        let (len_x, len_y, len_z) = if self.max.x < 0. || self.max.y < 0. || self.max.z < 0. {
            (0, 0, 0)
        } else {
            let max_x = self.max.x.floor() as usize;
            let max_y = self.max.y.floor() as usize;
            let max_z = self.max.z.floor() as usize;
            (max_x - min_x + 1, max_y - min_y + 1, max_z - min_z + 1)
        };
        (0..(len_x * len_y * len_z)).into_iter().map(move |i| {
            [
                min_x + i % len_x,
                min_y + (i / len_x) % len_y,
                min_z + i / len_x / len_y,
            ]
        })
    }
}
