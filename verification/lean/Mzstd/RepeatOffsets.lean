import Mzstd.Tables

/-! # Repeat offsets

The encoder carries a sequence's offset as the reference's "offBase": `1..=3`
name a repeated offset and a literal offset is stored plus 3. That is RFC 8878's
`Offset_Value` exactly (section 3.1.1.3.2.1.1), so `update_rep`
(`compressor/core/seq_store.rs`), which the encoder runs after every sequence to
track the decoder's three repeated offsets, can be compared with section
3.1.1.5 directly: it performs the RFC's update for every offset value and
literals length.
-/

open Aeneas Aeneas.Std Result
open Mzstd.Rfc

namespace mzstd
open compressor.core.seq_store

/-- The three repeated offsets the encoder's array holds, most recent first. -/
def repeats (rep : Std.Array U32 3#usize) : Repeats := ⟨rep[0]!.val, rep[1]!.val, rep[2]!.val⟩

/-- **Spec theorem for `mzstd::compressor::core::seq_store::offset_to_offbase`**
A literal offset becomes the `Offset_Value` `offset + 3`. -/
@[step]
theorem offset_to_offbase.spec (offset : U32) (h : offset.val + 3 < 2 ^ 32) :
    offset_to_offbase offset ⦃ (r : U32) => r.val = offset.val + 3 ∧ Mzstd.Rfc.offset r.val = offset.val ⦄ := by
  unfold offset_to_offbase
  step*
  simp only [Mzstd.Rfc.offset]; agrind

/-- **Spec theorem for `mzstd::compressor::core::seq_store::repcode_to_offbase`**
Repeat code `k` is the `Offset_Value` `k`. -/
@[step]
theorem repcode_to_offbase.spec (repcode : U32) :
    repcode_to_offbase repcode ⦃ (r : U32) => r = repcode ⦄ := by
  unfold repcode_to_offbase; simp

/-- **Spec theorem for `mzstd::compressor::core::seq_store::offbase_is_repcode`**
Values 1 to 3 are the RFC's "repeat codes". -/
@[step]
theorem offbase_is_repcode.spec (off_base : U32) :
    offbase_is_repcode off_base ⦃ (r : Bool) => r = decide (1 ≤ off_base.val ∧ off_base.val ≤ 3) ⦄ := by
  unfold offbase_is_repcode
  step*

/-- **Spec theorem for `mzstd::compressor::core::seq_store::offbase_to_offset`**
"if (Offset_Value > 3) Offset = Offset_Value - 3". -/
@[step]
theorem offbase_to_offset.spec (off_base : U32) (h : 3 ≤ off_base.val) :
    offbase_to_offset off_base ⦃ (r : U32) => r.val = Mzstd.Rfc.offset off_base.val ⦄ := by
  unfold offbase_to_offset
  step*
  simp only [Mzstd.Rfc.offset]; agrind

/-- **Spec theorem for `mzstd::compressor::core::seq_store::update_rep`**
After a sequence with `Offset_Value` `off_base` and a literals length that is
zero exactly when `ll0`, the encoder's repeated offsets are the ones RFC 8878
section 3.1.1.5 prescribes. The only excluded input is `Repeated_Offset1 - 1`
with `Repeated_Offset1 = 0`, which a stream never reaches: the starting offsets
are positive and every offset the format writes is. -/
@[step]
theorem update_rep.spec (rep : Std.Array U32 3#usize) (off_base : U32) (ll0 : Bool)
    (h1 : 1 ≤ off_base.val) (hpos : ll0 = true → off_base.val = 3 → 1 ≤ rep[0]!.val) :
    update_rep rep off_base ll0 ⦃ (r : Std.Array U32 3#usize) =>
      repeats r = updateRepeats (repeats rep) off_base.val ll0 ⦄ := by
  unfold update_rep
  step as ⟨ i, i_post ⟩
  have hi : i.val = 3 := by rw [i_post]; simp
  have two_mod : 2 % 2 ^ System.Platform.numBits = 2 := Nat.mod_eq_of_lt (by
    have := numBits_ge
    calc 2 < 2 ^ 32 := by norm_num
      _ ≤ _ := Nat.pow_le_pow_right (by decide) this)
  split
  · have hgt : 3 < off_base.val := by scalar_tac
    step*
    simp_all [repeats, updateRepeats, usedOffset, Mzstd.Rfc.offset]
  · have hcases : off_base.val = 1 ∨ off_base.val = 2 ∨ off_base.val = 3 := by scalar_tac
    -- Six cases: the offset value and whether the literals length is zero.
    -- Each runs the Rust branches to the end; contradictory branches close
    -- by arithmetic, and the rest compare entry by entry with the RFC. The
    -- one subtraction, `Repeated_Offset1 - 1`, is wrapping in Rust and exact
    -- here because `hpos` keeps it from going below zero.
    cases ll0 <;> rcases hcases with h | h | h <;> step* <;> (repeat' (first | split | step)) <;>
      (try scalar_tac) <;>
      simp_all [repeats, updateRepeats, usedOffset, Array.getElem!_Nat_eq, UScalar.cast_val_eq]
    scalar_tac

end mzstd
