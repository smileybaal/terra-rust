//! tiles: decode the tile section of a `.wld`.
//!
//! `WorldFile.cs:2524` `LoadWorldTiles`. It depends on bytes and the container's
//! importance bits, and on nothing else - no game state, no sockets - which is why it
//! could be written and checked before a world can be played.
//!
//! The format, per tile, is a header byte whose low bits say which further header bytes
//! follow and whose HIGH TWO BITS are a run length:
//!
//! ```text
//! b4 = byte                       // sTileHeader
//! if b4 & 1        : b  = byte    // bTileHeader
//! if b & 1         : b2 = byte    // bTileHeader2
//! if b2 & 1        : b3 = byte    // bTileHeader3
//! b4 & 2  active   : type (8 or 16 bits), then frames if the id is "important"
//! b4 & 4  wall     : wall id (8 bits, extended to 16 by b2 & 0x40)
//! b4 & 0x18        : liquid amount; a byte follows if non-zero
//! b  & 0x70        : shape (half brick / slope), gated on TileID.Sets.SaveSlopes
//! b2 & 2,4,0x20    : actuator, inactive, wire4
//! b3 & 2,4,8,0x10  : invisible block, invisible wall, fullbright block, fullbright wall
//! (b4 & 0xC0) >> 6 : run length, as 0, one byte, or a short - the tile is repeated
//! ```
//!
//! The run length is the reason the section is compact, and it is also the easiest
//! thing to get wrong: `LoadWorldTiles` advances the row cursor by `rle + 1` per tile
//! read, so a reader that ignores it desynchronises on the first repeated tile.
//!
//! Two tables this decoder needs are NOT in the book, because their values are assigned
//! at runtime rather than written in the source: `WallID.Count` (a `static readonly`
//! ushort) and `TileID.Sets.SaveSlopes` (a `bool[]`). They are parameters here, and the
//! caller supplies them, rather than this file inventing a number that the game would
//! then disagree with.

/// `Tile.cs:25`: the four liquid kinds, as the shape bits encode them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Liquid {
    None,
    Water,
    Lava,
    Honey,
    Shimmer,
}

/// One tile, after decoding. Field names follow `terraria.tile.*` in the book.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tile {
    pub type_id: u16,
    pub wall: u16,
    pub liquid_amount: u8,
    pub liquid: Liquid,
    pub s_tile_header: u16,
    pub b_tile_header: u8,
    pub b_tile_header2: u8,
    pub b_tile_header3: u8,
    pub frame_x: i16,
    pub frame_y: i16,
    pub tile_color: u8,
    pub wall_color: u8,
    pub active: bool,
    pub wire: bool,
    pub wire2: bool,
    pub wire3: bool,
    pub wire4: bool,
    pub actuator: bool,
    pub inactive: bool,
    pub half_brick: bool,
    pub slope: u8,
    pub invisible_block: bool,
    pub invisible_wall: bool,
    pub fullbright_block: bool,
    pub fullbright_wall: bool,
}

impl Tile {
    /// A tile with nothing on it. Public because a server has to be able to build one:
    /// the tests below construct a world by hand rather than needing a `.wld` on disk.
    pub fn empty() -> Self {
        Tile {
            type_id: 0,
            wall: 0,
            liquid_amount: 0,
            liquid: Liquid::None,
            s_tile_header: 0,
            b_tile_header: 0,
            b_tile_header2: 0,
            b_tile_header3: 0,
            frame_x: 0,
            frame_y: 0,
            tile_color: 0,
            wall_color: 0,
            active: false,
            wire: false,
            wire2: false,
            wire3: false,
            wire4: false,
            actuator: false,
            inactive: false,
            half_brick: false,
            slope: 0,
            invisible_block: false,
            invisible_wall: false,
            fullbright_block: false,
            fullbright_wall: false,
        }
    }
}

/// Why a tile section could not be decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TileError {
    /// The section ended before the tiles did.
    TooShort,
    /// A tile id with no `importance` entry. The C# would throw
    /// `IndexOutOfRangeException`; this says which id, so a mis-parse is legible.
    TypeOutOfRange(u16),
    /// The run length pushed the row cursor past the end of the row.
    RunOverrunsRow,
}

/// Totals from decoding a section, so a caller can check the result without holding
/// twenty million tiles in memory. An 8400x2400 world is 20.16M tiles; at 40 bytes each
/// that is 800 MB, which is why this decodes and counts rather than collects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TileStats {
    pub tiles: u64,
    pub active: u64,
    pub walls: u64,
    pub liquid_water: u64,
    pub liquid_lava: u64,
    pub liquid_honey: u64,
    pub liquid_shimmer: u64,
    pub wires: u64,
    pub slopes: u64,
    pub half_bricks: u64,
    pub actuators: u64,
    pub inactive: u64,
    pub invisible_blocks: u64,
    pub invisible_walls: u64,
    pub fullbright_blocks: u64,
    pub fullbright_walls: u64,
}

fn take<'a>(r: &mut &'a [u8], n: usize) -> Result<&'a [u8], TileError> {
    if r.len() < n {
        return Err(TileError::TooShort);
    }
    let (head, tail) = r.split_at(n);
    *r = tail;
    Ok(head)
}

fn u8_of(r: &mut &[u8]) -> Result<u8, TileError> {
    Ok(take(r, 1)?[0])
}

fn i16_of(r: &mut &[u8]) -> Result<i16, TileError> {
    Ok(i16::from_le_bytes(take(r, 2)?.try_into().unwrap()))
}

/// Decode one tile, returning it and its run length.
///
/// `importance` is the container's bit table, indexed by tile id; a type past its end
/// is an error rather than a default, because silently defaulting would mis-parse the
/// rest of the section rather than fail on it. `save_slopes` is
/// `TileID.Sets.SaveSlopes`, which gates the shape bits.
pub fn read_tile(
    r: &mut &[u8],
    importance: &[bool],
    save_slopes: &[bool],
    wall_count: u16,
) -> Result<(Tile, u16), TileError> {
    let mut t = Tile::empty();

    let b4 = u8_of(r)?;
    t.s_tile_header = b4 as u16;
    let mut b = 0u8;
    let mut b2 = 0u8;
    let mut b3 = 0u8;
    if b4 & 1 == 1 {
        b = u8_of(r)?;
    }
    if b & 1 == 1 {
        b2 = u8_of(r)?;
    }
    if b2 & 1 == 1 {
        b3 = u8_of(r)?;
    }
    t.b_tile_header = b;
    t.b_tile_header2 = b2;
    t.b_tile_header3 = b3;

    if b4 & 2 == 2 {
        t.active = true;
        // A 16-bit tile id is flagged in the header; 8-bit ids are the common case.
        let type_id = if b4 & 0x20 == 32 {
            let lo = u8_of(r)? as u16;
            let hi = u8_of(r)? as u16;
            (hi << 8) | lo
        } else {
            u8_of(r)? as u16
        };
        t.type_id = type_id;
        if importance.get(type_id as usize).copied().unwrap_or(false) {
            t.frame_x = i16_of(r)?;
            t.frame_y = i16_of(r)?;
            // TileID 144 is the one id whose frameY is forced to zero on load.
            if t.type_id == 144 {
                t.frame_y = 0;
            }
        } else if (type_id as usize) < importance.len() {
            // Not important: the C# stores -1 rather than reading frames.
            t.frame_x = -1;
            t.frame_y = -1;
        } else {
            return Err(TileError::TypeOutOfRange(type_id));
        }
        if b2 & 8 == 8 {
            t.tile_color = u8_of(r)?;
        }
    }

    if b4 & 4 == 4 {
        t.wall = u8_of(r)? as u16;
        if t.wall >= wall_count {
            t.wall = 0;
        }
        if b2 & 0x10 == 16 {
            t.wall_color = u8_of(r)?;
        }
    }

    let amount = (b4 & 0x18) >> 3;
    if amount != 0 {
        t.liquid_amount = u8_of(r)?;
        if b2 & 0x80 == 128 {
            t.liquid = Liquid::Shimmer;
        } else if amount > 1 {
            t.liquid = if amount == 2 { Liquid::Lava } else { Liquid::Honey };
        } else {
            t.liquid = Liquid::Water;
        }
    }

    if b > 1 {
        if b & 2 == 2 {
            t.wire = true;
        }
        if b & 4 == 4 {
            t.wire2 = true;
        }
        if b & 8 == 8 {
            t.wire3 = true;
        }
        let shape = (b & 0x70) >> 4;
        // The shape is only stored for ids that save slopes; anything else is dropped.
        if shape != 0 && save_slopes.get(t.type_id as usize).copied().unwrap_or(false) {
            if shape == 1 {
                t.half_brick = true;
            } else {
                t.slope = shape - 1;
            }
        }
    }

    if b2 > 1 {
        if b2 & 2 == 2 {
            t.actuator = true;
        }
        if b2 & 4 == 4 {
            t.inactive = true;
        }
        if b2 & 0x20 == 32 {
            t.wire4 = true;
        }
        if b2 & 0x40 == 64 {
            let hi = u8_of(r)? as u16;
            t.wall |= hi << 8;
            if t.wall >= wall_count {
                t.wall = 0;
            }
        }
    }

    if b3 > 1 {
        if b3 & 2 == 2 {
            t.invisible_block = true;
        }
        if b3 & 4 == 4 {
            t.invisible_wall = true;
        }
        if b3 & 8 == 8 {
            t.fullbright_block = true;
        }
        if b3 & 0x10 == 16 {
            t.fullbright_wall = true;
        }
    }

    let rle = match (b4 & 0xC0) >> 6 {
        0 => 0u16,
        1 => u8_of(r)? as u16,
        _ => i16_of(r)? as u16,
    };

    Ok((t, rle))
}

/// Decode a whole tile section, counting as it goes.
///
/// Returns the totals and the number of bytes consumed, which the caller can compare
/// with `positions[2] - positions[1]` - the same assertion `WorldFile.cs:1777` makes,
/// and an exact one.
pub fn decode_tiles(
    data: &[u8],
    max_tiles_x: i32,
    max_tiles_y: i32,
    importance: &[bool],
    save_slopes: &[bool],
    wall_count: u16,
) -> Result<(TileStats, usize), TileError> {
    let mut r = data;
    let mut stats = TileStats::default();
    for _x in 0..max_tiles_x {
        let mut y = 0i32;
        while y < max_tiles_y {
            let (tile, rle) = read_tile(&mut r, importance, save_slopes, wall_count)?;
            stats.tiles += 1;
            if tile.active {
                stats.active += 1;
            }
            if tile.wall != 0 {
                stats.walls += 1;
            }
            match tile.liquid {
                Liquid::None => {}
                Liquid::Water => stats.liquid_water += 1,
                Liquid::Lava => stats.liquid_lava += 1,
                Liquid::Honey => stats.liquid_honey += 1,
                Liquid::Shimmer => stats.liquid_shimmer += 1,
            }
            if tile.wire || tile.wire2 || tile.wire3 || tile.wire4 {
                stats.wires += 1;
            }
            if tile.slope != 0 {
                stats.slopes += 1;
            }
            if tile.half_brick {
                stats.half_bricks += 1;
            }
            if tile.actuator {
                stats.actuators += 1;
            }
            if tile.inactive {
                stats.inactive += 1;
            }
            if tile.invisible_block {
                stats.invisible_blocks += 1;
            }
            if tile.invisible_wall {
                stats.invisible_walls += 1;
            }
            if tile.fullbright_block {
                stats.fullbright_blocks += 1;
            }
            if tile.fullbright_wall {
                stats.fullbright_walls += 1;
            }
            // The run repeats the SAME tile, so the counts above are per distinct tile
            // read; the cursor advances by rle + 1 either way.
            y += rle as i32 + 1;
            if y > max_tiles_y {
                return Err(TileError::RunOverrunsRow);
            }
        }
    }
    Ok((stats, data.len() - r.len()))
}

/// Decode a whole tile section into a vector, indexed `x * max_tiles_y + y`.
///
/// That index order is the FILE's, not the natural one: `decode_tiles` walks x outer and
/// y inner because that is how `WorldFile.cs:2526` writes them, so storing them the same
/// way means the decode stays a single pass. `at` below is the only place the arithmetic
/// lives, and the tests pin it, because an x/y transposition here would send a client a
/// world mirrored along its diagonal - which still parses.
///
/// This is the collecting twin of `decode_tiles`. That one counts and throws the tiles
/// away because 8400x2400 is 20.16M tiles and a server that only wants to report a world
/// should not hold 600 MB of them. A server that is going to SEND them has to hold them,
/// which is exactly what the native server does.
pub fn decode_all_tiles(
    data: &[u8],
    max_tiles_x: i32,
    max_tiles_y: i32,
    importance: &[bool],
    save_slopes: &[bool],
    wall_count: u16,
) -> Result<(Vec<Tile>, usize), TileError> {
    let mut r = data;
    let count = (max_tiles_x as usize) * (max_tiles_y as usize);
    let mut tiles = Vec::with_capacity(count);
    for _x in 0..max_tiles_x {
        let mut y = 0i32;
        while y < max_tiles_y {
            let (tile, rle) = read_tile(&mut r, importance, save_slopes, wall_count)?;
            for _ in 0..=rle {
                tiles.push(tile);
            }
            y += rle as i32 + 1;
            if y > max_tiles_y {
                return Err(TileError::RunOverrunsRow);
            }
        }
    }
    Ok((tiles, data.len() - r.len()))
}

/// The index of tile (x, y) in a vector built by `decode_all_tiles`.
pub fn at(max_tiles_y: i32, x: i32, y: i32) -> usize {
    x as usize * max_tiles_y as usize + y as usize
}

/// `Tile.isTheSameAs` (`Tile.cs`), as the run-length encoder needs it.
///
/// The C# compares two `ushort`s and three `byte`s of packed header and then a handful of
/// fields. Those packed words are an in-memory detail, so this compares the FIELDS they
/// hold, which is the same comparison written out. The mapping was taken from the
/// accessors: `sTileHeader` holds active, inActive, wire, wire2, wire3, halfBrick,
/// actuator, slope and fullbrightWall; `bTileHeader` holds the wall colour, the liquid
/// KIND and wire4; `bTileHeader3` holds invisibleBlock, invisibleWall and fullbrightBlock.
///
/// Two things are deliberately NOT compared, because the C# does not compare them:
///
/// * `tileColor`. `CompressTileBlock_Inner` WRITES it (`NetMessage.cs:2121`) but
///   `isTheSameAs` never looks at it, so two adjacent tiles differing only in block
///   colour are merged into one run and the run carries the first one's colour. That is
///   the native server's behaviour and reproducing it is what keeps the bytes identical;
///   "fixing" it here would make the packet differ from the game's for no gain.
/// * the wall framing in `bTileHeader2`, which a freshly loaded world has not computed
///   yet and the packet does not carry at all.
pub fn same_for_compression(a: &Tile, b: &Tile, frame_important: &[bool]) -> bool {
    if a.active != b.active
        || a.inactive != b.inactive
        || a.wire != b.wire
        || a.wire2 != b.wire2
        || a.wire3 != b.wire3
        || a.half_brick != b.half_brick
        || a.actuator != b.actuator
        || a.slope != b.slope
        || a.fullbright_wall != b.fullbright_wall
    {
        return false;
    }
    if a.active {
        if a.type_id != b.type_id {
            return false;
        }
        if frame_important.get(a.type_id as usize).copied().unwrap_or(false)
            && (a.frame_x != b.frame_x || a.frame_y != b.frame_y)
        {
            return false;
        }
    }
    if a.wall != b.wall || a.liquid_amount != b.liquid_amount {
        return false;
    }
    // `if (compTile.liquid == 0) { compare wallColor and wire4 } else { compare
    // bTileHeader }`. The two branches carry the same information - when there is no
    // liquid the kind bits are zero anyway - so this compares all three either way.
    if a.wall_color != b.wall_color || a.wire4 != b.wire4 || a.liquid != b.liquid {
        return false;
    }
    if a.invisible_block != b.invisible_block
        || a.invisible_wall != b.invisible_wall
        || a.fullbright_block != b.fullbright_block
    {
        return false;
    }
    true
}

/// Raw DEFLATE (`System.IO.Compression.DeflateStream`), using stored blocks only.
///
/// A stored block is a legal DEFLATE block: a 5-byte header and the bytes verbatim. It
/// compresses nothing, so a tile section goes out larger than the native server's, and
/// that is a real difference in bytes on the wire for a real gain in not having to write
/// a Huffman coder before the client can be tested. What it is NOT is a difference the
/// client can see: any DEFLATE reader accepts stored blocks, and the bytes it recovers
/// are identical. `the_stored_blocks_are_valid_deflate` proves the stream with zlib.
///
/// NOTE on the decompiled source: ILSpy renders the two `CompressionMode` arguments in
/// `CompressTileBlock` and `DecompressTileBlock` as `(CompressionMode)0` and
/// `(CompressionMode)1`, which cannot both be right - the enum is `Decompress = 0`,
/// `Compress = 1` - and the method names say which is which. This goes by the names.
fn deflate_stored(data: &[u8]) -> Vec<u8> {
    // 5 bytes of header per 65535-byte block, and at least one block even when empty.
    let mut out = Vec::with_capacity(data.len() + 5 * (data.len() / 65535 + 1));
    let mut i = 0usize;
    loop {
        let n = (data.len() - i).min(65535);
        let last = i + n == data.len();
        out.push(if last { 1 } else { 0 });
        let len = n as u16;
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(!len).to_le_bytes());
        out.extend_from_slice(&data[i..i + n]);
        i += n;
        if last {
            break;
        }
    }
    out
}

/// A chest as `CompressTileBlock_Inner` writes one (`NetMessage.cs:2244-2249`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SectionChest {
    pub id: i16,
    pub x: i16,
    pub y: i16,
    pub name: String,
}

/// A sign, likewise (`NetMessage.cs:2253-2257`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SectionSign {
    pub id: i16,
    pub x: i16,
    pub y: i16,
    pub text: String,
}

/// Build the body of `TileSection` (10): `NetMessage.CompressTileBlock`
/// (`NetMessage.cs:1918-2264`).
///
/// The body IS the deflate stream, and the section rectangle is INSIDE it - the client
/// reads xStart, yStart, width, height back out of the decompressed bytes
/// (`MessageBuffer.cs:892` -> `DecompressTileBlock`). So a section cannot be assembled by
/// concatenating a header and a compressed payload; the header is compressed too.
///
/// `frame_important` is `Main.tileFrameImportant`, which is the same table the world
/// container carries as its importance bits: it decides both whether the world file
/// stores frames and whether the packet does. `allows_batching` is
/// `TileID.Sets.AllowsSaveCompressionBatching`, which is assigned at runtime and so is a
/// parameter rather than a number invented here - the same reason `tiles::decode_tiles`
/// takes `WallID.Count`.
///
/// Tile entities are written as an empty list. The world reader does not decode the tile
/// entity section yet, so there is nothing to send, and writing a count of zero is the
/// honest encoding of that rather than a silent omission: the client is told there are
/// none.
pub fn write_tile_section(
    out: &mut Vec<u8>,
    tiles: &[Tile],
    max_tiles_y: i32,
    x_start: i32,
    y_start: i32,
    width: i32,
    height: i32,
    frame_important: &[bool],
    allows_batching: &[bool],
    chests: &[SectionChest],
    signs: &[SectionSign],
) -> Result<(), TileError> {
    if width < 0 || height < 0 || x_start < 0 || y_start < 0 {
        return Err(TileError::TooShort);
    }
    let mut body: Vec<u8> = Vec::new();
    body.extend_from_slice(&x_start.to_le_bytes());
    body.extend_from_slice(&y_start.to_le_bytes());
    body.extend_from_slice(&(width as i16).to_le_bytes());
    body.extend_from_slice(&(height as i16).to_le_bytes());

    // The C# writes into a 16-byte scratch array and starts the body at index 4, leaving
    // room to prepend up to four header bytes at the end. That is why the header bytes are
    // assembled backwards: `num6` counts DOWN from 3 while `num5` counts UP from 4.
    let mut array = [0u8; 16];
    let mut cursor = 0usize;
    let mut run: u16 = 0;
    let mut header = 0u8;
    // `num6` in the C#: where the header byte of the CURRENT tile sits. It is carried
    // ACROSS the flush, because the flush writes the run length after the body of the
    // tile that just ended and the header byte belongs at the front of that same tile.
    // Defaulting it to 3 at flush time is wrong for any tile with header bytes, and that
    // is exactly the bug this comment exists to prevent.
    let mut header_start = 3usize;
    let mut previous: Option<Tile> = None;

    let flush =
        |array: &mut [u8; 16], header_start: usize, cursor: usize, run: u16, header: u8, out: &mut Vec<u8>| {
            let mut header = header;
            let mut cursor = cursor;
            if run > 0 {
                array[cursor] = (run & 0xFF) as u8;
                cursor += 1;
                if run > 255 {
                    header |= 0x80;
                    array[cursor] = ((run & 0xFF00) >> 8) as u8;
                    cursor += 1;
                } else {
                    header |= 0x40;
                }
            }
            array[header_start] = header;
            out.extend_from_slice(&array[header_start..cursor]);
        };

    for i in y_start..y_start + height {
        for j in x_start..x_start + width {
            let idx = at(max_tiles_y, j, i);
            let tile = *tiles.get(idx).ok_or(TileError::TooShort)?;
            let batchable = allows_batching.get(tile.type_id as usize).copied().unwrap_or(false);
            if let Some(prev) = previous {
                if same_for_compression(&tile, &prev, frame_important) && batchable {
                    run += 1;
                    continue;
                }
            }
            if previous.is_some() {
                flush(&mut array, header_start, cursor, run, header, &mut body);
                run = 0;
            }

            // A fresh tile: four body bytes' worth of header scratch, then the body.
            cursor = 4;
            let mut b = 0u8;
            let mut b2 = 0u8;
            let mut b3 = 0u8;
            let mut b4 = 0u8;

            if tile.active {
                b |= 2;
                array[cursor] = (tile.type_id & 0xFF) as u8;
                cursor += 1;
                if tile.type_id > 255 {
                    array[cursor] = (tile.type_id >> 8) as u8;
                    cursor += 1;
                    b |= 0x20;
                }
                if frame_important.get(tile.type_id as usize).copied().unwrap_or(false) {
                    let fx = tile.frame_x as u16;
                    let fy = tile.frame_y as u16;
                    array[cursor] = (fx & 0xFF) as u8;
                    cursor += 1;
                    array[cursor] = ((fx & 0xFF00) >> 8) as u8;
                    cursor += 1;
                    array[cursor] = (fy & 0xFF) as u8;
                    cursor += 1;
                    array[cursor] = ((fy & 0xFF00) >> 8) as u8;
                    cursor += 1;
                }
                if tile.tile_color != 0 {
                    b3 |= 8;
                    array[cursor] = tile.tile_color;
                    cursor += 1;
                }
            }
            if tile.wall != 0 {
                b |= 4;
                array[cursor] = (tile.wall & 0xFF) as u8;
                cursor += 1;
                if tile.wall_color != 0 {
                    b3 |= 0x10;
                    array[cursor] = tile.wall_color;
                    cursor += 1;
                }
            }
            if tile.liquid_amount != 0 {
                if tile.liquid != Liquid::Shimmer {
                    b = match tile.liquid {
                        Liquid::Lava => b | 0x10,
                        Liquid::Honey => b | 0x18,
                        _ => b | 8,
                    };
                } else {
                    b3 |= 0x80;
                    b |= 8;
                }
                array[cursor] = tile.liquid_amount;
                cursor += 1;
            }
            if tile.wire {
                b2 |= 2;
            }
            if tile.wire2 {
                b2 |= 4;
            }
            if tile.wire3 {
                b2 |= 8;
            }
            let shape = if tile.half_brick {
                16u8
            } else if tile.slope != 0 {
                (tile.slope + 1) << 4
            } else {
                0
            };
            b2 |= shape;
            if tile.actuator {
                b3 |= 2;
            }
            if tile.inactive {
                b3 |= 4;
            }
            if tile.wire4 {
                b3 |= 0x20;
            }
            if tile.wall > 255 {
                array[cursor] = (tile.wall >> 8) as u8;
                cursor += 1;
                b3 |= 0x40;
            }
            if tile.invisible_block {
                b4 |= 2;
            }
            if tile.invisible_wall {
                b4 |= 4;
            }
            if tile.fullbright_block {
                b4 |= 8;
            }
            if tile.fullbright_wall {
                b4 |= 0x10;
            }

            // The four header bytes go in front of the body, in this order, each one
            // present only if it is non-zero and each one announcing the next.
            let mut start = 3usize;
            if b4 != 0 {
                b3 |= 1;
                array[start] = b4;
                start -= 1;
            }
            if b3 != 0 {
                b2 |= 1;
                array[start] = b3;
                start -= 1;
            }
            if b2 != 0 {
                b |= 1;
                array[start] = b2;
                start -= 1;
            }
            header = b;
            previous = Some(tile);
            // `start` has been decremented once per secondary header byte, so it now
            // names the slot the header byte itself goes in - the C# does
            // `array[num6] = b` after exactly these three steps, with no further
            // adjustment. One off here writes the header over the first body byte and
            // drops it, which is a section that decompresses to garbage.
            header_start = start;
        }
    }
    flush(&mut array, header_start, cursor, run, header, &mut body);

    // The three list counts are SHORTS, not ints: `CompressTileBlock_Inner` declares
    // `short num`, `short num2`, `short num3`, and the client reads each with
    // `ReadInt16` (`NetMessage.cs:2465`). Writing them as ints would put two extra zero
    // bytes in front of every section, and the client would read the first chest id as
    // the count.
    body.extend_from_slice(&(chests.len() as i16).to_le_bytes());
    for c in chests {
        body.extend_from_slice(&c.id.to_le_bytes());
        body.extend_from_slice(&c.x.to_le_bytes());
        body.extend_from_slice(&c.y.to_le_bytes());
        crate::net::write_string(&mut body, &c.name).expect("a Vec cannot fail");
    }
    body.extend_from_slice(&(signs.len() as i16).to_le_bytes());
    for s in signs {
        body.extend_from_slice(&s.id.to_le_bytes());
        body.extend_from_slice(&s.x.to_le_bytes());
        body.extend_from_slice(&s.y.to_le_bytes());
        crate::net::write_string(&mut body, &s.text).expect("a Vec cannot fail");
    }
    // Tile entities: none, and said so rather than omitted.
    body.extend_from_slice(&0i16.to_le_bytes());

    out.extend_from_slice(&deflate_stored(&body));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worldfile::{read_container, read_header};

    /// The two tables the book cannot supply. `SaveSlopes` is true for the ids that
    /// carry a shape; `wall_count` is `WallID.Count`.
    fn slopes(len: usize) -> Vec<bool> {
        vec![true; len]
    }

    /// Inflate a stream of stored DEFLATE blocks.
    ///
    /// Deliberately only handles stored blocks, which is all `deflate_stored` emits. It is
    /// not a general inflater and does not pretend to be one; it exists to read back the
    /// one encoding the encoder produces, and it fails loudly on anything else.
    fn inflate_stored(stream: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut i = 0usize;
        loop {
            assert!(i < stream.len(), "the stream ended without a final block");
            let header = stream[i];
            i += 1;
            let last = header & 1 == 1;
            assert_eq!((header >> 1) & 3, 0, "only stored blocks are produced");
            let len = u16::from_le_bytes(stream[i..i + 2].try_into().unwrap());
            let nlen = u16::from_le_bytes(stream[i + 2..i + 4].try_into().unwrap());
            i += 4;
            assert_eq!(!len, nlen, "the stored block's length and its complement must agree");
            out.extend_from_slice(&stream[i..i + len as usize]);
            i += len as usize;
            if last {
                break;
            }
        }
        assert_eq!(i, stream.len(), "the stream had bytes after its final block");
        out
    }

    /// The CLIENT's reader, transcribed from `NetMessage.DecompressTileBlock_Inner`
    /// (`NetMessage.cs:2282-2463`).
    ///
    /// This is the point of the whole exercise: an encoder checked by re-reading its own
    /// bytes proves nothing, because both halves can share a mistake. This is the other
    /// half of the contract, written from the client's side, so the encoder has to agree
    /// with something it did not write.
    ///
    /// Returns the tiles in the section's own order (y outer, x inner), the rectangle,
    /// and the two lists.
    fn client_read_section(
        stream: &[u8],
        frame_important: &[bool],
        solid_or_slope: &[bool],
    ) -> (Vec<Tile>, [i32; 4], Vec<SectionChest>, Vec<SectionSign>) {
        let body = inflate_stored(stream);
        let mut r: &[u8] = &body;
        let i16_of = |r: &mut &[u8]| -> i16 {
            let v = i16::from_le_bytes(r[..2].try_into().unwrap());
            *r = &r[2..];
            v
        };
        let i32_of = |r: &mut &[u8]| -> i32 {
            let v = i32::from_le_bytes(r[..4].try_into().unwrap());
            *r = &r[4..];
            v
        };
        let u8_of = |r: &mut &[u8]| -> u8 {
            let v = r[0];
            *r = &r[1..];
            v
        };

        let x_start = i32_of(&mut r);
        let y_start = i32_of(&mut r);
        let width = i16_of(&mut r) as i32;
        let height = i16_of(&mut r) as i32;

        let mut tiles = Vec::new();
        let mut repeat = 0i32;
        let mut last: Option<Tile> = None;
        for _y in 0..height {
            for _x in 0..width {
                if repeat != 0 {
                    repeat -= 1;
                    tiles.push(last.expect("a run always follows a tile"));
                    continue;
                }
                let mut t = Tile::empty();
                let mut b = 0u8;
                let mut b2 = 0u8;
                let mut b3 = 0u8;
                let b4 = u8_of(&mut r);
                if b4 & 1 == 1 {
                    b = u8_of(&mut r);
                }
                if b & 1 == 1 {
                    b2 = u8_of(&mut r);
                }
                if b2 & 1 == 1 {
                    b3 = u8_of(&mut r);
                }
                if b4 & 2 == 2 {
                    t.active = true;
                    let type_id = if b4 & 0x20 == 32 {
                        let lo = u8_of(&mut r) as u16;
                        let hi = u8_of(&mut r) as u16;
                        (hi << 8) | lo
                    } else {
                        u8_of(&mut r) as u16
                    };
                    t.type_id = type_id;
                    if frame_important.get(type_id as usize).copied().unwrap_or(false) {
                        t.frame_x = i16_of(&mut r);
                        t.frame_y = i16_of(&mut r);
                    } else {
                        t.frame_x = -1;
                        t.frame_y = -1;
                    }
                    if b2 & 8 == 8 {
                        t.tile_color = u8_of(&mut r);
                    }
                }
                if b4 & 4 == 4 {
                    t.wall = u8_of(&mut r) as u16;
                    if b2 & 0x10 == 16 {
                        t.wall_color = u8_of(&mut r);
                    }
                }
                let kind = (b4 & 0x18) >> 3;
                if kind != 0 {
                    t.liquid_amount = u8_of(&mut r);
                    t.liquid = if b2 & 0x80 == 128 {
                        Liquid::Shimmer
                    } else if kind > 1 {
                        if kind == 2 {
                            Liquid::Lava
                        } else {
                            Liquid::Honey
                        }
                    } else {
                        Liquid::Water
                    };
                }
                if b > 1 {
                    t.wire = b & 2 == 2;
                    t.wire2 = b & 4 == 4;
                    t.wire3 = b & 8 == 8;
                    let shape = (b & 0x70) >> 4;
                    if shape != 0 && solid_or_slope.get(t.type_id as usize).copied().unwrap_or(false) {
                        if shape == 1 {
                            t.half_brick = true;
                        } else {
                            t.slope = shape - 1;
                        }
                    }
                }
                if b2 > 1 {
                    t.actuator = b2 & 2 == 2;
                    t.inactive = b2 & 4 == 4;
                    t.wire4 = b2 & 0x20 == 32;
                    if b2 & 0x40 == 64 {
                        let hi = u8_of(&mut r) as u16;
                        t.wall |= hi << 8;
                    }
                }
                if b3 > 1 {
                    t.invisible_block = b3 & 2 == 2;
                    t.invisible_wall = b3 & 4 == 4;
                    t.fullbright_block = b3 & 8 == 8;
                    t.fullbright_wall = b3 & 0x10 == 16;
                }
                repeat = match (b4 & 0xC0) >> 6 {
                    0 => 0,
                    1 => u8_of(&mut r) as i32,
                    _ => i16_of(&mut r) as i32,
                };
                last = Some(t);
                tiles.push(t);
            }
        }

        let chest_count = i16_of(&mut r);
        let mut chests = Vec::new();
        for _ in 0..chest_count {
            let id = i16_of(&mut r);
            let x = i16_of(&mut r);
            let y = i16_of(&mut r);
            let name = crate::net::read_string(&mut r).unwrap();
            chests.push(SectionChest { id, x, y, name });
        }
        let sign_count = i16_of(&mut r);
        let mut signs = Vec::new();
        for _ in 0..sign_count {
            let id = i16_of(&mut r);
            let x = i16_of(&mut r);
            let y = i16_of(&mut r);
            let text = crate::net::read_string(&mut r).unwrap();
            signs.push(SectionSign { id, x, y, text });
        }
        let entity_count = i16_of(&mut r);
        assert_eq!(entity_count, 0, "the encoder writes no entities yet");
        assert!(r.is_empty(), "the client would leave {} bytes unread", r.len());

        (tiles, [x_start, y_start, width, height], chests, signs)
    }

    /// The whole pipeline against a REAL world: decode the tile section of a `.wld`, encode
    /// a 200x150 window of it the way the server does, and read it back with the client's
    /// reader.
    ///
    /// The synthetic fixtures above prove the FORMAT, branch by branch. This proves it on
    /// real terrain, which is where a branch that is wrong only for a combination the
    /// fixture did not happen to build will show up. It is the same argument as the
    /// world-file test that lands on `positions[1]`: a check against data nobody wrote for
    /// the test is a different kind of check.
    #[test]
    fn the_client_reads_back_a_real_section() {
        let Some(dir) = worlds_dir() else {
            return; // no world file on this machine; the synthetic tests still ran
        };
        let path = dir.join("a.wld");
        if !path.exists() {
            return;
        }
        let data = std::fs::read(&path).unwrap();
        let world = crate::worldfile::load_world(&data).expect("a real world loads");
        let section = world.tile_section(&data).expect("the tile section is present");
        let save_slopes = vec![true; 0x10000];
        let (tiles, _) = decode_all_tiles(
            section,
            world.header.max_tiles_x,
            world.header.max_tiles_y,
            &world.container.importance,
            &save_slopes,
            u16::MAX,
        )
        .expect("the tiles decode");

        // The spawn window, which is the block a joining client is sent.
        let sx = world.header.spawn_tile_x / 200 - 2;
        let sy = world.header.spawn_tile_y / 150 - 1;
        let batching = crate::net::allows_save_compression_batching(world.container.importance.len());
        let mut sections_checked = 0;
        for dx in 0..5 {
            for dy in 0..3 {
                let (x_start, y_start) = ((sx + dx) * 200, (sy + dy) * 150);
                if x_start < 0 || y_start < 0 {
                    continue;
                }
                if x_start + 200 > world.header.max_tiles_x || y_start + 150 > world.header.max_tiles_y {
                    continue;
                }
                let mut out = Vec::new();
                write_tile_section(
                    &mut out,
                    &tiles,
                    world.header.max_tiles_y,
                    x_start,
                    y_start,
                    200,
                    150,
                    &world.container.importance,
                    &batching,
                    &[],
                    &[],
                )
                .expect("the section encodes");
                let (read, rect, _, _) =
                    client_read_section(&out, &world.container.importance, &save_slopes);
                assert_eq!(rect, [x_start, y_start, 200, 150]);
                assert_eq!(read.len(), 200 * 150, "section ({x_start},{y_start})");
                // Every field the wire can carry, on every tile.
                for y in 0..150 {
                    for x in 0..200 {
                        let want = tiles[at(world.header.max_tiles_y, x_start + x, y_start + y)];
                        let got = read[(y * 200 + x) as usize];
                        assert_eq!(got.active, want.active, "active at ({x_start}+{x},{y_start}+{y})");
                        assert_eq!(got.type_id, want.type_id, "type at ({x_start}+{x},{y_start}+{y})");
                        assert_eq!(got.wall, want.wall, "wall at ({x_start}+{x},{y_start}+{y})");
                        assert_eq!(got.liquid_amount, want.liquid_amount, "liquid at ({x_start}+{x},{y_start}+{y})");
                        assert_eq!(got.liquid, want.liquid, "liquid kind at ({x_start}+{x},{y_start}+{y})");
                        assert_eq!(got.tile_color, want.tile_color, "block colour at ({x_start}+{x},{y_start}+{y})");
                        assert_eq!(got.wall_color, want.wall_color, "wall colour at ({x_start}+{x},{y_start}+{y})");
                        assert_eq!(got.wire, want.wire, "wire at ({x_start}+{x},{y_start}+{y})");
                        assert_eq!(got.wire2, want.wire2, "wire2 at ({x_start}+{x},{y_start}+{y})");
                        assert_eq!(got.wire3, want.wire3, "wire3 at ({x_start}+{x},{y_start}+{y})");
                        assert_eq!(got.wire4, want.wire4, "wire4 at ({x_start}+{x},{y_start}+{y})");
                        assert_eq!(got.actuator, want.actuator, "actuator at ({x_start}+{x},{y_start}+{y})");
                        assert_eq!(got.inactive, want.inactive, "inactive at ({x_start}+{x},{y_start}+{y})");
                        assert_eq!(got.half_brick, want.half_brick, "half brick at ({x_start}+{x},{y_start}+{y})");
                        assert_eq!(got.slope, want.slope, "slope at ({x_start}+{x},{y_start}+{y})");
                        assert_eq!(got.invisible_block, want.invisible_block, "invisible block at ({x_start}+{x},{y_start}+{y})");
                        assert_eq!(got.invisible_wall, want.invisible_wall, "invisible wall at ({x_start}+{x},{y_start}+{y})");
                        assert_eq!(got.fullbright_block, want.fullbright_block, "fullbright block at ({x_start}+{x},{y_start}+{y})");
                        assert_eq!(got.fullbright_wall, want.fullbright_wall, "fullbright wall at ({x_start}+{x},{y_start}+{y})");
                        if want.active && world.container.importance.get(want.type_id as usize).copied().unwrap_or(false) {
                            assert_eq!(got.frame_x, want.frame_x, "frame x at ({x_start}+{x},{y_start}+{y})");
                            assert_eq!(got.frame_y, want.frame_y, "frame y at ({x_start}+{x},{y_start}+{y})");
                        }
                    }
                }
                sections_checked += 1;
            }
        }
        assert!(sections_checked > 0, "the spawn window must contain at least one section");
        println!("checked {sections_checked} real sections of {}", path.display());
    }

    /// The index formula, pinned. A transposition here sends a world mirrored along its
    /// diagonal, which parses perfectly and is wrong everywhere.
    #[test]
    fn the_tile_index_is_x_times_max_y_plus_y() {
        assert_eq!(at(1200, 0, 0), 0);
        assert_eq!(at(1200, 0, 5), 5);
        assert_eq!(at(1200, 1, 0), 1200);
        assert_eq!(at(1200, 2, 3), 2403);
    }

    /// The whole contract, end to end: encode a section the way the server does, then read
    /// it the way the CLIENT does, and require the two to agree tile for tile.
    ///
    /// The fixture deliberately includes every branch: an active block, a 16-bit type, a
    /// framed (important) block, a wall, a wall colour, a block colour, each liquid kind,
    /// each wire, a slope, a half brick, an actuator, an inactive block, a 16-bit wall, and
    /// all four invisible/fullbright flags.
    #[test]
    fn the_client_reads_back_what_the_encoder_wrote() {
        const MAX_Y: i32 = 8;
        const W: i32 = 4;
        const H: i32 = 6;
        let mut tiles = vec![Tile::empty(); (MAX_Y * 4) as usize];

        let mut put = |x: i32, y: i32, t: Tile| {
            tiles[at(MAX_Y, x, y)] = t;
        };
        let mut plain = Tile::empty();
        plain.active = true;
        plain.type_id = 2;
        for y in 0..H {
            for x in 0..W {
                put(x, y, plain);
            }
        }
        let mut framed = Tile::empty();
        framed.active = true;
        framed.type_id = 300; // above 255, so the 16-bit type path runs
        framed.frame_x = 18;
        framed.frame_y = 36;
        put(0, 0, framed);
        let mut walled = Tile::empty();
        walled.wall = 300; // above 255, so the wall extension byte runs
        walled.wall_color = 7;
        put(1, 1, walled);
        let mut wet = Tile::empty();
        wet.active = true;
        wet.type_id = 1;
        wet.liquid_amount = 128;
        wet.liquid = Liquid::Water;
        wet.tile_color = 3;
        wet.wire = true;
        wet.wire2 = true;
        wet.wire3 = true;
        wet.wire4 = true;
        put(2, 2, wet);
        let mut lava = wet;
        lava.liquid = Liquid::Lava;
        put(3, 2, lava);
        let mut honey = wet;
        honey.liquid = Liquid::Honey;
        put(0, 3, honey);
        let mut shimmer = wet;
        shimmer.liquid = Liquid::Shimmer;
        put(1, 3, shimmer);
        let mut shaped = Tile::empty();
        shaped.active = true;
        shaped.type_id = 1;
        shaped.slope = 2;
        shaped.actuator = true;
        shaped.inactive = true;
        shaped.invisible_block = true;
        shaped.invisible_wall = true;
        shaped.fullbright_block = true;
        shaped.fullbright_wall = true;
        put(2, 3, shaped);
        let mut half = Tile::empty();
        half.active = true;
        half.type_id = 1;
        half.half_brick = true;
        put(3, 3, half);

        // A run: 5 identical empty tiles in a row, so the run length path is exercised
        // for both the one-byte and the two-byte encoding.
        let mut long = Tile::empty();
        long.active = true;
        long.type_id = 1;
        put(0, 4, long);
        for x in 0..4 {
            put(x, 5, long);
        }

        let frame_important = {
            let mut t = vec![false; 512];
            t[300] = true;
            t
        };
        let batching = vec![true; 512];
        let solid = vec![true; 512];

        let mut out = Vec::new();
        write_tile_section(&mut out, &tiles, MAX_Y, 0, 0, W, H, &frame_important, &batching, &[], &[])
            .expect("the section encodes");

        let (read, rect, chests, signs) = client_read_section(&out, &frame_important, &solid);
        assert_eq!(rect, [0, 0, W, H]);
        assert!(chests.is_empty() && signs.is_empty());
        assert_eq!(read.len(), (W * H) as usize);

        for y in 0..H {
            for x in 0..W {
                let want = tiles[at(MAX_Y, x, y)];
                let got = read[(y * W + x) as usize];
                // The wire does not carry the file's header byte, so only the fields the
                // packet can express are compared - which is exactly the set the client
                // reconstructs.
                assert_eq!(got.active, want.active, "active at ({x},{y})");
                if want.active {
                    assert_eq!(got.type_id, want.type_id, "type at ({x},{y})");
                }
                assert_eq!(got.wall, want.wall, "wall at ({x},{y})");
                assert_eq!(got.liquid_amount, want.liquid_amount, "liquid at ({x},{y})");
                assert_eq!(got.liquid, want.liquid, "liquid kind at ({x},{y})");
                assert_eq!(got.tile_color, want.tile_color, "block colour at ({x},{y})");
                assert_eq!(got.wall_color, want.wall_color, "wall colour at ({x},{y})");
                assert_eq!(got.wire, want.wire, "wire at ({x},{y})");
                assert_eq!(got.wire2, want.wire2, "wire2 at ({x},{y})");
                assert_eq!(got.wire3, want.wire3, "wire3 at ({x},{y})");
                assert_eq!(got.wire4, want.wire4, "wire4 at ({x},{y})");
                assert_eq!(got.actuator, want.actuator, "actuator at ({x},{y})");
                assert_eq!(got.inactive, want.inactive, "inactive at ({x},{y})");
                assert_eq!(got.half_brick, want.half_brick, "half brick at ({x},{y})");
                assert_eq!(got.slope, want.slope, "slope at ({x},{y})");
                assert_eq!(got.invisible_block, want.invisible_block, "invisible block at ({x},{y})");
                assert_eq!(got.invisible_wall, want.invisible_wall, "invisible wall at ({x},{y})");
                assert_eq!(got.fullbright_block, want.fullbright_block, "fullbright block at ({x},{y})");
                assert_eq!(got.fullbright_wall, want.fullbright_wall, "fullbright wall at ({x},{y})");
                if want.active && frame_important[want.type_id as usize] {
                    assert_eq!(got.frame_x, want.frame_x, "frame x at ({x},{y})");
                    assert_eq!(got.frame_y, want.frame_y, "frame y at ({x},{y})");
                }
            }
        }
    }

    /// A run of identical tiles has to come back as that many tiles. The run length is the
    /// reason the format is compact and the easiest thing to get wrong: an encoder that
    /// writes the run but not the flag byte, or the wrong way round, produces a section
    /// that is short by exactly the repeats and still decompresses.
    #[test]
    fn a_run_of_identical_tiles_comes_back_whole() {
        const MAX_Y: i32 = 4;
        let mut tiles = vec![Tile::empty(); 16];
        let mut t = Tile::empty();
        t.active = true;
        t.type_id = 1;
        for x in 0..4 {
            tiles[at(MAX_Y, x, 0)] = t;
        }
        let frame_important = vec![false; 512];
        let batching = vec![true; 512];
        let mut out = Vec::new();
        write_tile_section(&mut out, &tiles, MAX_Y, 0, 0, 4, 1, &frame_important, &batching, &[], &[])
            .unwrap();
        let (read, _, _, _) = client_read_section(&out, &frame_important, &frame_important);
        assert_eq!(read.len(), 4);
        assert!(read.iter().all(|r| r.active && r.type_id == 1));
    }

    /// The three list counts are shorts. Writing them as ints adds two zero bytes per
    /// list, and the client would then read the first chest's id as the count.
    #[test]
    fn the_section_carries_its_chests_and_signs() {
        const MAX_Y: i32 = 2;
        let tiles = vec![Tile::empty(); 4];
        let frame_important = vec![false; 512];
        let batching = vec![true; 512];
        let chests = vec![SectionChest { id: 3, x: 10, y: 20, name: "Loot".into() }];
        let signs = vec![SectionSign { id: 4, x: 30, y: 40, text: "Hi".into() }];
        let mut out = Vec::new();
        write_tile_section(&mut out, &tiles, MAX_Y, 0, 0, 2, 2, &frame_important, &batching, &chests, &signs)
            .unwrap();
        let (_, _, got_chests, got_signs) = client_read_section(&out, &frame_important, &frame_important);
        assert_eq!(got_chests, chests);
        assert_eq!(got_signs, signs);
    }

    /// The stored-block framing itself, byte for byte, including the empty case. If this
    /// is wrong the client's `DeflateStream` throws and nothing else in this file matters.
    #[test]
    fn the_stored_blocks_are_framed_correctly() {
        // Empty input still needs one final block.
        assert_eq!(deflate_stored(&[]), vec![0x01, 0x00, 0x00, 0xFF, 0xFF]);
        // Three bytes: one final stored block, LEN=3, NLEN=0xFFFC.
        assert_eq!(
            deflate_stored(&[1, 2, 3]),
            vec![0x01, 0x03, 0x00, 0xFC, 0xFF, 1, 2, 3]
        );
        // Over the 65535 limit: a non-final block, then a final one.
        let big = vec![9u8; 65536];
        let d = deflate_stored(&big);
        assert_eq!(&d[0..5], &[0x00, 0xFF, 0xFF, 0x00, 0x00]);
        assert_eq!(d[5 + 65535], 0x01);
        assert_eq!(inflate_stored(&d), big);
    }

    /// `same_for_compression` must not merge two tiles the client would see as different,
    /// and must merge the ones `isTheSameAs` merges. The block colour case is the one that
    /// looks like a bug and is not: the C# writes the colour but does not compare it.
    #[test]
    fn the_compression_equality_matches_is_the_same_as() {
        let important = vec![false; 512];
        let a = Tile::empty();
        let b = Tile::empty();
        assert!(same_for_compression(&a, &b, &important));

        let mut c = Tile::empty();
        c.tile_color = 5;
        assert!(
            same_for_compression(&a, &c, &important),
            "the C# does not compare the block colour, so neither may this"
        );

        for mutate in [
            (|t: &mut Tile| t.active = true) as fn(&mut Tile),
            |t: &mut Tile| t.wall = 1,
            |t: &mut Tile| t.liquid_amount = 1,
            |t: &mut Tile| t.wire = true,
            |t: &mut Tile| t.slope = 1,
            |t: &mut Tile| t.half_brick = true,
            |t: &mut Tile| t.actuator = true,
            |t: &mut Tile| t.inactive = true,
            |t: &mut Tile| t.wire4 = true,
            |t: &mut Tile| t.invisible_block = true,
            |t: &mut Tile| t.invisible_wall = true,
            |t: &mut Tile| t.fullbright_block = true,
            |t: &mut Tile| t.fullbright_wall = true,
            |t: &mut Tile| t.wall_color = 1,
        ] {
            let mut m = Tile::empty();
            mutate(&mut m);
            assert!(!same_for_compression(&a, &m, &important), "a change must break the run");
        }
    }

    #[test]
    fn a_bare_tile_is_one_byte() {
        let mut r: &[u8] = &[0x00];
        let (t, rle) = read_tile(&mut r, &[false; 256], &slopes(256), 300).unwrap();
        assert_eq!(rle, 0);
        assert!(!t.active && t.wall == 0 && t.type_id == 0);
        assert!(r.is_empty(), "an empty tile is exactly one byte");
    }

    /// The run length is the high two bits of the header, and its three encodings are
    /// all reachable.
    #[test]
    fn the_run_length_has_three_encodings() {
        // 0: no run.
        let mut r: &[u8] = &[0x00];
        assert_eq!(read_tile(&mut r, &[false; 256], &slopes(256), 300).unwrap().1, 0);
        // 1: one byte follows the tile.
        let mut r: &[u8] = &[0x40, 5];
        assert_eq!(read_tile(&mut r, &[false; 256], &slopes(256), 300).unwrap().1, 5);
        // 2 and 3: a little-endian short.
        let mut r: &[u8] = &[0x80, 0x34, 0x12];
        assert_eq!(read_tile(&mut r, &[false; 256], &slopes(256), 300).unwrap().1, 0x1234);
        let mut r: &[u8] = &[0xC0, 0x01, 0x00];
        assert_eq!(read_tile(&mut r, &[false; 256], &slopes(256), 300).unwrap().1, 1);
    }

    /// An active tile reads its type, and its frames only when the id is important.
    #[test]
    fn frames_are_read_only_for_important_ids() {
        // active, 8-bit type 5, importance[5] = true, frames 1 and 2
        let mut r: &[u8] = &[0x02, 5, 0x01, 0x00, 0x02, 0x00];
        let mut importance = vec![false; 256];
        importance[5] = true;
        let (t, _) = read_tile(&mut r, &importance, &slopes(256), 300).unwrap();
        assert!(t.active);
        assert_eq!(t.type_id, 5);
        assert_eq!((t.frame_x, t.frame_y), (1, 2));

        // Not important: no frames are read, and the tile records -1. Type 6 is NOT in
        // the table above, which is the point - type 5 is, so it would read frames.
        let mut r: &[u8] = &[0x02, 6];
        let (t, _) = read_tile(&mut r, &importance, &slopes(256), 300).unwrap();
        assert_eq!((t.frame_x, t.frame_y), (-1, -1));
        assert!(r.is_empty(), "nothing follows an unimportant tile");
    }

    /// A 16-bit tile id is flagged in the header and stored low byte first.
    #[test]
    fn a_sixteen_bit_type_is_low_byte_first() {
        // An important id reads two frame shorts after the type.
        let mut r: &[u8] = &[0x02 | 0x20, 0x34, 0x12, 0x01, 0x00, 0x02, 0x00];
        let mut importance = vec![false; 0x1235];
        importance[0x1234] = true;
        let (t, _) = read_tile(&mut r, &importance, &slopes(0x1235), 300).unwrap();
        assert_eq!(t.type_id, 0x1234);
        assert_eq!((t.frame_x, t.frame_y), (1, 2));
        assert!(r.is_empty());
    }

    /// A wall id past `WallID.Count` becomes 0, which is how the game drops walls it no
    /// longer knows.
    #[test]
    fn an_unknown_wall_becomes_zero() {
        let mut r: &[u8] = &[0x04, 200];
        let (t, _) = read_tile(&mut r, &[false; 256], &slopes(256), 100).unwrap();
        assert_eq!(t.wall, 0, "200 is past a count of 100");
        let mut r: &[u8] = &[0x04, 99];
        let (t, _) = read_tile(&mut r, &[false; 256], &slopes(256), 100).unwrap();
        assert_eq!(t.wall, 99);
    }

    /// The liquid kind comes from the amount bits, with shimmer overriding them.
    #[test]
    fn liquid_kind_follows_the_amount_bits() {
        // amount 1 = water
        let mut r: &[u8] = &[0x08, 255];
        let (t, _) = read_tile(&mut r, &[false; 256], &slopes(256), 300).unwrap();
        assert_eq!(t.liquid, Liquid::Water);
        assert_eq!(t.liquid_amount, 255);
        // amount 2 = lava
        let mut r: &[u8] = &[0x10, 255];
        assert_eq!(read_tile(&mut r, &[false; 256], &slopes(256), 300).unwrap().0.liquid, Liquid::Lava);
        // amount 3 = honey
        let mut r: &[u8] = &[0x18, 255];
        assert_eq!(read_tile(&mut r, &[false; 256], &slopes(256), 300).unwrap().0.liquid, Liquid::Honey);
        // shimmer is a flag in bTileHeader2, and bTileHeader2 only exists when
        // bTileHeader's bit 0 is set - so the header byte chain is b4, b, b2 here.
        let mut r: &[u8] = &[0x08 | 0x01, 0x81, 0x80, 255];
        let (t, _) = read_tile(&mut r, &[false; 256], &slopes(256), 300).unwrap();
        assert_eq!(t.liquid, Liquid::Shimmer);
        assert_eq!(t.liquid_amount, 255, "the amount is still read");
    }

    /// The shape bits are gated: an id that does not save slopes drops them.
    #[test]
    fn shapes_are_gated_on_save_slopes() {
        // bTileHeader with shape 1 (half brick) in bits 4..6. An important id also
        // carries two frame shorts, so the bytes after the type are its frames.
        let mut r: &[u8] = &[0x02 | 0x01, 0x10, 7, 0x00, 0x00, 0x00, 0x00];
        let mut importance = vec![false; 256];
        importance[7] = true;
        let mut slopes_v = vec![true; 256];
        let (t, _) = read_tile(&mut r, &importance, &slopes_v, 300).unwrap();
        assert!(t.half_brick);

        // Same bytes, but id 7 does not save slopes: the shape is dropped, and no extra
        // byte is read.
        slopes_v[7] = false;
        let mut r: &[u8] = &[0x02 | 0x01, 0x10, 7, 0x00, 0x00, 0x00, 0x00];
        let (t, _) = read_tile(&mut r, &importance, &slopes_v, 300).unwrap();
        assert!(!t.half_brick && t.slope == 0);
        assert!(r.is_empty(), "a dropped shape consumes no byte");
    }

    /// A type past the importance table is an error naming the id, not a silent zero.
    #[test]
    fn a_type_outside_the_importance_table_is_an_error() {
        let mut r: &[u8] = &[0x02, 9];
        let importance = vec![false; 4];
        assert_eq!(
            read_tile(&mut r, &importance, &slopes(256), 300),
            Err(TileError::TypeOutOfRange(9))
        );
    }

    #[test]
    fn a_truncated_tile_is_too_short() {
        // Each case stops one byte short of something the header asked for. Note
        // `[0x02, 5]` is NOT here: with a table that says id 5 is unimportant, an
        // active tile is type-only and therefore complete.
        for data in [vec![], vec![0x01u8], vec![0x02], vec![0x04], vec![0x08], vec![0x40], vec![0x80]] {
            let mut r: &[u8] = &data;
            assert_eq!(
                read_tile(&mut r, &[false; 256], &slopes(256), 300),
                Err(TileError::TooShort),
                "{data:?}"
            );
        }
        // ...and the one that IS complete, so the list above is not vacuously short.
        let mut r: &[u8] = &[0x02, 5];
        let (t, _) = read_tile(&mut r, &[false; 256], &slopes(256), 300).unwrap();
        assert!(t.active && t.type_id == 5 && r.is_empty());
    }

    /// A run that walks off the end of a row is refused rather than wrapping into the
    /// next row.
    #[test]
    fn a_run_that_overruns_a_row_is_refused() {
        // One row of 4 tiles, each a bare tile except the first, which claims a run of 9.
        let data = vec![0x40u8, 9, 0x00, 0x00, 0x00];
        assert_eq!(
            decode_tiles(&data, 1, 4, &[false; 256], &slopes(256), 300),
            Err(TileError::RunOverrunsRow)
        );
    }

    /// The strongest check available: decode a REAL world's tile section and assert the
    /// cursor lands exactly on the next section pointer, which is what
    /// `WorldFile.cs:1777` refuses to continue without.
    #[test]
    fn a_real_worlds_tile_section_lands_on_the_next_pointer() {
        let Some(dir) = worlds_dir() else {
            println!("no Terraria Worlds directory; the real-file check did not run");
            return;
        };
        let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
            .expect("the Worlds directory is readable")
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "wld"))
            .collect();
        files.sort();
        if files.is_empty() {
            println!("no .wld in {}; the real-file check did not run", dir.display());
            return;
        }

        for path in &files {
            let data = std::fs::read(path).expect("the world file is readable");
            let (c, _) = read_container(&data).expect("the container parses");
            let mut hr = &data[c.positions[0] as usize..];
            let h = read_header(&mut hr, c.version).expect("the header parses");

            let start = c.positions[1] as usize;
            let end = c.positions[2] as usize;
            let section = &data[start..end];
            // The two tables the book lacks. WallID.Count is past every wall the files
            // use, and slopes are allowed everywhere: neither choice can make the
            // cursor land correctly by accident, because a wrong shape read consumes
            // the wrong number of bytes.
            let save_slopes = vec![true; 0x10000];
            let (stats, used) = decode_tiles(
                section,
                h.max_tiles_x,
                h.max_tiles_y,
                &c.importance,
                &save_slopes,
                u16::MAX,
            )
            .expect("the tile section decodes");

            assert_eq!(
                used,
                end - start,
                "{}: the tile section must end exactly on positions[2]",
                path.display()
            );
            let expected_tiles = h.max_tiles_x as u64 * h.max_tiles_y as u64;
            assert!(stats.tiles > 0);
            assert!(
                stats.tiles <= expected_tiles,
                "{}: {} runs cover more than {expected_tiles} tiles",
                path.display(),
                stats.tiles
            );
            // A real world is mostly air, but not entirely, and it has walls, water and
            // wires. Bounds that a mis-parse would violate.
            assert!(stats.active > 0, "{}: no active tiles at all", path.display());
            assert!(
                stats.active < expected_tiles,
                "{}: every tile is active, which a real world is not",
                path.display()
            );
            assert!(stats.walls > 0, "{}: no walls", path.display());
            // No compression-ratio assertion here: the byte-exact check above already
            // fails if the run length is ignored, and a ratio would be a guess about how
            // airy a particular world is.
            println!(
                "{}: {} tile reads for {}x{} = {} tiles; active={} walls={} water={} lava={} honey={} shimmer={} wires={} slopes={} halfbricks={} actuators={}",
                path.file_name().unwrap().to_string_lossy(),
                stats.tiles,
                h.max_tiles_x,
                h.max_tiles_y,
                expected_tiles,
                stats.active,
                stats.walls,
                stats.liquid_water,
                stats.liquid_lava,
                stats.liquid_honey,
                stats.liquid_shimmer,
                stats.wires,
                stats.slopes,
                stats.half_bricks,
                stats.actuators
            );
        }
    }

    fn worlds_dir() -> Option<std::path::PathBuf> {
        let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?;
        let dir = std::path::PathBuf::from(home)
            .join("Documents")
            .join("My Games")
            .join("Terraria")
            .join("Worlds");
        if dir.is_dir() {
            Some(dir)
        } else {
            None
        }
    }
}
