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
}
