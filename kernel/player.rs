//! player: the server's view of a connected player, and the rules it applies to
//! what a client claims about itself.
//!
//! This is the first slice of the `player` unit (`sheets/02-plan.tsv`, ranked 1 by
//! measured workload), and it is deliberately the smallest useful one. `SyncPlayer`
//! (message 4) is the first thing a vanilla client sends after `PlayerInfo`, and the
//! C# sanitises three of its fields before storing them (`MessageBuffer.cs:296-305`).
//! A server that stores an unclamped variant stores a value no client could have
//! sent, and the generated `Player` struct would hold it happily, because a
//! projected shape has no opinions - the clamps are behaviour, so they live here.

use std::io::{self, Cursor, Read, Write};

use crate::port::terraria::id::PlayerVariantID;

/// The highest valid skin variant.
///
/// The C# is `MathHelper.Clamp(skinVariant, 0f, PlayerVariantID.Count - 1)`
/// (`MessageBuffer.cs:298`). `Count` is a `public static readonly` row and the book
/// carries no value for it, so it cannot be read as a constant: the projection emits
/// it as a struct field, not a const. Its MEMBERS do carry values, though, and the
/// highest of them is the same bound, so it is derived from those rather than typed.
/// That keeps the clamp tied to the id table: adding a variant to the sheet moves
/// this bound, and a number written here would not.
pub const MAX_SKIN_VARIANT: i32 = PlayerVariantID::FemaleDisplayDoll;

/// `MessageBuffer.cs:298`: `MathHelper.Clamp(skinVariant, 0f, PlayerVariantID.Count - 1)`.
pub fn clamp_skin_variant(v: i32) -> i32 {
    v.clamp(0, MAX_SKIN_VARIANT)
}

/// `MessageBuffer.cs:300`: `Utils.Clamp(voiceVariant, 1, 4)`.
///
/// The bounds are literals in the C#, with no id table behind them, so they are
/// cited rather than derived. There is no `PlayerVoiceID` range to read: the sheet
/// rows for that type are a `Sets` class, not an enumeration of variants.
pub fn clamp_voice_variant(v: i32) -> i32 {
    v.clamp(1, 4)
}

/// `MessageBuffer.cs:301-304`: a NaN pitch becomes zero.
///
/// The C# guards only against NaN and otherwise stores the float verbatim - it does
/// not range-check it - so neither does this. Reproducing the guard without
/// inventing a range is the point: an out-of-range pitch is a client's business.
pub fn sanitise_voice_pitch(f: f32) -> f32 {
    if f.is_nan() {
        0.0
    } else {
        f
    }
}

/// `Player.cs:1507`: `public static readonly int maxBuffs = 44`.
///
/// A `readonly` field, so its row carries no value and the projection emits it as a
/// struct field rather than a constant - there is nothing to read. It is cited from
/// the declaration instead, the same treatment the voice bounds get.
pub const MAX_BUFFS: usize = 44;

/// `Player.cs:5459` `DelBuff`: clear one slot, then compact the list upward.
///
/// Reproduced exactly, including the bound: the C# loop is `i < maxBuffs - 1`, so
/// the LAST slot is never a source for compaction. That looks like an off-by-one and
/// may well be one, but parity means reproducing it, and the test below pins it so a
/// future "fix" has to be deliberate rather than accidental.
///
/// The C# would throw `IndexOutOfRangeException` for an out-of-range `b`; the slice
/// index here panics the same way rather than silently ignoring it.
pub fn del_buff(buff_type: &mut [i32], buff_time: &mut [i32], b: usize) {
    buff_time[b] = 0;
    buff_type[b] = 0;
    let n = MAX_BUFFS.min(buff_type.len()).min(buff_time.len());
    let mut num = 0usize;
    for i in 0..n.saturating_sub(1) {
        if buff_time[i] != 0 && buff_type[i] != 0 {
            if num < i {
                buff_time[num] = buff_time[i];
                buff_type[num] = buff_type[i];
                buff_time[i] = 0;
                buff_type[i] = 0;
            }
            num += 1;
        }
    }
}

/// `Player.cs:5480` `ClearBuff`: drop every slot holding `buff_type_id`.
///
/// Forward iteration, calling `del_buff` as it goes - which compacts underneath the
/// loop, so a later duplicate can be skipped. That is what the C# does; the loop
/// bound is `maxBuffs` here, unlike `del_buff`'s `maxBuffs - 1`.
pub fn clear_buff(buff_type: &mut [i32], buff_time: &mut [i32], buff_type_id: i32) {
    let n = MAX_BUFFS.min(buff_type.len());
    for i in 0..n {
        if buff_type[i] == buff_type_id {
            del_buff(buff_type, buff_time, i);
        }
    }
}

/// `Player.cs:5491` `CountBuffs`.
///
/// The C# loop variable is `i` but it indexes `buffType[num]`:
///
/// ```text
/// for (int i = 0; i < maxBuffs; i++)
///     if (buffType[num] > 0) num++;
/// ```
///
/// So it counts the unbroken PREFIX of non-zero entries rather than all of them: a
/// gap stops the count even if buffs sit after it. A tidy reimplementation would
/// return a different number, so this reproduces the original and the test pins the
/// gap behaviour explicitly.
pub fn count_buffs(buff_type: &[i32]) -> usize {
    let n = MAX_BUFFS.min(buff_type.len());
    let mut num = 0usize;
    for _ in 0..n {
        if buff_type[num] > 0 {
            num += 1;
        }
    }
    num
}

/// `MessageBuffer.cs:1105`: `if (player9.statLifeMax < 20) player9.statLifeMax = 20;`
///
/// A literal in the C#, with no id table or constant behind it, so it is cited.
pub const MIN_LIFE_MAX: i16 = 20;

/// The life and mana a client claims about itself, after the server has sanitised it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LifeMana {
    /// `Player.cs:571`. Stored as the client sent it - the C# does not clamp it.
    pub stat_life: i32,
    /// `Player.cs:569`. Floored at `MIN_LIFE_MAX`.
    pub stat_life_max: i32,
    /// `Player.cs` `dead`, derived rather than read off the wire.
    pub dead: bool,
}

/// `MessageBuffer.cs:1103-1112`, the server's handling of `PlayerLifeMana` (16).
///
/// This is the second thing a client sends after `PlayerInfo` (it is in the
/// `EARLY_ALLOWED` set at `MessageBuffer.cs:165`), so it is on the same early path as
/// the appearance clamps above. Two decisions are worth naming:
///
/// 1. `statLifeMax` is floored at 20 and `statLife` is NOT clamped to it, so a client
///    may claim more life than its maximum and the C# stores that verbatim.
/// 2. `dead` is DERIVED from `statLife <= 0` rather than read from a flag, so a client
///    cannot declare itself alive at zero life or dead at full.
pub fn sanitise_life(stat_life: i16, stat_life_max: i16) -> LifeMana {
    LifeMana {
        stat_life: stat_life as i32,
        stat_life_max: if stat_life_max < MIN_LIFE_MAX {
            MIN_LIFE_MAX as i32
        } else {
            stat_life_max as i32
        },
        dead: stat_life <= 0,
    }
}

/// `Player.cs:5255` `FindBuffIndex`: where a buff sits, or -1.
///
/// Two rules are easy to miss and are both reproduced:
///
/// 1. An IMMUNE buff is never found, even when it is held: `buffImmune[type]` returns
///    -1 before the search runs.
/// 2. A slot must still have `buffTime >= 1`, so a slot with a type but no time left
///    is invisible. Checking the type alone would find expired buffs.
pub fn find_buff_index(
    buff_immune: &[bool],
    buff_type: &[i32],
    buff_time: &[i32],
    buff_type_id: i32,
) -> i32 {
    let idx = buff_type_id as usize;
    if buff_immune.get(idx).copied().unwrap_or(false) {
        return -1;
    }
    let n = MAX_BUFFS.min(buff_type.len()).min(buff_time.len());
    for i in 0..n {
        if buff_time[i] >= 1 && buff_type[i] == buff_type_id {
            return i as i32;
        }
    }
    -1
}

/// `Player.cs:53240`: `int num = (int)((float)amount * manaCost);`
///
/// A float multiply and a truncating cast, so `7 * 0.5` costs 3, not 4. Rounding
/// here would desynchronise the client's prediction from the server's deduction.
///
/// One honest divergence: C#'s `(int)` cast is unchecked and can wrap, while `as i32`
/// in Rust saturates. The difference is only reachable for a `manaCost` far outside
/// any real one, since real costs are ratios near 1.
pub fn mana_cost(amount: i32, mana_cost: f32) -> i32 {
    (amount as f32 * mana_cost) as i32
}

/// `Player.cs:53236` `CheckMana`, the part that needs no item state.
///
/// Returns whether the cost is affordable, and the mana left after paying. The C#
/// also has a `manaFlower` / `QuickMana` path and a `DebugOptions.ManaV2` path; both
/// need state this slice does not have (restorable mana potions, a debug flag), so
/// they are NOT reproduced here and are recorded as pending rather than guessed at.
/// `slowMagicUse` is likewise a field write left to the caller.
pub fn check_mana(stat_mana: i32, amount: i32, mana_cost_ratio: f32, pay: bool) -> (bool, i32) {
    let cost = mana_cost(amount, mana_cost_ratio);
    if stat_mana >= cost {
        (true, if pay { stat_mana - cost } else { stat_mana })
    } else {
        // Not affordable: the C# returns before deducting, so `pay` changes nothing.
        (false, stat_mana)
    }
}

/// `NetMessage.cs:1905` `WriteAccessoryVisibility`: bit i of a ushort, little-endian.
///
/// The C# shifts into a `ushort`, so an index at or beyond 16 contributes nothing -
/// the bits are gone before the write. The array the game passes holds roughly
/// fourteen flags, so the only indices that matter are the low ones; the guard records
/// the behaviour instead of relying on Rust's shift overflowing in a debug build.
pub fn accessory_visibility(hide_visible_accessory: &[bool]) -> u16 {
    let mut bits = 0u16;
    for (i, hidden) in hide_visible_accessory.iter().enumerate() {
        if *hidden && i < 16 {
            bits |= 1 << i;
        }
    }
    bits
}

/// `NetMessage.cs:179-190`: the difficulty bits, including the gap at bit 2.
///
/// Normal (0) sets nothing at all; Expert (1) is bit 0; Master (2) is bit 1; and
/// Journey (3) is **bit 3, not bit 2**, because bit 2 carries `extraAccessory`. Packing
/// Journey as bit 2 would silently turn a Journey player into an Expert one with an
/// extra accessory slot.
pub fn difficulty_bits(difficulty: i32, extra_accessory: bool) -> u8 {
    let mut bits = 0u8;
    match difficulty {
        1 => bits |= 1 << 0,
        2 => bits |= 1 << 1,
        3 => bits |= 1 << 3,
        _ => {}
    }
    if extra_accessory {
        bits |= 1 << 2;
    }
    bits
}

/// The fields `SyncPlayer` (4) puts on the wire (`NetMessage.cs:156-206`).
///
/// A plain input struct rather than the projected `Player`: that type has 1,313 fields
/// and cannot be built in a test, and the message carries only these, so reaching for
/// anything else would mean reading state the message does not have.
#[derive(Debug, Clone, Copy)]
pub struct SyncPlayer<'a> {
    pub slot: u8,
    pub skin_variant: i32,
    pub voice_variant: i32,
    pub voice_pitch_offset: f32,
    pub haircut: i32,
    pub name: &'a str,
    /// A `byte`, not a string. `terraria.player.hairdye` is type `byte`, and the reader
    /// uses `ReadByte()` (`MessageBuffer.cs:311`) where this writer uses
    /// `writer.Write(player6.hairDye)` - the same single byte, because the field is one.
    /// Writing a string here (as this file first did) shifts every field after it.
    pub hair_dye: u8,
    pub hide_visible_accessory: &'a [bool],
    pub hide_misc: u8,
    /// hair, skin, eye, shirt, underShirt, pants, shoe - in that order.
    pub colors: [(u8, u8, u8); 7],
    pub difficulty: i32,
    pub extra_accessory: bool,
    pub using_biome_torches: bool,
    pub happy_fun_torch_time: bool,
    pub unlocked_biome_torches: bool,
    pub unlocked_super_cart: bool,
    pub enabled_super_cart: bool,
    pub used_aegis_crystal: bool,
    pub used_aegis_fruit: bool,
    pub used_arcane_crystal: bool,
    pub used_galaxy_pearl: bool,
    pub used_gummy_worm: bool,
    pub used_ambrosia: bool,
    pub ate_artisan_bread: bool,
}

/// Write the body of `SyncPlayer` (4). The caller frames it with the id and length.
///
/// Field order is the C#'s, and every packed byte is a `BitsByte`, which is ONE byte
/// with bit i holding the flag the C# names at index i. The colours go through
/// `Utils.WriteRGB` (`Utils.cs:1414`), which writes R, G then B as three bytes.
pub fn write_sync_player<W: Write>(w: &mut W, p: &SyncPlayer<'_>) -> io::Result<()> {
    w.write_all(&[p.slot, p.skin_variant as u8, p.voice_variant as u8])?;
    w.write_all(&p.voice_pitch_offset.to_le_bytes())?;
    w.write_all(&[p.haircut as u8])?;
    crate::net::write_string(w, p.name)?;
    w.write_all(&[p.hair_dye])?;
    w.write_all(&accessory_visibility(p.hide_visible_accessory).to_le_bytes())?;
    w.write_all(&[p.hide_misc])?;
    for (r, g, b) in p.colors {
        w.write_all(&[r, g, b])?;
    }
    w.write_all(&[difficulty_bits(p.difficulty, p.extra_accessory)])?;

    let mut biome = 0u8;
    if p.using_biome_torches {
        biome |= 1 << 0;
    }
    if p.happy_fun_torch_time {
        biome |= 1 << 1;
    }
    if p.unlocked_biome_torches {
        biome |= 1 << 2;
    }
    if p.unlocked_super_cart {
        biome |= 1 << 3;
    }
    if p.enabled_super_cart {
        biome |= 1 << 4;
    }
    w.write_all(&[biome])?;

    let mut crystal = 0u8;
    if p.used_aegis_crystal {
        crystal |= 1 << 0;
    }
    if p.used_aegis_fruit {
        crystal |= 1 << 1;
    }
    if p.used_arcane_crystal {
        crystal |= 1 << 2;
    }
    if p.used_galaxy_pearl {
        crystal |= 1 << 3;
    }
    if p.used_gummy_worm {
        crystal |= 1 << 4;
    }
    if p.used_ambrosia {
        crystal |= 1 << 5;
    }
    if p.ate_artisan_bread {
        crystal |= 1 << 6;
    }
    w.write_all(&[crystal])?;
    Ok(())
}

/// `MessageBuffer.cs:307`: `if (player18.hair >= 228) player18.hair = 0;`
///
/// A literal in the C#, so it is cited. An out-of-range haircut becomes the first one
/// rather than being clamped to the last, which is what makes it worth naming.
pub const MAX_HAIR: i32 = 228;

/// `Player.cs:1437`: `public static int nameLen = 20`.
///
/// A plain `static`, not a `readonly` one, so its row carries no value and there is
/// nothing to project; it is cited from the declaration.
pub const NAME_LEN: usize = 20;

/// The wire carries the accessory-visibility flags as a `ushort`, so sixteen is what
/// the message can express (`NetMessage.cs:1905`).
pub const ACCESSORY_SLOTS: usize = 16;

/// `MessageBuffer.cs:305-306`: the pitch a client sent, made safe.
///
/// The READ side does more than the write side: it replaces NaN with zero and then
/// clamps into -1..1. The writer stores the float verbatim, so the range rule exists
/// only here, which is why `sanitise_voice_pitch` (NaN only) is not enough on its own.
pub fn clamp_received_pitch(f: f32) -> f32 {
    sanitise_voice_pitch(f).clamp(-1.0, 1.0)
}

/// `MessageBuffer.cs:307-310`: an out-of-range haircut becomes 0, not the maximum.
pub fn sanitise_hair(hair: i32) -> i32 {
    if hair >= MAX_HAIR {
        0
    } else {
        hair
    }
}

/// `MessageBuffer.cs:320-337`: decode the difficulty byte.
///
/// The C# tests the bits in order 0, 1, then 3, each ASSIGNING, so the last one set
/// wins - and `extraAccessory` is bit 2. Normal is the absence of all three. The
/// `difficulty > 3` guard that follows in the C# cannot fire, because no path assigns
/// more than 3; it is not reproduced, and this comment is why.
pub fn difficulty_from_bits(bits: u8) -> i32 {
    let mut difficulty = 0;
    if bits & (1 << 0) != 0 {
        difficulty = 1;
    }
    if bits & (1 << 1) != 0 {
        difficulty = 2;
    }
    if bits & (1 << 3) != 0 {
        difficulty = 3;
    }
    difficulty
}

/// `MessageBuffer.cs:1905` read back: one bit per accessory slot.
pub fn read_accessory_visibility(bits: u16) -> [bool; ACCESSORY_SLOTS] {
    let mut out = [false; ACCESSORY_SLOTS];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = bits & (1 << i) != 0;
    }
    out
}

/// Why a `SyncPlayer` body was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncPlayerError {
    /// The body ended before the message did.
    TooShort,
    /// `Net.NameTooLong` (`MessageBuffer.cs:377`).
    NameTooLong,
    /// `Net.EmptyName` (`MessageBuffer.cs:381`).
    EmptyName,
}

impl SyncPlayerError {
    /// The `Net.*` key the C# kicks with, or `None` when the C# has no text for it.
    ///
    /// Read from the book - the rows are `net.nametoolong` and `net.emptyname` - so a
    /// caller cannot kick with a key that does not exist. `TooShort` has no key because
    /// the C# has no message for it: a body that ends early is a framing failure, not a
    /// client to be explained to.
    pub fn key(self) -> Option<&'static str> {
        match self {
            SyncPlayerError::TooShort => None,
            SyncPlayerError::NameTooLong => {
                crate::sheets::localization::by_id("net.nametoolong").map(|d| d.key)
            }
            SyncPlayerError::EmptyName => {
                crate::sheets::localization::by_id("net.emptyname").map(|d| d.key)
            }
        }
    }
}

/// A `SyncPlayer` (4) body after the server has sanitised it.
#[derive(Debug, Clone, PartialEq)]
pub struct SyncPlayerRead {
    pub slot: u8,
    pub skin_variant: i32,
    pub voice_variant: i32,
    pub voice_pitch_offset: f32,
    pub haircut: i32,
    pub name: String,
    pub hair_dye: u8,
    pub hide_visible_accessory: [bool; ACCESSORY_SLOTS],
    pub hide_misc: u8,
    pub colors: [(u8, u8, u8); 7],
    pub difficulty: i32,
    pub extra_accessory: bool,
    pub using_biome_torches: bool,
    pub happy_fun_torch_time: bool,
    pub unlocked_biome_torches: bool,
    pub unlocked_super_cart: bool,
    pub enabled_super_cart: bool,
    pub used_aegis_crystal: bool,
    pub used_aegis_fruit: bool,
    pub used_arcane_crystal: bool,
    pub used_galaxy_pearl: bool,
    pub used_gummy_worm: bool,
    pub used_ambrosia: bool,
    pub ate_artisan_bread: bool,
}

fn take_u8(c: &mut Cursor<&[u8]>) -> Result<u8, SyncPlayerError> {
    let mut b = [0u8; 1];
    c.read_exact(&mut b).map_err(|_| SyncPlayerError::TooShort)?;
    Ok(b[0])
}

fn take_u16(c: &mut Cursor<&[u8]>) -> Result<u16, SyncPlayerError> {
    let mut b = [0u8; 2];
    c.read_exact(&mut b).map_err(|_| SyncPlayerError::TooShort)?;
    Ok(u16::from_le_bytes(b))
}

fn take_f32(c: &mut Cursor<&[u8]>) -> Result<f32, SyncPlayerError> {
    let mut b = [0u8; 4];
    c.read_exact(&mut b).map_err(|_| SyncPlayerError::TooShort)?;
    Ok(f32::from_le_bytes(b))
}

/// Read a `SyncPlayer` (4) body: `MessageBuffer.cs:284-400`.
///
/// Every field rule the C# applies is applied here, and the reader is the stricter
/// half - it clamps the skin and voice, replaces a NaN pitch AND clamps it to -1..1,
/// sends a haircut at or past `MAX_HAIR` to zero, and trims the name.
///
/// The C# then does two checks that need state this function does not have, so they
/// are left to the caller: a duplicate name against the other connected players (kick
/// `Lang.mp[5]`, `MessageBuffer.cs:359-370`) and a difficulty/world mismatch
/// (`Net.PlayerIsCreativeAndWorldIsNotCreative` and its converse, `:385-393`).
///
/// `slot` is returned as the wire carried it. On the server the C# DISCARDS it and uses
/// the connection's own slot (`if (Main.netMode == 2) num188 = whoAmI;`), so a caller
/// must do the same - the field is not trusted.
pub fn read_sync_player(body: &[u8]) -> Result<SyncPlayerRead, SyncPlayerError> {
    let mut c = Cursor::new(body);
    let slot = take_u8(&mut c)?;
    let skin_variant = clamp_skin_variant(take_u8(&mut c)? as i32);
    let voice_variant = clamp_voice_variant(take_u8(&mut c)? as i32);
    let voice_pitch_offset = clamp_received_pitch(take_f32(&mut c)?);
    let haircut = sanitise_hair(take_u8(&mut c)? as i32);
    let name = crate::net::read_string(&mut c)
        .map_err(|_| SyncPlayerError::TooShort)?
        .trim()
        .to_string();
    let hair_dye = take_u8(&mut c)?;
    let hide_visible_accessory = read_accessory_visibility(take_u16(&mut c)?);
    let hide_misc = take_u8(&mut c)?;
    let mut colors = [(0u8, 0u8, 0u8); 7];
    for color in colors.iter_mut() {
        *color = (take_u8(&mut c)?, take_u8(&mut c)?, take_u8(&mut c)?);
    }
    let difficulty_byte = take_u8(&mut c)?;
    let difficulty = difficulty_from_bits(difficulty_byte);
    let extra_accessory = difficulty_byte & (1 << 2) != 0;
    let biome = take_u8(&mut c)?;
    let crystal = take_u8(&mut c)?;

    // `MessageBuffer.cs:377-384`: length first, then emptiness.
    if name.len() > NAME_LEN {
        return Err(SyncPlayerError::NameTooLong);
    }
    if name.is_empty() {
        return Err(SyncPlayerError::EmptyName);
    }

    Ok(SyncPlayerRead {
        slot,
        skin_variant,
        voice_variant,
        voice_pitch_offset,
        haircut,
        name,
        hair_dye,
        hide_visible_accessory,
        hide_misc,
        colors,
        difficulty,
        extra_accessory,
        using_biome_torches: biome & (1 << 0) != 0,
        happy_fun_torch_time: biome & (1 << 1) != 0,
        unlocked_biome_torches: biome & (1 << 2) != 0,
        unlocked_super_cart: biome & (1 << 3) != 0,
        enabled_super_cart: biome & (1 << 4) != 0,
        used_aegis_crystal: crystal & (1 << 0) != 0,
        used_aegis_fruit: crystal & (1 << 1) != 0,
        used_arcane_crystal: crystal & (1 << 2) != 0,
        used_galaxy_pearl: crystal & (1 << 3) != 0,
        used_gummy_worm: crystal & (1 << 4) != 0,
        used_ambrosia: crystal & (1 << 5) != 0,
        ate_artisan_bread: crystal & (1 << 6) != 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bound is the id table's own highest member, not a number typed here.
    /// `Count` would be 12, and `Count - 1` is 11, which is `FemaleDisplayDoll`.
    #[test]
    fn the_skin_bound_is_the_highest_variant_in_the_book() {
        assert_eq!(MAX_SKIN_VARIANT, PlayerVariantID::FemaleDisplayDoll);
        assert_eq!(MAX_SKIN_VARIANT, 11, "the highest variant value in the sheet");
        // Every member the sheet declares is inside the clamp, which is what makes
        // this a bound rather than a guess.
        for v in [
            PlayerVariantID::MaleStarter,
            PlayerVariantID::MaleSticker,
            PlayerVariantID::MaleGangster,
            PlayerVariantID::MaleCoat,
            PlayerVariantID::FemaleStarter,
            PlayerVariantID::FemaleSticker,
            PlayerVariantID::FemaleGangster,
            PlayerVariantID::FemaleCoat,
            PlayerVariantID::MaleDress,
            PlayerVariantID::FemaleDress,
            PlayerVariantID::MaleDisplayDoll,
            PlayerVariantID::FemaleDisplayDoll,
        ] {
            assert_eq!(clamp_skin_variant(v), v, "member {v} must survive its own clamp");
        }
    }

    /// The clamp matches `MathHelper.Clamp`: below is min, above is max, inside is
    /// itself.
    #[test]
    fn the_skin_clamp_is_the_c_sharp_clamp() {
        assert_eq!(clamp_skin_variant(-1), 0);
        assert_eq!(clamp_skin_variant(0), 0);
        assert_eq!(clamp_skin_variant(5), 5);
        assert_eq!(clamp_skin_variant(11), 11);
        assert_eq!(clamp_skin_variant(12), 11);
        assert_eq!(clamp_skin_variant(i32::MAX), 11);
        assert_eq!(clamp_skin_variant(i32::MIN), 0);
    }

    /// The voice clamp is 1..4, so 0 is not a valid voice and is raised rather than
    /// rejected - the C# clamps, it does not disconnect.
    #[test]
    fn the_voice_clamp_is_one_to_four() {
        assert_eq!(clamp_voice_variant(-5), 1);
        assert_eq!(clamp_voice_variant(0), 1);
        assert_eq!(clamp_voice_variant(1), 1);
        assert_eq!(clamp_voice_variant(4), 4);
        assert_eq!(clamp_voice_variant(5), 4);
    }

    /// Only NaN is replaced, and `-0.0` and infinities are left alone because the
    /// C# leaves them alone.
    #[test]
    fn only_a_nan_pitch_is_replaced() {
        assert_eq!(sanitise_voice_pitch(f32::NAN), 0.0);
        assert!(sanitise_voice_pitch(f32::NAN).is_sign_positive());
        assert_eq!(sanitise_voice_pitch(0.0), 0.0);
        assert_eq!(sanitise_voice_pitch(-1.5), -1.5);
        assert_eq!(sanitise_voice_pitch(f32::INFINITY), f32::INFINITY);
        assert_eq!(sanitise_voice_pitch(f32::NEG_INFINITY), f32::NEG_INFINITY);
        assert!(sanitise_voice_pitch(-0.0).is_sign_negative(), "-0.0 is preserved");
    }

    /// A fresh player has no buffs, and the count follows the C#'s prefix rule.
    #[test]
    fn count_buffs_counts_the_unbroken_prefix() {
        let empty = [0i32; MAX_BUFFS];
        assert_eq!(count_buffs(&empty), 0);

        let mut one = [0i32; MAX_BUFFS];
        one[0] = 5;
        assert_eq!(count_buffs(&one), 1);

        let mut three = [0i32; MAX_BUFFS];
        three[0] = 1;
        three[1] = 2;
        three[2] = 3;
        assert_eq!(count_buffs(&three), 3);

        // The quirk: a GAP stops the count, so two buffs with a hole between them
        // count as one. `Player.cs:5491` indexes buffType[num], not buffType[i].
        let mut gapped = [0i32; MAX_BUFFS];
        gapped[0] = 1;
        gapped[2] = 3;
        assert_eq!(count_buffs(&gapped), 1, "a gap stops the C# count");
    }

    /// `DelBuff` clears the slot and compacts, leaving no hole behind it.
    #[test]
    fn del_buff_clears_and_compacts() {
        let mut types = [0i32; MAX_BUFFS];
        let mut times = [0i32; MAX_BUFFS];
        for (i, (t, tm)) in [(1, 100), (2, 200), (3, 300)].iter().enumerate() {
            types[i] = *t;
            times[i] = *tm;
        }
        del_buff(&mut types, &mut times, 1);
        assert_eq!(&types[..4], &[1, 3, 0, 0], "the third buff moved down into the hole");
        assert_eq!(&times[..4], &[100, 300, 0, 0]);
        assert_eq!(count_buffs(&types), 2);
    }

    /// The last slot is never a compaction source, because the C# loop is
    /// `i < maxBuffs - 1`. Pinned so changing it has to be a decision.
    #[test]
    fn del_buff_never_compacts_the_last_slot() {
        let mut types = [0i32; MAX_BUFFS];
        let mut times = [0i32; MAX_BUFFS];
        types[MAX_BUFFS - 1] = 9;
        times[MAX_BUFFS - 1] = 900;
        del_buff(&mut types, &mut times, 0);
        assert_eq!(types[MAX_BUFFS - 1], 9, "the C# leaves the last slot alone");
        assert_eq!(times[MAX_BUFFS - 1], 900);
    }

    /// `ClearBuff` removes every slot of one type, and is a no-op for a type that is
    /// not held.
    #[test]
    fn clear_buff_removes_the_whole_type() {
        let mut types = [0i32; MAX_BUFFS];
        let mut times = [0i32; MAX_BUFFS];
        types[0] = 7;
        times[0] = 10;
        types[1] = 8;
        times[1] = 20;
        clear_buff(&mut types, &mut times, 99);
        assert_eq!(count_buffs(&types), 2, "a type nobody holds changes nothing");
        clear_buff(&mut types, &mut times, 7);
        assert_eq!(count_buffs(&types), 1);
        assert_eq!(types[0], 8, "the survivor compacted down");
        assert_eq!(times[0], 20);
    }

    /// The max is floored and the current value is not clamped to it.
    #[test]
    fn life_max_is_floored_at_twenty_and_life_is_not_clamped() {
        let normal = sanitise_life(100, 120);
        assert_eq!(normal, LifeMana { stat_life: 100, stat_life_max: 120, dead: false });

        let floored = sanitise_life(100, 5);
        assert_eq!(floored.stat_life_max, MIN_LIFE_MAX as i32);
        assert_eq!(floored.stat_life, 100, "the current value is not clamped to the max");

        let at_bound = sanitise_life(20, 20);
        assert_eq!(at_bound.stat_life_max, 20, "exactly the floor is left alone");

        let above = sanitise_life(1, 9999);
        assert_eq!(above.stat_life_max, 9999);
    }

    /// `dead` is derived, so it cannot disagree with the life value.
    #[test]
    fn dead_is_derived_from_life() {
        assert!(!sanitise_life(1, 100).dead);
        assert!(sanitise_life(0, 100).dead, "zero life is dead");
        assert!(sanitise_life(-5, 100).dead, "negative life is dead");
        assert_eq!(sanitise_life(-5, 100).stat_life, -5, "the value is stored, not fixed");
        assert!(!sanitise_life(i16::MAX, 100).dead);
    }

    /// Immunity wins over presence, and an expired slot is invisible.
    #[test]
    fn find_buff_index_respects_immunity_and_time() {
        let mut immune = [false; 16];
        let mut types = [0i32; MAX_BUFFS];
        let mut times = [0i32; MAX_BUFFS];
        types[3] = 12;
        times[3] = 600;
        assert_eq!(find_buff_index(&immune, &types, &times, 12), 3);
        assert_eq!(find_buff_index(&immune, &types, &times, 99), -1, "absent");

        // Immune: present, and still not found.
        immune[12] = true;
        assert_eq!(find_buff_index(&immune, &types, &times, 12), -1, "immune wins");
        immune[12] = false;

        // Expired: the type is still set, the time is not, so it is not held.
        times[3] = 0;
        assert_eq!(find_buff_index(&immune, &types, &times, 12), -1, "expired is invisible");
        times[3] = 1;
        assert_eq!(find_buff_index(&immune, &types, &times, 12), 3, "one tick left still counts");

        // Outside the immunity array is not immune: the C# would throw, this does not.
        let empty_immune: [bool; 0] = [];
        assert_eq!(find_buff_index(&empty_immune, &types, &times, 12), 3);
    }

    /// The cost truncates rather than rounds, which is what keeps the client's
    /// prediction and the server's deduction in step.
    #[test]
    fn mana_cost_truncates() {
        assert_eq!(mana_cost(10, 1.0), 10);
        assert_eq!(mana_cost(10, 0.5), 5);
        assert_eq!(mana_cost(7, 0.5), 3, "3.5 truncates to 3, it does not round");
        assert_eq!(mana_cost(1, 0.5), 0, "0.5 truncates to 0");
        assert_eq!(mana_cost(0, 1.0), 0);
        assert_eq!(mana_cost(100, 1.0), 100);
    }

    /// Affordability, and the deduction only happening when the cost is affordable.
    #[test]
    fn check_mana_pays_only_when_affordable() {
        assert_eq!(check_mana(100, 10, 1.0, false), (true, 100), "a check does not pay");
        assert_eq!(check_mana(100, 10, 1.0, true), (true, 90));
        assert_eq!(check_mana(10, 10, 1.0, true), (true, 0), "exactly enough is affordable");
        assert_eq!(check_mana(9, 10, 1.0, true), (false, 9), "not enough, and nothing is deducted");
        assert_eq!(check_mana(0, 10, 1.0, false), (false, 0));
        // A free spell is always affordable, at any mana.
        assert_eq!(check_mana(0, 50, 0.0, true), (true, 0));
    }

    /// A plain player, so the golden bytes below are readable.
    fn sample() -> SyncPlayer<'static> {
        SyncPlayer {
            slot: 0,
            skin_variant: 11,
            voice_variant: 2,
            voice_pitch_offset: 0.5,
            haircut: 3,
            name: "Bob",
            hair_dye: 5,
            hide_visible_accessory: &[true, false, false, true],
            hide_misc: 0,
            colors: [
                (1, 2, 3),
                (4, 5, 6),
                (7, 8, 9),
                (10, 11, 12),
                (13, 14, 15),
                (16, 17, 18),
                (19, 20, 21),
            ],
            difficulty: 3,
            extra_accessory: true,
            using_biome_torches: true,
            happy_fun_torch_time: false,
            unlocked_biome_torches: true,
            unlocked_super_cart: false,
            enabled_super_cart: true,
            used_aegis_crystal: true,
            used_aegis_fruit: false,
            used_arcane_crystal: false,
            used_galaxy_pearl: false,
            used_gummy_worm: false,
            used_ambrosia: false,
            ate_artisan_bread: true,
        }
    }

    /// The exact bytes of `SyncPlayer` (4) for `sample()`, transcribed from
    /// `NetMessage.cs:156-206`. Both the writer test and the reader test use THIS, so
    /// the two halves are pinned to one vector rather than to each other: a round trip
    /// would agree with itself even when both sides share a mistake, which is exactly
    /// how the hairDye field was written as a string by mistake earlier in this file.
    #[rustfmt::skip]
    fn golden() -> Vec<u8> {
        vec![
            0,                                  // slot
            11,                                 // skinVariant
            2,                                  // voiceVariant
            0x00, 0x00, 0x00, 0x3F,             // voicePitchOffset 0.5f, little-endian
            3,                                  // hair
            3, b'B', b'o', b'b',                // name: 7-bit length, then UTF-8
            5,                                  // hairDye: ONE byte, not a string
            0x09, 0x00,                         // accessory visibility: bits 0 and 3
            0,                                  // hideMisc
            1, 2, 3,                            // hairColor
            4, 5, 6,                            // skinColor
            7, 8, 9,                            // eyeColor
            10, 11, 12,                         // shirtColor
            13, 14, 15,                         // underShirtColor
            16, 17, 18,                         // pantsColor
            19, 20, 21,                         // shoeColor
            0x0C,                               // difficulty: Journey = bit 3, plus extraAccessory bit 2
            0x15,                               // biome torches: bits 0, 2 and 4
            0x41,                               // crystals: AegisCrystal bit 0, ArtisanBread bit 6
        ]
    }

    /// The strongest write-side test available without a client: the exact bytes.
    #[test]
    fn sync_player_writes_the_c_sharp_layout() {
        let mut out = Vec::new();
        write_sync_player(&mut out, &sample()).unwrap();
        assert_eq!(out, golden());
        assert_eq!(out.len(), 3 + 4 + 1 + 4 + 1 + 2 + 1 + 21 + 3);
    }

    /// The reader, on the same bytes the writer must produce.
    #[test]
    fn sync_player_reads_the_c_sharp_layout() {
        let p = read_sync_player(&golden()).expect("the golden bytes must parse");
        assert_eq!(p.slot, 0);
        assert_eq!(p.skin_variant, 11);
        assert_eq!(p.voice_variant, 2);
        assert_eq!(p.voice_pitch_offset, 0.5);
        assert_eq!(p.haircut, 3);
        assert_eq!(p.name, "Bob");
        assert_eq!(p.hair_dye, 5, "one byte, read as a byte");
        assert!(p.hide_visible_accessory[0], "bit 0 is set");
        assert!(!p.hide_visible_accessory[1]);
        assert!(!p.hide_visible_accessory[2]);
        assert!(p.hide_visible_accessory[3]);
        assert_eq!(p.hide_misc, 0);
        assert_eq!(p.colors[0], (1, 2, 3));
        assert_eq!(p.colors[6], (19, 20, 21));
        assert_eq!(p.difficulty, 3, "Journey is bit 3");
        assert!(p.extra_accessory, "bit 2");
        assert!(p.using_biome_torches);
        assert!(!p.happy_fun_torch_time);
        assert!(p.unlocked_biome_torches);
        assert!(!p.unlocked_super_cart);
        assert!(p.enabled_super_cart);
        assert!(p.used_aegis_crystal);
        assert!(!p.used_aegis_fruit);
        assert!(!p.used_ambrosia);
        assert!(p.ate_artisan_bread);
    }

    /// Round trip: what the writer produced, the reader accepts, field for field.
    #[test]
    fn sync_player_survives_a_round_trip() {
        let mut out = Vec::new();
        let sent = sample();
        write_sync_player(&mut out, &sent).unwrap();
        let got = read_sync_player(&out).unwrap();
        assert_eq!(got.skin_variant, sent.skin_variant as i32);
        assert_eq!(got.voice_variant, sent.voice_variant as i32);
        assert_eq!(got.voice_pitch_offset, sent.voice_pitch_offset);
        assert_eq!(got.haircut, sent.haircut as i32);
        assert_eq!(got.name, sent.name);
        assert_eq!(got.hair_dye, sent.hair_dye);
        assert_eq!(got.colors, sent.colors);
        assert_eq!(got.difficulty, sent.difficulty);
        assert_eq!(got.extra_accessory, sent.extra_accessory);
        assert_eq!(got.hide_visible_accessory[0], sent.hide_visible_accessory[0]);
        assert_eq!(got.hide_visible_accessory[3], sent.hide_visible_accessory[3]);
    }

    /// Everything a client could lie about, sanitised on the way in.
    #[test]
    fn the_reader_is_stricter_than_the_writer() {
        let mut bytes = golden();
        // Offsets into the golden vector: 0 slot, 1 skin, 2 voice, 3..7 pitch, 7 hair.
        bytes[1] = 255; // skinVariant, out of range
        bytes[2] = 0; // voiceVariant, below the minimum of 1
        bytes[3..7].copy_from_slice(&f32::NAN.to_le_bytes()); // pitch: NaN
        bytes[7] = 228; // hair, at the limit
        let p = read_sync_player(&bytes).unwrap();
        assert_eq!(p.skin_variant, MAX_SKIN_VARIANT, "clamped to the id table's highest");
        assert_eq!(p.voice_variant, 1, "raised to the minimum");
        assert_eq!(p.voice_pitch_offset, 0.0, "a NaN pitch becomes zero");
        assert!(p.voice_pitch_offset.is_sign_positive());
        assert_eq!(p.haircut, 0, "at or past MAX_HAIR becomes 0, not the maximum");

        // A pitch outside the range is clamped on the way in.
        assert_eq!(clamp_received_pitch(5.0), 1.0);
        assert_eq!(clamp_received_pitch(-5.0), -1.0);
        assert_eq!(clamp_received_pitch(0.25), 0.25);
        assert_eq!(clamp_received_pitch(f32::NAN), 0.0);
    }

    /// Names the C# refuses, and a truncated body.
    #[test]
    fn the_reader_refuses_bad_names_and_short_bodies() {
        let long = "x".repeat(NAME_LEN + 1);
        let mut bytes = Vec::new();
        let mut p = sample();
        p.name = &long;
        write_sync_player(&mut bytes, &p).unwrap();
        assert_eq!(read_sync_player(&bytes), Err(SyncPlayerError::NameTooLong));

        // Exactly the limit is allowed.
        let at_limit = "x".repeat(NAME_LEN);
        let mut bytes = Vec::new();
        p.name = &at_limit;
        write_sync_player(&mut bytes, &p).unwrap();
        assert_eq!(read_sync_player(&bytes).unwrap().name.len(), NAME_LEN);

        // Whitespace is trimmed, and a name of only whitespace is empty.
        let mut bytes = Vec::new();
        p.name = "  Bob  ";
        write_sync_player(&mut bytes, &p).unwrap();
        assert_eq!(read_sync_player(&bytes).unwrap().name, "Bob");

        let mut bytes = Vec::new();
        p.name = "   ";
        write_sync_player(&mut bytes, &p).unwrap();
        assert_eq!(read_sync_player(&bytes), Err(SyncPlayerError::EmptyName));

        // A body that stops early is TooShort, never a partial player.
        assert_eq!(read_sync_player(&[]), Err(SyncPlayerError::TooShort));
        assert_eq!(read_sync_player(&golden()[..10]), Err(SyncPlayerError::TooShort));
        assert_eq!(read_sync_player(&golden()[..39]), Err(SyncPlayerError::TooShort));
    }

    /// The refusal keys come from the book, so a kick cannot name a row that is not
    /// there, and the framing failure deliberately has no key.
    #[test]
    fn the_refusal_keys_are_rows() {
        assert_eq!(SyncPlayerError::NameTooLong.key(), Some("Net.NameTooLong"));
        assert_eq!(SyncPlayerError::EmptyName.key(), Some("Net.EmptyName"));
        assert_eq!(SyncPlayerError::TooShort.key(), None);
        // The key is a lookup, so the text it names is one too.
        assert_eq!(
            crate::sheets::localization::by_id("net.nametoolong").unwrap().text,
            "Name is too long."
        );
    }

    /// Journey is bit 3, not bit 2, and Normal sets nothing.
    #[test]
    fn difficulty_bits_leave_the_gap_at_bit_two() {
        assert_eq!(difficulty_bits(0, false), 0b0000_0000, "Normal is all zeros");
        assert_eq!(difficulty_bits(1, false), 0b0000_0001);
        assert_eq!(difficulty_bits(2, false), 0b0000_0010);
        assert_eq!(difficulty_bits(3, false), 0b0000_1000, "Journey is bit 3");
        assert_eq!(difficulty_bits(0, true), 0b0000_0100, "bit 2 is extraAccessory");
        assert_eq!(difficulty_bits(3, true), 0b0000_1100);
        // An unknown difficulty sets no difficulty bit but still reports the slot.
        assert_eq!(difficulty_bits(99, true), 0b0000_0100);
    }

    /// The visibility field is one bit per index, little-endian on the wire.
    #[test]
    fn accessory_visibility_packs_one_bit_per_index() {
        assert_eq!(accessory_visibility(&[]), 0);
        assert_eq!(accessory_visibility(&[false; 14]), 0);
        assert_eq!(accessory_visibility(&[true]), 1);
        assert_eq!(accessory_visibility(&[false, true]), 2);
        let all: Vec<bool> = vec![true; 16];
        assert_eq!(accessory_visibility(&all), u16::MAX);
        // At or beyond 16 the C# has already lost the bit into a ushort.
        let mut long = vec![false; 20];
        long[16] = true;
        long[19] = true;
        assert_eq!(accessory_visibility(&long), 0, "indices past 15 contribute nothing");
    }
}
