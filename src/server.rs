use std::{borrow::Cow, cell::RefCell, collections::HashMap, rc::Rc, time::Duration};

use image::{DynamicImage, Rgba, RgbaImage, imageops};
use message_io::{
    network::{Endpoint, NetEvent, ResourceId, Transport},
    node::{self, NodeEvent, NodeHandler},
};
use mlua::{
    Function, IntoLua, Lua, MetaMethod, Table, UserData,
    Value::{self, Nil},
};
use raylib::ffi::Vector3;
use serialimage::DynamicSerialImage;
use uuid::Uuid;

use crate::{
    common::{
        AssetId, BlockBuffer, BlockInfo, CharacterController, EntityInfo, Heading, HitBox,
        ItemInfo, PlayerMovement, Registries, TextureInfo,
    },
    net::{MessageC2S, MessageS2C},
};

const MAP_CHUNK_SIZE: usize = 64 * 64 * 64;

enum ServerSignal {
    Tick,
}
pub const TICK_RATE: u64 = 20;
pub const DT: f32 = 1. / TICK_RATE as f32;

struct ServerEntity {
    pub position: Vector3,
    pub heading: Heading,
    pub kind: AssetId,
    pub name: String,
    pub data: Table,
    pub should_remove: bool,
    pub client: Option<ResourceId>,
    pub character: CharacterController,
}
struct ScriptEntity(Uuid);
impl UserData for ScriptEntity {
    fn add_methods<M: mlua::prelude::LuaUserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("kill", |lua, this, _: ()| {
            let mut server = lua.app_data_mut::<Server>().unwrap();
            let Some(entity) = server.entities.get_mut(&this.0) else {
                return Ok(Value::Nil);
            };
            if entity.client.is_none() {
                entity.should_remove = true;
            }
            Ok(Value::Nil)
        });
        methods.add_method("teleport", |lua, this, (x, y, z): (f32, f32, f32)| {
            let mut server = lua.app_data_mut::<Server>().unwrap();
            let Some(entity) = server.entities.get_mut(&this.0) else {
                return Ok(Value::Nil);
            };
            let client = entity.client;
            entity.position = Vector3 { x, y, z };
            if let Some(client) = client {
                let (endpoint, next_nonce) = match server.clients.get_mut(&client) {
                    Some(client) => {
                        client.teleport_nonce += 1;
                        (client.endpoint, client.teleport_nonce)
                    }
                    None => return Ok(Value::Nil),
                };
                server.send_message(
                    endpoint,
                    &MessageS2C::Teleport {
                        position: [x, y, z],
                        heading: None,
                        nonce: next_nonce,
                    },
                );
            }
            Ok(Value::Nil)
        });
        methods.add_method(
            "teleport_heading",
            |lua, this, (x, y, z, yaw, pitch): (f32, f32, f32, f32, f32)| {
                let mut server = lua.app_data_mut::<Server>().unwrap();
                let Some(entity) = server.entities.get_mut(&this.0) else {
                    return Ok(Value::Nil);
                };
                let client = entity.client;
                entity.position = Vector3 { x, y, z };
                entity.heading = Heading { yaw, pitch };
                if let Some(client) = client {
                    let (endpoint, next_nonce) = match server.clients.get_mut(&client) {
                        Some(client) => {
                            client.teleport_nonce += 1;
                            (client.endpoint, client.teleport_nonce)
                        }
                        None => return Ok(Value::Nil),
                    };
                    server.send_message(
                        endpoint,
                        &MessageS2C::Teleport {
                            position: [x, y, z],
                            heading: Some(Heading { yaw, pitch }),
                            nonce: next_nonce,
                        },
                    );
                }
                Ok(Value::Nil)
            },
        );
        methods.add_method(
            "colliding_blocks",
            |lua, this, (x, y, z, inflate): (f32, f32, f32, f32)| {
                let mut server = lua.app_data_mut::<Server>().unwrap();
                let Some(entity) = server.entities.get_mut(&this.0) else {
                    return Ok(Value::Nil);
                };
                let mut master_table = lua.create_table().unwrap();
                let kind = entity.kind;
                for block in HitBox::from_entity(
                    Vector3 { x, y, z },
                    &server.registries.entities[kind as usize],
                )
                .inflate(Vector3::one() * inflate)
                .inflate(Vector3::one() * -inflate)
                .block_iter()
                {
                    let mut table = lua.create_table().unwrap();
                    table.set("x", block[0]);
                    table.set("y", block[1]);
                    table.set("z", block[2]);
                    master_table.push(table);
                }
                Ok(Value::Table(master_table))
            },
        );
        methods.add_method(
            "set_item_count",
            |lua, this, (item, count): (AssetId, u32)| {
                let mut server = lua.app_data_mut::<Server>().unwrap();
                let Some(entity) = server.entities.get_mut(&this.0) else {
                    return Ok(Value::Nil);
                };
                let Some(client) = entity.client.and_then(|client| server.clients.get(&client))
                else {
                    return Ok(Value::Nil);
                };
                server.send_message(client.endpoint, &MessageS2C::SetItemCount { item, count });
                Ok(Value::Nil)
            },
        );
        methods.add_method(
            "set_hotbar_slot",
            |lua, this, (slot, item, lock): (usize, Option<AssetId>, bool)| {
                let mut server = lua.app_data_mut::<Server>().unwrap();
                let Some(entity) = server.entities.get_mut(&this.0) else {
                    return Ok(Value::Nil);
                };
                let Some(client) = entity.client.and_then(|client| server.clients.get(&client))
                else {
                    return Ok(Value::Nil);
                };
                server.send_message(client.endpoint, &MessageS2C::SetSlot { item, lock, slot });
                Ok(Value::Nil)
            },
        );
        methods.add_method(
            "move_input",
            |lua, this, (x, y, z, accel_y, acceleration): (f32, f32, f32, bool, f32)| {
                let mut server = lua.app_data_mut::<Server>().unwrap();
                let Some(entity) = server.entities.get_mut(&this.0) else {
                    return Ok(Value::Nil);
                };
                entity
                    .character
                    .input(Vector3 { x, y, z }, accel_y, acceleration, DT);
                Ok(Value::Nil)
            },
        );
        methods.add_method("set_physics", |lua, this, (physics): (Table)| {
            let mut server = lua.app_data_mut::<Server>().unwrap();
            let server = &mut *server;
            let Some(entity) = server.entities.get_mut(&this.0) else {
                return Ok(Value::Nil);
            };
            if let Ok(drag) = physics.get::<f32>("drag") {
                entity.character.drag = drag;
            }
            let mut client = match entity.client {
                Some(client) => server.clients.get_mut(&client),
                None => None,
            };
            if let Ok(gravity) = physics.get::<f32>("gravity") {
                entity.character.gravity = gravity;
                if let Some(client) = client.as_mut() {
                    client.movement.gravity = gravity;
                }
            }
            if let Some(client) = client {
                if let Ok(speed) = physics.get::<f32>("speed") {
                    client.movement.speed = speed;
                }
                if let Ok(jump_height) = physics.get::<f32>("jump_height") {
                    client.movement.jump_height = jump_height;
                }
                if let Ok(fly) = physics.get::<bool>("fly") {
                    client.movement.fly = fly;
                }

                let message = MessageS2C::SetMovement(client.movement);
                let ep = client.endpoint;
                server.send_message(ep, &message);
            }
            Ok(Value::Nil)
        });
        methods.add_meta_method(MetaMethod::Index, |lua, this, key: Value| {
            let server = lua.app_data_ref::<Server>().unwrap();
            let Some(entity) = server.entities.get(&this.0) else {
                return Ok(Value::Nil);
            };
            if let Some(key) = key.as_string() {
                match key.to_string_lossy().as_ref() {
                    "player" => return Ok(Value::Boolean(entity.client.is_some())),
                    "x" => return Ok(Value::Number(entity.position.x.into())),
                    "y" => return Ok(Value::Number(entity.position.y.into())),
                    "z" => return Ok(Value::Number(entity.position.z.into())),
                    "pitch" => return Ok(Value::Number(entity.heading.pitch.into())),
                    "yaw" => return Ok(Value::Number(entity.heading.yaw.into())),
                    _ => {}
                }
            }
            entity.data.raw_get(key)
        });
        methods.add_meta_method(
            MetaMethod::NewIndex,
            |lua, this, (key, value): (Value, Value)| {
                let server = lua.app_data_ref::<Server>().unwrap();
                let Some(entity) = server.entities.get(&this.0) else {
                    return Ok(Value::Nil);
                };
                entity.data.set(key, value);
                Ok(Value::Nil)
            },
        );
    }
}

struct Server {
    clients: HashMap<ResourceId, ClientConnection>,
    world: BlockBuffer,
    entities: HashMap<Uuid, ServerEntity>,
    registries: Registries,
    network: NodeHandler<ServerSignal>,
}
impl Server {
    pub fn spawn_entity(&mut self, entity: ServerEntity) -> Uuid {
        let id = Uuid::new_v4();
        self.broadcast_message(MessageS2C::SpawnEntity {
            id,
            position: [entity.position.x, entity.position.y, entity.position.z],
            kind: entity.kind,
            name: entity.name.clone(),
            heading: entity.heading,
        });
        self.entities.insert(id, entity);
        id
    }
    pub fn send_message(&self, endpoint: Endpoint, message: &MessageS2C) {
        self.network
            .network()
            .send(endpoint, &postcard::to_stdvec(&message).unwrap());
    }
    pub fn broadcast_message(&self, message: MessageS2C) {
        for client in self.clients.values() {
            self.send_message(client.endpoint, &message);
        }
    }
}

pub fn server(port: u16) {
    let mut lua = Lua::new();
    let mut registries = Registries {
        blocks: vec![BlockInfo {
            collision: false,
            selectable: false,
            render_data: None,
        }],
        entities: vec![],
        items: vec![],
        textures: vec![],
    };
    {
        let registries = RefCell::new(&mut registries);
        lua.scope(|scope| {
            lua.globals().set(
                "register_block",
                scope
                    .create_function_mut(|_, (textures): (Vec<i32>)| {
                        let mut registries = registries.borrow_mut();
                        let mut registries = &mut **registries;
                        let textures = textures
                            .into_iter()
                            .map(|texture| {
                                if texture < 0 || texture >= registries.textures.len() as i32 {
                                    panic!()
                                }
                                if !registries.textures[texture as usize].is_block {
                                    panic!()
                                }
                                texture as AssetId
                            })
                            .collect::<Vec<AssetId>>()
                            .try_into()
                            .unwrap();
                        registries.blocks.push(BlockInfo {
                            collision: true,
                            selectable: true,
                            render_data: Some(textures),
                        });
                        Ok(registries.blocks.len() - 1)
                    })
                    .unwrap(),
            );
            lua.globals().set(
                "load_texture",
                scope
                    .create_function_mut(|_, (texture, is_block): (String, bool)| {
                        let mut registries = registries.borrow_mut();
                        let mut registries = &mut **registries;
                        let image = image::open(texture).unwrap();
                        registries.textures.push(TextureInfo {
                            image: DynamicSerialImage::from(image),
                            is_block,
                        });
                        Ok(registries.textures.len() - 1)
                    })
                    .unwrap(),
            );
            lua.globals().set(
                "register_entity",
                scope
                    .create_function_mut(
                        |_, (texture, size, height, eye_height): (AssetId, f32, f32, f32)| {
                            let mut registries = registries.borrow_mut();
                            let mut registries = &mut **registries;
                            registries.entities.push(EntityInfo {
                                texture,
                                size,
                                height,
                                eye_height,
                            });
                            Ok(registries.entities.len() - 1)
                        },
                    )
                    .unwrap(),
            );
            lua.globals().set(
                "register_item",
                scope
                    .create_function_mut(|_, (texture): (AssetId)| {
                        let mut registries = registries.borrow_mut();
                        let mut registries = &mut **registries;
                        registries.items.push(ItemInfo(texture));
                        Ok(registries.items.len() - 1)
                    })
                    .unwrap(),
            );
            let _: () = lua
                .load(std::fs::read_to_string("script.lua").unwrap())
                .eval()
                .unwrap();
            Ok(())
        });
    }
    let (handler, listener) = node::split::<ServerSignal>();

    handler
        .network()
        .listen(Transport::FramedTcp, format!("0.0.0.0:{port}"))
        .unwrap();

    handler.signals().send(ServerSignal::Tick);

    let server = Server {
        clients: HashMap::new(),
        entities: HashMap::new(),
        network: handler.clone(),
        registries,
        world: BlockBuffer::filled(0, 256, 128, 256),
    };
    lua.set_app_data(server);

    {
        let globals = lua.globals();
        globals.set(
            "set_block",
            lua.create_function(|lua, (x, y, z, block): (usize, usize, usize, AssetId)| {
                let server = lua.app_data_mut::<Server>().unwrap();
                if server.world.set(block, x, y, z).is_ok() {
                    server.broadcast_message(MessageS2C::SetBlock { x, y, z, block });
                }
                Ok(())
            })
            .unwrap(),
        );
        globals.set(
            "get_block",
            lua.create_function(|lua, (x, y, z): (usize, usize, usize)| {
                let server = lua.app_data_mut::<Server>().unwrap();
                Ok(server.world.get(x, y, z).unwrap_or(0))
            })
            .unwrap(),
        );
        globals.set(
            "entities",
            lua.create_function(|lua, _: ()| {
                let server = lua.app_data_mut::<Server>().unwrap();
                Ok(server
                    .entities
                    .keys()
                    .map(|id| ScriptEntity(*id))
                    .collect::<Vec<_>>())
            })
            .unwrap(),
        );
        globals.set(
            "spawn_entity",
            lua.create_function(|lua, (id, x, y, z, pitch, yaw, data): (AssetId, f32, f32, f32, f32, f32, Table)|{
                let mut server = lua.app_data_mut::<Server>().unwrap();
                Ok(ScriptEntity(server.spawn_entity(ServerEntity {
                    position: Vector3 { x, y, z },
                    heading: Heading { yaw, pitch },
                    kind: id,
                    name: data.get::<String>("name").unwrap_or(String::new()),
                    data,
                    should_remove: false,
                    client: None,
                    character: CharacterController::default(),
                })))
            })
            .unwrap(),
        );
    }

    {
        let start_fn: Function = lua.globals().get("start").unwrap();
        let _: () = start_fn.call(()).unwrap();
    }
    listener.for_each(|event| match event {
        NodeEvent::Network(event) => match event {
            NetEvent::Connected(_, _) => unreachable!(),
            NetEvent::Accepted(endpoint, _listener) => {
                let mut server = lua.app_data_mut::<Server>().unwrap();
                server.clients.insert(
                    endpoint.resource_id(),
                    ClientConnection {
                        teleport_nonce: 1,
                        entity: None,
                        map_chunk_index: 1,
                        movement: PlayerMovement::default(),
                        endpoint,
                    },
                );
                server.send_message(
                    endpoint,
                    &MessageS2C::GameData {
                        world_x: server.world.dim_x,
                        world_y: server.world.dim_y,
                        world_z: server.world.dim_z,
                        registries: Cow::Owned(server.registries.clone()), //todo: borrowed
                    },
                );
            }
            NetEvent::Message(endpoint, data) => {
                let Ok(message) = postcard::from_bytes::<MessageC2S>(data) else {
                    return;
                };
                match message {
                    MessageC2S::Login { name, cookie } => {
                        let login_fn: Function = lua.globals().get("login").unwrap();

                        let (kind, x, y, z, data): (AssetId, f32, f32, f32, Table) =
                            login_fn.call((name.clone(), cookie)).unwrap();

                        let mut server = lua.app_data_mut::<Server>().unwrap();
                        let id = server.spawn_entity(ServerEntity {
                            position: Vector3 { x, y, z },
                            kind,
                            name,
                            heading: Heading { pitch: 0., yaw: 0. },
                            data,
                            client: Some(endpoint.resource_id()),
                            should_remove: false,
                            character: CharacterController::default(),
                        });
                        server.send_message(endpoint, &MessageS2C::SetPlayerEntity { kind, id });
                        server.send_message(
                            endpoint,
                            &MessageS2C::Teleport {
                                nonce: 1,
                                position: [x, y, z],
                                heading: None,
                            },
                        );
                        let chunk = create_map_chunk(&server.world, 0).unwrap();
                        server.send_message(endpoint, &chunk);
                        server
                            .clients
                            .get_mut(&endpoint.resource_id())
                            .unwrap()
                            .entity = Some(id);
                    }
                    MessageC2S::UpdatePosition {
                        position,
                        heading,
                        nonce,
                    } => {
                        let mut server = lua.app_data_mut::<Server>().unwrap();
                        let (client_nonce, entity) = {
                            let Some(client) = server.clients.get(&endpoint.resource_id()) else {
                                return;
                            };
                            let Some(entity) = client.entity else { return };
                            (client.teleport_nonce, entity)
                        };
                        if client_nonce != nonce {
                            return;
                        }
                        let Some(entity) = server.entities.get_mut(&entity) else {
                            return;
                        };
                        entity.position = Vector3 {
                            x: position[0],
                            y: position[1],
                            z: position[2],
                        };
                        entity.heading = heading;
                    }
                    MessageC2S::Click {
                        is_right,
                        heading,
                        item,
                        block,
                        entity,
                    } => {
                        let entity_id = {
                            let mut server = lua.app_data_mut::<Server>().unwrap();
                            let Some(client) = server.clients.get_mut(&endpoint.resource_id())
                            else {
                                return;
                            };
                            match client.entity {
                                Some(entity) => entity,
                                None => return,
                            }
                        };
                        let click_fn: Function = lua.globals().get("click").unwrap();
                        let event = lua.create_table().unwrap();
                        event.set("player", ScriptEntity(entity_id));
                        event.set("is_right", is_right);
                        if let Some(item) = item {
                            event.set("item", item);
                        }
                        event.set("block", block.is_some());
                        if let Some((block, face)) = block {
                            event.set("block_x", block[0]);
                            event.set("block_y", block[1]);
                            event.set("block_z", block[2]);
                            event.set("block_face_x", face.offset_x());
                            event.set("block_face_y", face.offset_y());
                            event.set("block_face_z", face.offset_z());
                        }
                        if let Some(entity) = entity {
                            event.set("entity", ScriptEntity(entity));
                        }
                        match click_fn.call(event) {
                            Ok(()) => {}
                            Err(error) => {
                                println!("{:?}", error);
                            }
                        }
                    }
                    MessageC2S::MapChunkLoaded => {
                        let mut server = lua.app_data_mut::<Server>().unwrap();
                        let next_id = match server.clients.get_mut(&endpoint.resource_id()) {
                            Some(client) => {
                                client.map_chunk_index += 1;
                                client.map_chunk_index - 1
                            }
                            None => return,
                        };
                        if let Some(next_chunk) = create_map_chunk(&server.world, next_id) {
                            server.send_message(endpoint, &next_chunk);
                        }
                    }
                }
            }
            NetEvent::Disconnected(endpoint) => {
                let mut server = lua.app_data_mut::<Server>().unwrap();
                server.clients.remove(&endpoint.resource_id());
            }
        },
        NodeEvent::Signal(signal) => match signal {
            ServerSignal::Tick => {
                handler
                    .signals()
                    .send_with_timer(ServerSignal::Tick, Duration::from_millis(1000 / TICK_RATE));
                {
                    let tick_fn: Function = lua.globals().get("tick").unwrap();
                    match tick_fn.call(()) {
                        Ok(()) => {}
                        Err(error) => {
                            println!("{:?}", error);
                        }
                    }
                }
                let mut server = lua.app_data_mut::<Server>().unwrap();
                let server = &mut *server;
                for (id, entity) in &mut server.entities {
                    entity.character.tick(
                        &mut entity.position,
                        &server.world,
                        entity.kind,
                        &server.registries,
                        DT,
                    );
                    let move_message = postcard::to_stdvec(&MessageS2C::MoveEntity {
                        id: *id,
                        position: [entity.position.x, entity.position.y, entity.position.z],
                        heading: entity.heading,
                    })
                    .unwrap();
                    for (_, client) in &server.clients {
                        server
                            .network
                            .network()
                            .send(client.endpoint, &move_message);
                    }
                }
                let to_remove = server
                    .entities
                    .extract_if(|_, entity| {
                        entity.should_remove
                            || match entity.client {
                                Some(client) => !server.clients.contains_key(&client),
                                None => false,
                            }
                    })
                    .collect::<Vec<_>>();
                for (id, _) in to_remove {
                    server.broadcast_message(MessageS2C::RemoveEntity(id));
                }
            }
        },
    });
}
fn create_map_chunk(map: &BlockBuffer, index: usize) -> Option<MessageS2C<'static>> {
    let start_index = index * MAP_CHUNK_SIZE;
    if start_index >= map.blocks.len() {
        return None;
    }
    let mut end_index = (start_index + MAP_CHUNK_SIZE);
    let finished = if end_index >= map.blocks.len() {
        end_index = map.blocks.len();
        true
    } else {
        false
    };
    Some(MessageS2C::LoadMap {
        offset: index * MAP_CHUNK_SIZE,
        data: map.blocks[start_index..end_index]
            .iter()
            .map(|c| c.get())
            .collect(),
        finished,
    })
}
struct ClientConnection {
    teleport_nonce: usize,
    entity: Option<Uuid>,
    map_chunk_index: usize,
    movement: PlayerMovement,
    endpoint: Endpoint,
}
