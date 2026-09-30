//! worldfile: read a Terraria `.wld`.
//!
//! The first slice of the world work, and the one with no conflicts: it depends on
//! bytes and nothing else - no sockets, no player, no game state. `WorldFile` is 56
//! methods in the book; this covers the container and the world's identity, which is
//! what the world-select list and `WorldData` (7) need first.
//!
//! Container, from `WorldFile.cs:1907` `LoadFileFormatHeader`:
//!
//! ```text
//! int32   version
//! (version >= 135) uint64 magic, uint32 revision, uint64 flags
//! int16   sectionCount, then int32 position[sectionCount]
//! uint16  importanceCount, then that many BITS, LSB-first within each byte
//! ```
//!
//! The section pointers are the reason this parses at all: `positions[0]` is where the
//! world header starts, so a container parse can be checked EXACTLY rather than
//! approximately - see the test that asserts the stream lands on it.
//!
//! Header, from `WorldFile.cs:1957` `LoadHeader` and `:1998` `LoadWorldFlags`. This
//! slice stops after `rockLayer` and says so; the flags tail (banners, the downed-boss
//! table, the ore tiers, the backgrounds) and every section after the header (tiles,
//! chests, signs, NPCs, tile entities, pressure plates, town manager, bestiary,
//! creative powers) are the next slices.

use std::io::Read;

/// `FileMetadata.cs:66`: `(num & 0x00FFFFFFFFFFFFFF) == 27981915666277746`.
///
/// The low 7 bytes are the magic; the top byte is the file type.
pub const MAGIC: u64 = 27981915666277746;

/// `FileType.cs`: `None, Map, World, Player`, so World is 2.
pub const FILE_TYPE_WORLD: u8 = 2;

/// The version this reader is known to understand. `a.wld` and `d.wld` on the machine
/// this was written on both report 279.
pub const KNOWN_VERSION: i32 = 279;

/// Why a `.wld` could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorldFileError {
    /// The data ended before the structure did.
    TooShort,
    /// The 7-byte magic did not match: this is not a Re-Logic file.
    BadMagic,
    /// The magic matched but the type byte was not `World`.
    NotAWorld,
}

/// The file container: the version, the section table, and the importance bits.
#[derive(Debug, Clone, PartialEq)]
pub struct Container {
    pub version: i32,
    pub revision: u32,
    pub favorite: bool,
    /// Byte offset of each section. `positions[0]` is the world header.
    pub positions: Vec<i32>,
    /// One bit per tile id: whether the tile is "important" (carries data even when
    /// its id is 0). Packed LSB-first.
    pub importance: Vec<bool>,
}

/// The world's identity and geometry, as far as `rockLayer`.
#[derive(Debug, Clone, PartialEq)]
pub struct WorldHeader {
    pub name: String,
    pub seed: String,
    pub generator_version: u64,
    pub unique_id: [u8; 16],
    pub world_id: i32,
    pub left_world: i32,
    pub right_world: i32,
    pub top_world: i32,
    pub bottom_world: i32,
    pub max_tiles_x: i32,
    pub max_tiles_y: i32,
    pub game_mode: i32,
    /// The special-seed booleans, in the order the C# reads them (version >= 302).
    pub drunk_world: bool,
    pub get_good_world: bool,
    pub tenth_anniversary_world: bool,
    pub dont_starve_world: bool,
    pub not_the_bees_world: bool,
    pub remix_world: bool,
    pub no_traps_world: bool,
    pub zenith_world: bool,
    pub skyblock_world: bool,
    /// `DateTime.FromBinary` ticks, kept raw: the C# converts, and the conversion is
    /// `DateTime`-specific rather than a fact about the file.
    pub creation_time_binary: i64,
    pub last_played_binary: i64,
    pub moon_type: u8,
    pub tree_x: [i32; 3],
    pub tree_style: [i32; 4],
    pub cave_back_x: [i32; 3],
    pub cave_back_style: [i32; 4],
    pub ice_back_style: i32,
    pub jungle_back_style: i32,
    pub hell_back_style: i32,
    pub spawn_tile_x: i32,
    pub spawn_tile_y: i32,
    pub world_surface: f64,
    pub rock_layer: f64,
}

// -- primitive readers -------------------------------------------------------
//
// `&[u8]` implements `Read` and `read_exact` advances it, so the stream IS the slice
// and the position is `whole.len() - rest.len()`. No cursor object, no second copy of
// the 7-bit string rule: `net::read_string` is reused.

fn take<'a>(r: &mut &'a [u8], n: usize) -> Result<&'a [u8], WorldFileError> {
    if r.len() < n {
        return Err(WorldFileError::TooShort);
    }
    let (head, tail) = r.split_at(n);
    *r = tail;
    Ok(head)
}

fn u8_of(r: &mut &[u8]) -> Result<u8, WorldFileError> {
    Ok(take(r, 1)?[0])
}

fn bool_of(r: &mut &[u8]) -> Result<bool, WorldFileError> {
    Ok(u8_of(r)? != 0)
}

fn u16_of(r: &mut &[u8]) -> Result<u16, WorldFileError> {
    Ok(u16::from_le_bytes(take(r, 2)?.try_into().unwrap()))
}

fn i16_of(r: &mut &[u8]) -> Result<i16, WorldFileError> {
    Ok(i16::from_le_bytes(take(r, 2)?.try_into().unwrap()))
}

fn u32_of(r: &mut &[u8]) -> Result<u32, WorldFileError> {
    Ok(u32::from_le_bytes(take(r, 4)?.try_into().unwrap()))
}

fn i32_of(r: &mut &[u8]) -> Result<i32, WorldFileError> {
    Ok(i32::from_le_bytes(take(r, 4)?.try_into().unwrap()))
}

fn u64_of(r: &mut &[u8]) -> Result<u64, WorldFileError> {
    Ok(u64::from_le_bytes(take(r, 8)?.try_into().unwrap()))
}

fn i64_of(r: &mut &[u8]) -> Result<i64, WorldFileError> {
    Ok(i64::from_le_bytes(take(r, 8)?.try_into().unwrap()))
}

fn f64_of(r: &mut &[u8]) -> Result<f64, WorldFileError> {
    Ok(f64::from_le_bytes(take(r, 8)?.try_into().unwrap()))
}

fn string_of(r: &mut &[u8]) -> Result<String, WorldFileError> {
    crate::net::read_string(r).map_err(|_| WorldFileError::TooShort)
}

/// Read the container and report how many bytes it consumed.
///
/// The returned length is the check that matters: `WorldFile.cs:1771` refuses to
/// continue unless the stream is exactly at `positions[0]` after this structure, so a
/// caller can assert the same thing and catch any drift in the metadata size or the
/// importance packing.
pub fn read_container(data: &[u8]) -> Result<(Container, usize), WorldFileError> {
    let mut r = data;
    let version = i32_of(&mut r)?;

    let (revision, favorite) = if version >= 135 {
        let magic = u64_of(&mut r)?;
        if magic & 0x00FF_FFFF_FFFF_FFFF != MAGIC {
            return Err(WorldFileError::BadMagic);
        }
        let file_type = ((magic >> 56) & 0xFF) as u8;
        if file_type != FILE_TYPE_WORLD {
            return Err(WorldFileError::NotAWorld);
        }
        let revision = u32_of(&mut r)?;
        let flags = u64_of(&mut r)?;
        (revision, flags & 1 == 1)
    } else {
        // `FileMetadata.FromCurrentSettings` writes nothing for an old file.
        (0, false)
    };

    let count = i16_of(&mut r)?;
    if count < 0 {
        return Err(WorldFileError::TooShort);
    }
    let mut positions = Vec::with_capacity(count as usize);
    for _ in 0..count {
        positions.push(i32_of(&mut r)?);
    }

    let important = u16_of(&mut r)?;
    let mut importance = Vec::with_capacity(important as usize);
    // LSB-first within each byte: bit i of the block is `(bytes[i / 8] >> (i % 8)) & 1`.
    let mut byte = 0u8;
    for i in 0..important as usize {
        if i % 8 == 0 {
            byte = u8_of(&mut r)?;
        }
        importance.push((byte >> (i % 8)) & 1 == 1);
    }

    Ok((
        Container { version, revision, favorite, positions, importance },
        data.len() - r.len(),
    ))
}

/// Read the world header from the start of `positions[0]`.
///
/// `version` comes from the container: every conditional field in the header is keyed
/// on it, so passing the wrong one mis-reads silently rather than failing.
pub fn read_header(r: &mut &[u8], version: i32) -> Result<WorldHeader, WorldFileError> {
    let name = string_of(r)?;

    let (seed, generator_version) = if version >= 179 {
        // Version 179 exactly stored the seed as an int; every later one as a string.
        let seed = if version != 179 {
            string_of(r)?
        } else {
            i32_of(r)?.to_string()
        };
        (seed, u64_of(r)?)
    } else {
        (String::new(), 0)
    };

    let unique_id: [u8; 16] = if version >= 181 {
        take(r, 16)?.try_into().unwrap()
    } else {
        // The C# invents a GUID for files older than 181 (`WorldFile.cs:1966`). Zero is
        // this reader's way of saying the file carried none, which is not the same thing
        // as a world whose id happens to be all zeros.
        [0u8; 16]
    };

    let world_id = i32_of(r)?;
    let left_world = i32_of(r)?;
    let right_world = i32_of(r)?;
    let top_world = i32_of(r)?;
    let bottom_world = i32_of(r)?;
    // Y before X: the file stores them in this order (`WorldFile.cs:1972`).
    let max_tiles_y = i32_of(r)?;
    let max_tiles_x = i32_of(r)?;

    // Every one of these is version-guarded in the C# (`WorldFile.cs:1998`
    // `LoadWorldFlags`), and reading them unconditionally would shift the whole header
    // silently. The order is the C#'s.
    let game_mode = i32_of(r)?;
    let drunk_world = if version >= 222 { bool_of(r)? } else { false };
    let get_good_world = if version >= 227 { bool_of(r)? } else { false };
    let tenth_anniversary_world = if version >= 238 { bool_of(r)? } else { false };
    let dont_starve_world = if version >= 239 { bool_of(r)? } else { false };
    let not_the_bees_world = if version >= 241 { bool_of(r)? } else { false };
    let remix_world = if version >= 249 { bool_of(r)? } else { false };
    let no_traps_world = if version >= 266 { bool_of(r)? } else { false };
    // Zenith is READ from 267 on, and DERIVED before that.
    let zenith_world = if version >= 267 {
        bool_of(r)?
    } else {
        remix_world && drunk_world
    };
    let skyblock_world = if version >= 302 { bool_of(r)? } else { false };

    let creation_time_binary = if version >= 141 { i64_of(r)? } else { 0 };
    // `>= 284`, not `>= 141`: a 279 world has no last-played field at all.
    let last_played_binary = if version >= 284 { i64_of(r)? } else { 0 };
    let moon_type = u8_of(r)?;

    let mut tree_x = [0i32; 3];
    for v in tree_x.iter_mut() {
        *v = i32_of(r)?;
    }
    let mut tree_style = [0i32; 4];
    for v in tree_style.iter_mut() {
        *v = i32_of(r)?;
    }
    let mut cave_back_x = [0i32; 3];
    for v in cave_back_x.iter_mut() {
        *v = i32_of(r)?;
    }
    let mut cave_back_style = [0i32; 4];
    for v in cave_back_style.iter_mut() {
        *v = i32_of(r)?;
    }
    let ice_back_style = i32_of(r)?;
    let jungle_back_style = i32_of(r)?;
    let hell_back_style = i32_of(r)?;
    let spawn_tile_x = i32_of(r)?;
    let spawn_tile_y = i32_of(r)?;
    let world_surface = f64_of(r)?;
    let rock_layer = f64_of(r)?;

    Ok(WorldHeader {
        name,
        seed,
        generator_version,
        unique_id,
        world_id,
        left_world,
        right_world,
        top_world,
        bottom_world,
        max_tiles_x,
        max_tiles_y,
        game_mode,
        drunk_world,
        get_good_world,
        tenth_anniversary_world,
        dont_starve_world,
        not_the_bees_world,
        remix_world,
        no_traps_world,
        zenith_world,
        skyblock_world,
        creation_time_binary,
        last_played_binary,
        moon_type,
        tree_x,
        tree_style,
        cave_back_x,
        cave_back_style,
        ice_back_style,
        jungle_back_style,
        hell_back_style,
        spawn_tile_x,
        spawn_tile_y,
        world_surface,
        rock_layer,
    })
}

/// The world sizes the interactive world-select offers (`Main.cs:5473-5483`).
pub const VALID_WORLD_SIZES: [(i32, i32); 3] = [(4200, 1200), (6400, 1800), (8400, 2400)];

/// One section of the file, named, with whether this port decodes it yet.
///
/// The order is `WorldFile.cs:1765` `LoadWorld_Version2`: each entry is a section the
/// C# loads in turn, and several only exist from a given version. Naming them is what
/// makes "unimplemented" visible instead of implied: the reader reports every section
/// the file declares and says which two it actually reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub index: usize,
    pub name: &'static str,
    pub offset: i32,
    /// Whether `load_world` decodes this section's contents.
    pub decoded: bool,
}

/// The section names, in the C#'s load order.
pub const SECTION_NAMES: [&str; 11] = [
    "world header",
    "tiles",
    "chests",
    "signs",
    "NPCs",
    "tile entities",
    "weighted pressure plates",
    "town manager",
    "bestiary",
    "creative powers",
    "footer",
];

/// Why a whole world could not be loaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadError {
    /// The container could not be read.
    Container(WorldFileError),
    /// The header could not be read.
    Header(WorldFileError),
    /// The tile section could not be decoded.
    Tiles(crate::tiles::TileError),
    /// A section pointer is negative, out of order, or past the end of the file.
    BadSectionPointer(usize),
}

/// A world, as far as this port can read one.
#[derive(Debug, Clone, PartialEq)]
pub struct LoadedWorld {
    pub container: Container,
    pub header: WorldHeader,
    pub tile_stats: crate::tiles::TileStats,
    pub sections: Vec<Section>,
}

impl LoadedWorld {
    /// The sections this port does not decode yet, by name.
    pub fn undecoded(&self) -> Vec<&'static str> {
        self.sections.iter().filter(|s| !s.decoded).map(|s| s.name).collect()
    }
}

/// Read a whole world: container, header, tiles.
///
/// The two runtime tables `tiles::decode_tiles` takes are supplied permissively here,
/// and that is safe for this function's purpose because NEITHER one can move the cursor:
/// `wall_count` only decides whether a wall id is zeroed, and `save_slopes` only decides
/// whether a shape in the header byte is recorded. The byte position is unaffected by
/// both, so the section-pointer checks still hold. What it does mean is that the
/// reported slope count is an upper bound, which the caller says out loud rather than
/// hiding.
pub fn load_world(data: &[u8]) -> Result<LoadedWorld, LoadError> {
    let (container, used) = read_container(data).map_err(LoadError::Container)?;
    if container.positions.is_empty() {
        return Err(LoadError::BadSectionPointer(0));
    }
    // The C#'s own check, reproduced: the container must end where the header begins.
    if used as i32 != container.positions[0] {
        return Err(LoadError::BadSectionPointer(0));
    }
    let header_start = container.positions[0] as usize;
    if header_start >= data.len() {
        return Err(LoadError::BadSectionPointer(0));
    }
    let mut r = &data[header_start..];
    let header = read_header(&mut r, container.version).map_err(LoadError::Header)?;

    // The tile section is the span from positions[1] to positions[2]; the C# refuses to
    // continue unless the decode ends exactly on the latter.
    let tile_stats = if container.positions.len() >= 3 {
        let start = container.positions[1];
        let end = container.positions[2];
        if start < 0 || end < start || end as usize > data.len() {
            return Err(LoadError::BadSectionPointer(1));
        }
        let section = &data[start as usize..end as usize];
        let save_slopes = vec![true; 0x10000];
        let (stats, consumed) = crate::tiles::decode_tiles(
            section,
            header.max_tiles_x,
            header.max_tiles_y,
            &container.importance,
            &save_slopes,
            u16::MAX,
        )
        .map_err(LoadError::Tiles)?;
        if consumed != (end - start) as usize {
            return Err(LoadError::BadSectionPointer(2));
        }
        stats
    } else {
        crate::tiles::TileStats::default()
    };

    let sections = container
        .positions
        .iter()
        .enumerate()
        .map(|(i, offset)| Section {
            index: i,
            name: SECTION_NAMES.get(i).copied().unwrap_or("unknown section"),
            offset: *offset,
            // Only the first two are read. The rest are NAMED so the gap is visible.
            decoded: i <= 1,
        })
        .collect();

    Ok(LoadedWorld { container, header, tile_stats, sections })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    /// Build a container the way the C# writes one, so the reader has something to
    /// fail against without a world file.
    fn synthetic(version: i32, positions: &[i32], importance: &[bool]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&version.to_le_bytes());
        if version >= 135 {
            let magic = MAGIC | ((FILE_TYPE_WORLD as u64) << 56);
            out.extend_from_slice(&magic.to_le_bytes());
            out.extend_from_slice(&7u32.to_le_bytes());
            out.extend_from_slice(&1u64.to_le_bytes()); // favorite
        }
        out.extend_from_slice(&(positions.len() as i16).to_le_bytes());
        for p in positions {
            out.extend_from_slice(&p.to_le_bytes());
        }
        out.extend_from_slice(&(importance.len() as u16).to_le_bytes());
        let mut i = 0;
        while i < importance.len() {
            let mut byte = 0u8;
            for bit in 0..8 {
                if i + bit < importance.len() && importance[i + bit] {
                    byte |= 1 << bit;
                }
            }
            out.push(byte);
            i += 8;
        }
        out
    }

    #[test]
    fn a_container_round_trips_and_reports_its_length() {
        let data = synthetic(KNOWN_VERSION, &[100, 200, 300], &[true, false, true, false, true]);
        let (c, used) = read_container(&data).unwrap();
        assert_eq!(c.version, KNOWN_VERSION);
        assert_eq!(c.revision, 7);
        assert!(c.favorite);
        assert_eq!(c.positions, vec![100, 200, 300]);
        assert_eq!(c.importance, vec![true, false, true, false, true]);
        assert_eq!(used, data.len(), "the container is the whole fixture");
    }

    /// The importance bits are LSB-first within each byte, which is the packing the
    /// C# walks with a shifting mask.
    #[test]
    fn importance_is_lsb_first() {
        let mut bits = vec![false; 10];
        bits[0] = true;
        bits[7] = true;
        bits[8] = true;
        let data = synthetic(KNOWN_VERSION, &[1], &bits);
        let (c, _) = read_container(&data).unwrap();
        assert_eq!(c.importance, bits);
        // Byte 0 holds indices 0..7, byte 1 holds 8..15.
        assert!(c.importance[0] && c.importance[7] && c.importance[8]);
        assert!(!c.importance[1] && !c.importance[9]);
    }

    #[test]
    fn a_foreign_file_is_refused_by_name() {
        let mut data = synthetic(KNOWN_VERSION, &[1], &[]);
        // Corrupt the magic: the low 7 bytes must match exactly.
        data[4] ^= 0xFF;
        assert_eq!(read_container(&data), Err(WorldFileError::BadMagic));

        // Right magic, wrong type byte: a .plr, not a .wld.
        let mut data = synthetic(KNOWN_VERSION, &[1], &[]);
        data[11] = 3; // top byte of the little-endian u64
        assert_eq!(read_container(&data), Err(WorldFileError::NotAWorld));
    }

    #[test]
    fn a_truncated_file_is_too_short_not_a_panic() {
        let data = synthetic(KNOWN_VERSION, &[100, 200], &[true]);
        for cut in 0..data.len() {
            let r = read_container(&data[..cut]);
            assert_eq!(r, Err(WorldFileError::TooShort), "cut at {cut}");
        }
    }

    /// Where the world files live on this machine. The real-file check below is only
    /// as good as the presence of one, and says so rather than pretending.
    fn worlds_dir() -> Option<PathBuf> {
        let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?;
        let dir = PathBuf::from(home).join("Documents").join("My Games").join("Terraria").join("Worlds");
        if dir.is_dir() {
            Some(dir)
        } else {
            None
        }
    }

    /// The per-file checks, so every world present is checked rather than the first.
    fn check_one(path: &std::path::Path) -> WorldHeader {
        let data = fs::read(path).expect("the world file is readable");
        let (c, used) = read_container(&data).expect("a real world's container parses");
        assert!(!c.positions.is_empty());
        assert_eq!(
            used as i32, c.positions[0],
            "{}: the container must end exactly where the C# says the header begins",
            path.display()
        );
        // The header starts at that pointer, so the C# check is reproduced rather than
        // approximated.
        let mut r = &data[c.positions[0] as usize..];
        let h = read_header(&mut r, c.version).expect("the header parses");

        assert!(!h.name.is_empty(), "a world has a name");
        assert!(h.name.is_ascii(), "world names are ascii in practice: {:?}", h.name);
        assert!(h.max_tiles_x > 0 && h.max_tiles_y > 0);
        assert!(
            VALID_WORLD_SIZES.contains(&(h.max_tiles_x, h.max_tiles_y)),
            "{}x{} is not one of the three sizes the game offers",
            h.max_tiles_x,
            h.max_tiles_y
        );
        assert!(h.right_world >= h.left_world && h.bottom_world >= h.top_world);
        assert!(
            h.spawn_tile_x >= 0 && h.spawn_tile_x < h.max_tiles_x,
            "spawn x {} is outside 0..{}",
            h.spawn_tile_x,
            h.max_tiles_x
        );
        assert!(
            h.spawn_tile_y >= 0 && h.spawn_tile_y < h.max_tiles_y,
            "spawn y {} is outside 0..{}",
            h.spawn_tile_y,
            h.max_tiles_y
        );
        assert!(
            h.world_surface > 0.0 && h.world_surface <= h.rock_layer,
            "surface {} must be above rock layer {}",
            h.world_surface,
            h.rock_layer
        );
        assert!(h.rock_layer < h.max_tiles_y as f64);
        assert!(h.unique_id.iter().any(|b| *b != 0), "the GUID is not all zeros");
        assert!(!h.seed.is_empty() || h.generator_version > 0);
        println!(
            "{} (file version {}): {:?} {}x{} mode={} seed={:?} spawn=({},{}) surface={} rock={}",
            path.file_name().unwrap().to_string_lossy(),
            c.version,
            h.name,
            h.max_tiles_x,
            h.max_tiles_y,
            h.game_mode,
            h.seed,
            h.spawn_tile_x,
            h.spawn_tile_y,
            h.world_surface,
            h.rock_layer
        );
        h
    }

    /// `load_world` ties the three pieces together, and names what it did NOT read.
    ///
    /// The undecoded list is the part that matters: a loader that reported only its own
    /// successes would be indistinguishable from one that reads everything.
    #[test]
    fn load_world_reports_what_it_did_not_read() {
        let Some(dir) = worlds_dir() else {
            println!("no Terraria Worlds directory; the load_world check did not run");
            return;
        };
        let mut files: Vec<PathBuf> = fs::read_dir(&dir)
            .expect("the Worlds directory is readable")
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "wld"))
            .collect();
        files.sort();
        let Some(path) = files.first() else {
            println!("no .wld in {}; the load_world check did not run", dir.display());
            return;
        };

        let data = fs::read(path).expect("the world file is readable");
        let w = load_world(&data).expect("the world loads");
        assert!(!w.header.name.is_empty());
        assert!(w.tile_stats.active > 0);
        // Eleven sections in a 1.4.4 file, and the first two are the ones read.
        assert!(w.sections.len() >= 3, "only {} sections", w.sections.len());
        assert_eq!(w.sections[0].name, "world header");
        assert_eq!(w.sections[1].name, "tiles");
        assert!(w.sections[0].decoded && w.sections[1].decoded);
        assert!(
            w.sections[2..].iter().all(|s| !s.decoded),
            "only the header and the tiles are read"
        );
        assert!(w.undecoded().contains(&"chests"));
        // Every section pointer is inside the file and in order.
        for pair in w.sections.windows(2) {
            assert!(pair[0].offset <= pair[1].offset, "{:?} then {:?}", pair[0], pair[1]);
        }
        let last = w.sections.last().unwrap();
        assert!(last.offset >= 0 && (last.offset as usize) < data.len());
    }

    /// A file whose section pointers disagree with its own contents is refused rather
    /// than decoded optimistically.
    #[test]
    fn load_world_refuses_a_broken_section_pointer() {
        // A container whose positions[0] does not match where the container ended.
        let mut data = synthetic(KNOWN_VERSION, &[999, 1000, 1001], &[false]);
        data.extend_from_slice(&[0u8; 64]);
        assert_eq!(load_world(&data), Err(LoadError::BadSectionPointer(0)));

        // A container that ends correctly but declares an empty section table.
        let data = synthetic(KNOWN_VERSION, &[], &[]);
        assert_eq!(load_world(&data), Err(LoadError::BadSectionPointer(0)));
    }

    /// A REAL world file, end to end: the container must land exactly on `positions[0]`,
    /// and the header must describe a plausible world.
    ///
    /// EVERY `.wld` present is checked, not the first, because a second file is what
    /// would expose a version guard that is wrong - the two on this machine were written
    /// fifteen months apart. On a machine with no world files this asserts nothing and
    /// says so out loud; the synthetic tests above are what run everywhere.
    #[test]
    fn a_real_world_parses_and_lands_on_the_header_pointer() {
        let Some(dir) = worlds_dir() else {
            println!("no Terraria Worlds directory; the real-file check did not run");
            return;
        };
        let mut files: Vec<PathBuf> = fs::read_dir(&dir)
            .expect("the Worlds directory is readable")
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "wld"))
            .collect();
        files.sort();
        if files.is_empty() {
            println!("no .wld in {}; the real-file check did not run", dir.display());
            return;
        }
        let headers: Vec<WorldHeader> = files.iter().map(|p| check_one(p)).collect();
        assert_eq!(headers.len(), files.len());
    }
}
