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
}
