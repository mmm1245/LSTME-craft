use std::{
    cell::Cell,
    collections::{HashMap, HashSet},
    f32::consts::PI,
    io::Cursor,
    sync::mpsc::{Receiver, TryRecvError, channel},
    thread,
};

use image::{DynamicImage, ImageFormat, imageops};
use message_io::{
    network::{Endpoint, NetEvent, Transport},
    node::{self, NodeHandler},
};
use raylib::{
    ffi::{CSSPalette, RaylibPalette, Rectangle},
    prelude::*,
};
use uuid::Uuid;

use crate::{
    common::{
        AssetId, BLOCK_TEXTURE_SIZE, BlockBuffer, BlockInfo, CharacterController, Face, Heading,
        HitBox, PlayerMovement, Registries, TextureInfo,
    },
    net::{MessageC2S, MessageS2C},
};

enum TextureEntry {
    Block([[f32; 2]; 2]),
    Texture(Texture2D),
}
struct TextureManager {
    entries: Vec<TextureEntry>,
    atlas: Texture2D,
    size: f32,
}
impl TextureManager {
    fn new(registry: &Vec<TextureInfo>, rl: &mut RaylibHandle, thread: &RaylibThread) -> Self {
        let cells = registry.iter().filter(|t| t.is_block).count();
        let atlas_side_length = (cells as f32).sqrt().ceil() as u32;
        let atlas_pixel_length = atlas_side_length * BLOCK_TEXTURE_SIZE;
        let mut atlas = DynamicImage::new(
            atlas_pixel_length,
            atlas_pixel_length,
            image::ColorType::Rgba8,
        );
        let mut cell_alloc_index = 0;
        Self {
            size: atlas_pixel_length as f32,
            entries: registry
                .iter()
                .map(|texture| {
                    let img: DynamicImage = texture.image.clone().into();
                    match texture.is_block {
                        true => TextureEntry::Block({
                            let x = cell_alloc_index % atlas_side_length;
                            let y = cell_alloc_index / atlas_side_length;
                            cell_alloc_index += 1;
                            imageops::overlay(
                                &mut atlas,
                                &img,
                                (x * BLOCK_TEXTURE_SIZE) as i64,
                                (y * BLOCK_TEXTURE_SIZE) as i64,
                            );
                            let l = atlas_side_length as f32;
                            println!("l: {}", l);
                            [
                                [x as f32 / l, y as f32 / l],
                                [(x + 1) as f32 / l, (y + 1) as f32 / l],
                            ]
                        }),
                        false => TextureEntry::Texture(Self::load_dynamic_image(&img, rl, thread)),
                    }
                })
                .collect(),
            atlas: Self::load_dynamic_image(&atlas, rl, thread),
        }
    }
    fn get_with_rect(&self, texture: AssetId) -> (&Texture2D, Rectangle) {
        match &self.entries[texture as usize] {
            TextureEntry::Block(block) => (
                &self.atlas,
                Rectangle {
                    x: block[0][0] * self.size,
                    y: block[0][1] * self.size,
                    width: (block[1][0] - block[0][0]) * self.size,
                    height: (block[1][1] - block[0][1]) * self.size,
                },
            ),
            TextureEntry::Texture(texture) => (
                texture,
                Rectangle {
                    x: 0.,
                    y: 0.,
                    width: texture.width as f32,
                    height: texture.height as f32,
                },
            ),
        }
    }
    fn load_dynamic_image(
        image: &DynamicImage,
        rl: &mut RaylibHandle,
        thread: &RaylibThread,
    ) -> Texture2D {
        let mut buf = Vec::new();
        image.write_to(&mut Cursor::new(&mut buf), ImageFormat::Png);
        let image = Image::load_image_from_mem(".png", &buf).unwrap();
        rl.load_texture_from_image(thread, &image).unwrap()
    }
}

const RENDER_CHUNK_SIZE: usize = 16;

pub fn client(addr: &str, name: String) {
    let (mut rl, thread) = raylib::init().size(640, 480).title("Hello, World").build();

    rl.disable_cursor();
    set_trace_log(TraceLogLevel::LOG_NONE);

    let (network, messages) = network_thread(addr);

    let mut world = BlockBuffer::filled(0, 0, 0, 0);
    let mut registries = None;
    let mut entities = HashMap::new();
    let mut player_position = Vector3::ZERO;
    let mut player_heading = Heading { pitch: 0., yaw: 0. };
    let mut teleport_nonce = 0;
    let mut player_kind = 0;
    let mut player_id = Uuid::nil();
    let mut textures = None;
    let mut render_chunks = HashMap::new();
    let mut chunk_build_list = HashSet::new();
    let mut player_movement = PlayerMovement::default();
    let mut character_controller = CharacterController::default();
    let mut player_movement = PlayerMovement::default();
    let mut hotbar = [(None, false); 10];
    let mut hotbar_slot = 0;
    let mut show_inventory = false;
    let mut item_counts = Vec::new();

    let slot_renderer = SlotRenderer::new(&mut rl, &thread);

    while !rl.window_should_close() {
        loop {
            match messages.try_recv() {
                Ok(message) => match message {
                    MessageS2C::GameData {
                        world_x,
                        world_y,
                        world_z,
                        registries: r,
                    } => {
                        world = BlockBuffer::filled(0, world_x, world_y, world_z);
                        item_counts.resize(r.items.len(), 0);
                        registries = Some(r.into_owned());
                        textures = Some(TextureManager::new(
                            &registries.as_ref().unwrap().textures,
                            &mut rl,
                            &thread,
                        ));
                        network.send(MessageC2S::Login {
                            name: name.clone(),
                            cookie: "".to_string(),
                        });
                    }
                    MessageS2C::SetPlayerEntity { kind, id } => {
                        player_kind = kind;
                        player_id = id;
                    }
                    MessageS2C::SetBlock { x, y, z, block } => {
                        let _ = world.set(block, x, y, z);
                        let (xc, xo) = (x / RENDER_CHUNK_SIZE, x % RENDER_CHUNK_SIZE);
                        let (yc, yo) = (y / RENDER_CHUNK_SIZE, y % RENDER_CHUNK_SIZE);
                        let (zc, zo) = (z / RENDER_CHUNK_SIZE, z % RENDER_CHUNK_SIZE);
                        chunk_build_list.insert((xc, yc, zc));
                        if xo == 0 && xc != 0 {
                            chunk_build_list.insert((xc - 1, yc, zc));
                        }
                        if xo == RENDER_CHUNK_SIZE - 1 {
                            chunk_build_list.insert((xc + 1, yc, zc));
                        }
                        if yo == 0 && yc != 0 {
                            chunk_build_list.insert((xc, yc - 1, zc));
                        }
                        if yo == RENDER_CHUNK_SIZE - 1 {
                            chunk_build_list.insert((xc, yc + 1, zc));
                        }
                        if zo == 0 && zc != 0 {
                            chunk_build_list.insert((xc, yc, zc - 1));
                        }
                        if zo == RENDER_CHUNK_SIZE - 1 {
                            chunk_build_list.insert((xc, yc, zc + 1));
                        }
                    }
                    MessageS2C::SpawnEntity {
                        id,
                        position,
                        kind,
                        name,
                        heading,
                    } => {
                        entities.insert(
                            id,
                            ClientEntity {
                                position,
                                heading,
                                kind,
                                name,
                            },
                        );
                    }
                    MessageS2C::MoveEntity {
                        id,
                        position,
                        heading,
                    } => {
                        let Some(entity) = entities.get_mut(&id) else {
                            continue;
                        };
                        entity.position = position;
                        entity.heading = heading;
                    }
                    MessageS2C::RemoveEntity(id) => {
                        entities.remove(&id);
                    }
                    MessageS2C::Teleport {
                        position: pos,
                        heading,
                        nonce,
                    } => {
                        player_position = Vector3 {
                            x: pos[0],
                            y: pos[1],
                            z: pos[2],
                        };

                        if let Some(heading) = heading {
                            player_heading = heading;
                        }
                        teleport_nonce = nonce;
                    }
                    MessageS2C::LoadMap {
                        offset,
                        data,
                        finished,
                    } => {
                        for i in 0..data.len() {
                            world.blocks[offset + i] = Cell::new(data[i]);
                        }
                        network.send(MessageC2S::MapChunkLoaded);
                        if finished {
                            for x in 0..world.dim_x.div_ceil(RENDER_CHUNK_SIZE) {
                                for y in 0..world.dim_z.div_ceil(RENDER_CHUNK_SIZE) {
                                    for z in 0..world.dim_z.div_ceil(RENDER_CHUNK_SIZE) {
                                        chunk_build_list.insert((x, y, z));
                                    }
                                }
                            }
                        }
                    }
                    MessageS2C::SetMovement(movement) => {
                        player_movement = movement;
                        character_controller.gravity = movement.gravity;
                    }
                    MessageS2C::SetSlot { slot, item, lock } => {
                        hotbar[slot] = (item, lock);
                    }
                    MessageS2C::SetItemCount { item, count } => {
                        item_counts[item as usize] = count;
                    }
                },
                Err(TryRecvError::Disconnected) => {
                    network.close();
                    println!("disconnect");
                    return;
                }
                Err(TryRecvError::Empty) => {
                    break;
                }
            }
        }

        for (cx, cy, cz) in chunk_build_list.drain() {
            let get_block =
                |x: usize, y: usize, z: usize, offset: Option<Face>| -> Option<AssetId> {
                    let (x, y, z) = match offset {
                        Some(offset) => (
                            x.wrapping_add_signed(offset.offset_x()),
                            y.wrapping_add_signed(offset.offset_y()),
                            z.wrapping_add_signed(offset.offset_z()),
                        ),
                        None => (x, y, z),
                    };
                    world.get(x, y, z).ok()
                };
            let bx = cx * RENDER_CHUNK_SIZE;
            let by = cy * RENDER_CHUNK_SIZE;
            let bz = cz * RENDER_CHUNK_SIZE;

            let mut vertices = Vec::new();
            let mut indices = Vec::new();
            let mut tex_coords = Vec::new();
            let mut colors = Vec::new();

            let vertex_offsets: [_; 8] = std::array::from_fn(|i| Vector3 {
                x: if i & 0x1 != 0 { 1. } else { 0. },
                y: if i & 0x2 != 0 { 1. } else { 0. },
                z: if i & 0x4 != 0 { 1. } else { 0. },
            });

            for ix in 0..RENDER_CHUNK_SIZE {
                for iy in 0..RENDER_CHUNK_SIZE {
                    for iz in 0..RENDER_CHUNK_SIZE {
                        let block = get_block(bx + ix, by + iy, bz + iz, None).unwrap_or(0);
                        if let Some(faces) =
                            &registries.as_ref().unwrap().blocks[block as usize].render_data
                        {
                            for face in Face::all() {
                                let neighbor =
                                    get_block(bx + ix, by + iy, bz + iz, Some(face)).unwrap_or(0);
                                if registries.as_ref().unwrap().blocks[neighbor as usize]
                                    .render_data
                                    .is_none()
                                {
                                    let vts = match face {
                                        Face::NZ => [3, 2, 0, 1],
                                        Face::Z => [6, 7, 5, 4],
                                        Face::Y => [2, 3, 7, 6],
                                        Face::NY => [1, 0, 4, 5],
                                        Face::NX => [2, 6, 4, 0],
                                        Face::X => [7, 3, 1, 5],
                                    }
                                    .map(|i| {
                                        vertex_offsets[i]
                                            + Vector3 {
                                                x: (bx + ix) as f32,
                                                y: (by + iy) as f32,
                                                z: (bz + iz) as f32,
                                            }
                                    });

                                    let texture = &textures.as_ref().unwrap().entries
                                        [faces[face as usize] as usize];
                                    let texture = match texture {
                                        TextureEntry::Block(texture) => *texture,
                                        TextureEntry::Texture(_) => unreachable!(),
                                    };
                                    tex_coords.push(Vector2 {
                                        x: texture[0][0],
                                        y: texture[0][1],
                                    });
                                    tex_coords.push(Vector2 {
                                        x: texture[1][0],
                                        y: texture[0][1],
                                    });
                                    tex_coords.push(Vector2 {
                                        x: texture[1][0],
                                        y: texture[1][1],
                                    });
                                    tex_coords.push(Vector2 {
                                        x: texture[0][0],
                                        y: texture[1][1],
                                    });
                                    let shade = match face {
                                        Face::X | Face::NX => 200,
                                        Face::Z | Face::NZ => 150,
                                        Face::Y => 255,
                                        Face::NY => 120,
                                    };
                                    colors.extend(std::iter::repeat_n(
                                        Color {
                                            r: shade,
                                            g: shade,
                                            b: shade,
                                            a: 255,
                                        },
                                        4,
                                    ));

                                    let start_index = vertices.len() as u16;
                                    vertices.extend_from_slice(&vts);
                                    indices.push(start_index + 0);
                                    indices.push(start_index + 3);
                                    indices.push(start_index + 2);
                                    indices.push(start_index + 2);
                                    indices.push(start_index + 1);
                                    indices.push(start_index + 0);
                                }
                            }
                        }
                    }
                }
            }

            if indices.len() <= 0 {
                render_chunks.remove(&(cx, cy, cz));
            } else {
                let mut mesh = MeshBuilder::new(&vertices, &tex_coords)
                    .indices(&indices)
                    .colors(&colors)
                    .build(&thread)
                    .unwrap();
                let mut model = rl
                    .load_model_from_mesh(&thread, unsafe { mesh.make_weak() })
                    .unwrap();
                model.materials_mut()[0].set_material_texture(
                    MaterialMapIndex::MATERIAL_MAP_ALBEDO,
                    &textures.as_ref().unwrap().atlas,
                );
                render_chunks.insert((cx, cy, cz), model);
            }
        }

        if !show_inventory {
            let sensitivity = 0.005;
            player_heading.yaw += rl.get_mouse_delta().x * sensitivity;
            player_heading.pitch -= rl.get_mouse_delta().y * sensitivity;
            player_heading.pitch = player_heading.pitch.clamp(-PI / 2. * 0.99, PI / 2. * 0.99);
        }

        let Some(registries) = &registries else {
            continue;
        };
        let mut move_vector = Vector3::zero();
        let vec_front = Vector3 {
            x: player_heading.yaw.sin(),
            y: 0.,
            z: -player_heading.yaw.cos(),
        };
        let vec_right = Vector3 {
            x: player_heading.yaw.cos(),
            y: 0.,
            z: player_heading.yaw.sin(),
        };
        for (key, v) in [
            (KeyboardKey::KEY_W, vec_front),
            (KeyboardKey::KEY_S, -vec_front),
            (KeyboardKey::KEY_A, -vec_right),
            (KeyboardKey::KEY_D, vec_right),
            (KeyboardKey::KEY_LEFT_SHIFT, -Vector3::Y),
            (KeyboardKey::KEY_SPACE, Vector3::Y),
        ] {
            if rl.is_key_down(key) {
                move_vector += v;
            }
        }
        let move_vector = if move_vector.length_sqr() > 0. {
            move_vector.normalize() * player_movement.speed
        } else {
            Vector3::ZERO
        };
        character_controller.input(
            move_vector,
            player_movement.fly,
            player_movement.speed * 8.,
            rl.get_frame_time(),
        );
        if !player_movement.fly && move_vector.y > 0. && character_controller.on_ground {
            let t = (2. * player_movement.jump_height / character_controller.gravity).sqrt();
            character_controller.velocity.y = character_controller.gravity * t;
        }

        character_controller.tick(
            &mut player_position,
            &world,
            player_kind,
            registries,
            rl.get_frame_time(),
        );

        network.send(MessageC2S::UpdatePosition {
            position: [player_position.x, player_position.y, player_position.z],
            heading: player_heading,
            nonce: teleport_nonce,
        });

        let cam_pos =
            player_position + Vector3::Y * registries.entities[player_kind as usize].eye_height;
        let cam_dir = Vector3 {
            x: player_heading.yaw.sin() * player_heading.pitch.cos(),
            y: player_heading.pitch.sin(),
            z: -player_heading.yaw.cos() * player_heading.pitch.cos(),
        };
        for (i, key) in [
            KeyboardKey::KEY_ONE,
            KeyboardKey::KEY_TWO,
            KeyboardKey::KEY_THREE,
            KeyboardKey::KEY_FOUR,
            KeyboardKey::KEY_FIVE,
            KeyboardKey::KEY_SIX,
            KeyboardKey::KEY_SEVEN,
            KeyboardKey::KEY_EIGHT,
            KeyboardKey::KEY_NINE,
            KeyboardKey::KEY_ZERO,
        ]
        .into_iter()
        .enumerate()
        {
            if rl.is_key_pressed(key) {
                hotbar_slot = i;
            }
        }
        let mut hit_distance = 6.;
        for (button, is_right) in [
            (MouseButton::MOUSE_BUTTON_LEFT, false),
            (MouseButton::MOUSE_BUTTON_RIGHT, true),
        ] {
            if rl.is_mouse_button_pressed(button) && !show_inventory {
                let mut block_hit = None;
                let min_hit_time = f32::INFINITY;
                let ray_start = (cam_pos.x, cam_pos.y, cam_pos.z);
                let ray_end = (
                    cam_pos.x + cam_dir.x * hit_distance,
                    cam_pos.y + cam_dir.y * hit_distance,
                    cam_pos.z + cam_dir.z * hit_distance,
                );
                voxel_traversal::voxel_traversal(ray_start, ray_end, |pos, normal| {
                    if pos.0 < 0 || pos.1 < 0 || pos.2 < 0 {
                        return false;
                    }
                    let d = (
                        ray_end.0 - ray_start.0,
                        ray_end.1 - ray_start.1,
                        ray_end.2 - ray_start.2,
                    );
                    let t = if normal.0 != 0 {
                        let plane = pos.0 as f32 + if pos.0 > 0 { 1.0 } else { 0.0 };
                        (plane - ray_start.0) / d.0
                    } else if normal.1 != 0 {
                        let plane = pos.1 as f32 + if pos.1 > 0 { 1.0 } else { 0.0 };
                        (plane - ray_start.1) / d.1
                    } else {
                        let plane = pos.2 as f32 + if pos.2 > 0 { 1.0 } else { 0.0 };
                        (plane - ray_start.2) / d.2
                    };
                    hit_distance = t;

                    let block = world
                        .get(pos.0 as usize, pos.1 as usize, pos.2 as usize)
                        .unwrap_or(0);
                    let block = &registries.blocks[block as usize];
                    if block.selectable {
                        block_hit = Some((
                            [pos.0 as usize, pos.1 as usize, pos.2 as usize],
                            Face::all()
                                .into_iter()
                                .find(|face| {
                                    face.offset_x() == normal.0 as isize
                                        && face.offset_y() == normal.1 as isize
                                        && face.offset_z() == normal.2 as isize
                                })
                                .unwrap_or(Face::Y),
                        ));
                        true
                    } else {
                        false
                    }
                });
                let mut entity_hit = None;
                for (id, entity) in &entities {
                    if *id == player_id {
                        continue;
                    }
                    let entity_kind = &registries.entities[entity.kind as usize];
                    let hitbox = HitBox::from_entity(Vector3::from(entity.position), entity_kind);
                    if let Some((t, _)) = voxel_traversal::intersect_aabb(
                        ray_start,
                        ray_end,
                        (hitbox.min.x, hitbox.min.y, hitbox.min.z),
                        (hitbox.max.x, hitbox.max.y, hitbox.max.z),
                    ) {
                        if t < hit_distance {
                            entity_hit = Some(*id);
                            block_hit = None;
                        }
                    }
                }
                network.send(MessageC2S::Click {
                    is_right,
                    heading: player_heading,
                    item: hotbar[hotbar_slot].0,
                    block: block_hit,
                    entity: entity_hit,
                });
            }
        }

        if rl.is_key_pressed(KeyboardKey::KEY_TAB) {
            show_inventory ^= true;
            if show_inventory {
                rl.enable_cursor();
            } else {
                rl.disable_cursor();
            }
        }

        let mut d = rl.begin_drawing(&thread);
        d.clear_background(Color::new(134, 182, 207, 255));

        let Some(textures) = &textures else { return };

        let camera = Camera3D::perspective(cam_pos, cam_pos + cam_dir, Vector3::Y, 90.);
        d.draw_mode3D(camera, |mut ctx| {
            for chunk_model in render_chunks.values() {
                ctx.draw_model(chunk_model, Vector3::ZERO, 1., Color::WHITE);
            }
            for (id, entity) in &entities {
                if *id == player_id {
                    continue;
                }
                let info = &registries.entities[entity.kind as usize];
                let (texture, source) = textures.get_with_rect(info.texture);
                ctx.draw_billboard_pro(
                    camera,
                    **texture,
                    source,
                    Vector3::from(entity.position) + Vector3::Y * (info.height / 2.),
                    Vector3::Y,
                    Vector2 {
                        x: source.width / source.height * info.height,
                        y: info.height,
                    },
                    Vector2::ZERO,
                    0.,
                    Color::WHITE,
                );
            }
        });
        let render_w = d.get_render_width() as f32;
        let render_h = d.get_render_height() as f32;
        const SLOT_SIZE: f32 = 100.;
        for i in 0..10 {
            slot_renderer.draw(
                Rectangle {
                    x: render_w / 2. + (i as f32 - 5.) * SLOT_SIZE * 1.3,
                    y: render_h - SLOT_SIZE * 1.5,
                    width: SLOT_SIZE,
                    height: SLOT_SIZE,
                },
                hotbar_slot == i,
                hotbar[i].1,
                hotbar[i].0.map(|item| (item, item_counts[item as usize])),
                &mut d,
                textures,
                registries,
            );
        }
        if show_inventory {
            let mouse = Vector2 {
                x: d.get_mouse_x() as f32
                    * (d.get_render_width() as f32 / d.get_screen_width() as f32),
                y: d.get_mouse_y() as f32
                    * (d.get_render_height() as f32 / d.get_screen_height() as f32),
            };
            for i in 0..registries.items.len() {
                let x = (i % 10) as f32;
                let y = (i / 10) as f32;
                let rect = Rectangle {
                    x: render_w / 2. + (x - 5.) * SLOT_SIZE * 1.3,
                    y: render_h / 2.
                        + (y - (registries.items.len() / 10) as f32 / 2.) * SLOT_SIZE * 1.5,
                    width: SLOT_SIZE,
                    height: SLOT_SIZE,
                };
                if d.is_mouse_button_pressed(MouseButton::MOUSE_BUTTON_LEFT) {
                    if rect.check_collision_point_rec(mouse) {
                        let mut sel = &mut hotbar[hotbar_slot];
                        let new_item = i as AssetId;
                        if !sel.1 {
                            match sel.0 {
                                Some(item) => {
                                    if item == new_item {
                                        sel.0 = None;
                                    } else {
                                        sel.0 = Some(new_item);
                                    }
                                }
                                None => {
                                    sel.0 = Some(new_item);
                                }
                            }
                        }
                    }
                }
                slot_renderer.draw(
                    rect,
                    false,
                    false,
                    Some((i as u16, item_counts[i])),
                    &mut d,
                    textures,
                    registries,
                );
            }
        }
    }
}
struct ClientEntity {
    position: [f32; 3],
    kind: AssetId,
    name: String,
    heading: Heading,
}
struct ClientNetworkSender(NodeHandler<()>, Endpoint);
impl ClientNetworkSender {
    pub fn send(&self, message: MessageC2S) {
        self.0
            .network()
            .send(self.1, &postcard::to_stdvec(&message).unwrap());
    }
    pub fn close(&self) {
        self.0.stop();
    }
}
fn network_thread(addr: &str) -> (ClientNetworkSender, Receiver<MessageS2C<'static>>) {
    let (handler, listener) = node::split::<()>();
    let (endpoint, _) = handler
        .network()
        .connect(Transport::FramedTcp, addr)
        .unwrap();

    let (tx, rx) = channel();
    thread::spawn(move || {
        let mut tx = Some(tx);
        listener.for_each(move |event| match event.network() {
            NetEvent::Connected(_, _) => {}
            NetEvent::Accepted(endpoint, _listener) => unreachable!(),
            NetEvent::Message(endpoint, data) => {
                let Some(tx) = &tx else {
                    return;
                };
                tx.send(postcard::from_bytes(data).unwrap()).unwrap();
            }
            NetEvent::Disconnected(_endpoint) => {
                tx = None;
            }
        });
    });
    (ClientNetworkSender(handler, endpoint), rx)
}
struct SlotRenderer {
    slot: Texture2D,
    selected_slot: Texture2D,
    lock: Texture2D,
}
impl SlotRenderer {
    fn new(rl: &mut RaylibHandle, thread: &RaylibThread) -> Self {
        Self {
            slot: TextureManager::load_dynamic_image(
                &image::load_from_memory(include_bytes!("builtin/slot.png")).unwrap(),
                rl,
                &thread,
            ),
            selected_slot: TextureManager::load_dynamic_image(
                &image::load_from_memory(include_bytes!("builtin/slot_selected.png")).unwrap(),
                rl,
                &thread,
            ),
            lock: TextureManager::load_dynamic_image(
                &image::load_from_memory(include_bytes!("builtin/lock.png")).unwrap(),
                rl,
                &thread,
            ),
        }
    }
    fn draw(
        &self,
        rect: Rectangle,
        selected: bool,
        locked: bool,
        item: Option<(AssetId, u32)>,
        d: &mut RaylibDrawHandle,
        textures: &TextureManager,
        registries: &Registries,
    ) {
        let src = Rectangle {
            x: 0.,
            y: 0.,
            width: 16.,
            height: 16.,
        };
        if let Some((item, count)) = item {
            let (texture, src) = textures.get_with_rect(registries.items[item as usize].0);
            d.draw_texture_pro(texture, src, rect, Vector2::ZERO, 0., Color::WHITE);
            if count > 0 {
                d.draw_text(
                    &format!("{count}"),
                    rect.x as i32,
                    rect.y as i32,
                    10,
                    Color::WHITE,
                );
            }
        }
        d.draw_texture_pro(
            if selected {
                &self.selected_slot
            } else {
                &self.slot
            },
            src,
            rect,
            Vector2::ZERO,
            0.,
            Color::WHITE,
        );
        if locked {
            d.draw_texture_pro(&self.lock, src, rect, Vector2::ZERO, 0., Color::WHITE);
        }
    }
}
