player = register_entity(0, 0.3, 1.9, 1.7)
stone_texture = load_texture("stone.png", true)
dirt_texture = load_texture("dirt.png", true)
grass_texture = load_texture("grass.png", true)
stone_block = register_block({stone_texture, stone_texture, stone_texture, stone_texture, stone_texture, stone_texture})
dirt_block = register_block({dirt_texture, dirt_texture, dirt_texture, dirt_texture, dirt_texture, dirt_texture})
grass_block = register_block({dirt_texture, dirt_texture, dirt_texture, dirt_texture, grass_texture, dirt_texture})
stone_item = register_item(stone_texture)
dirt_item = register_item(dirt_texture)
grass_item = register_item(grass_texture)
item_to_block = {}
item_to_block[stone_item] = stone_block
item_to_block[dirt_item] = dirt_block
item_to_block[grass_item] = grass_block

function start()
    for x=1,255 do 
        for z=1,255 do 
            height = (x+z)%5
            for y=1,height do 
                set_block(x, y, z, dirt_block)
            end
            set_block(x, height, z, grass_block)
        end
    end
end

function tick()

end

function login(name, cookie)
    return player, 20, 30, 20, {}
end

function click(event)
    if event.is_right then
        if event.block and event.item then
            mapped_block = item_to_block[event.item]
            if mapped_block then
                bx = event.block_x + event.block_face_x
                by = event.block_y + event.block_face_y
                bz = event.block_z + event.block_face_z
                for _, entity in ipairs(entities()) do
                    for _, b in ipairs(entity:colliding_blocks(entity.x, entity.y, entity.z, 0)) do
                        if b.x == bx and b.y == by and b.z == bz then
                            return
                        end
                    end
                end

                set_block(bx, by, bz, mapped_block)
            end
        end
    else
        if event.block then
            set_block(event.block_x, event.block_y, event.block_z, 0)
        end
    end
end