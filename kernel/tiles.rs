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
    fn empty() -> Self {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worldfile::{read_container, read_header};

    /// The two tables the book cannot supply. `SaveSlopes` is true for the ids that
    /// carry a shape; `wall_count` is `WallID.Count`.
    fn slopes(len: usize) -> Vec<bool> {
        vec![true; len]
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
