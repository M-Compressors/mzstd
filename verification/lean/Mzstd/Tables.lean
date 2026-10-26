import Mzstd.Basic

/-! # The format constants and code tables are the RFC's

The literal length and match length code tables of `shared/format.rs` and the
decoder's baseline tables in `decompressor/core/sequences.rs`, the offset code
tables, the default distributions, the initial repeat offsets, the magic
numbers and the two-bit mode fields all equal what RFC 8878 prints. The
baseline tables are computed by loops in the Rust code; their specifications
prove the loops compute the running sum the RFC tables list.
-/

open Aeneas Aeneas.Std Result
open Mzstd.Rfc

namespace mzstd
open decompressor.core.sequences

/-! ## Baselines -/

/-- The baseline of code `c` in a table that starts at `first` and gives each
code `2 ^ bits[c]` values. -/
def partialBase (bits : List U8) (first c : Nat) : Nat :=
  first + ((bits.take c).map (fun b => 2 ^ b.val)).sum

theorem partialBase_succ (bits : List U8) (first c : Nat) (h : c < bits.length) :
    partialBase bits first (c + 1) = partialBase bits first c + 2 ^ bits[c]!.val := by
  simp only [partialBase, List.take_add_one, List.map_append, List.sum_append]
  rw [List.getElem?_eq_getElem h]
  simp [List.getElem!_eq_getElem?_getD, List.getElem?_eq_getElem h, Nat.add_assoc]

theorem partialBase_mono (bits : List U8) (first c d : Nat) (h : c ≤ d) :
    partialBase bits first c ≤ partialBase bits first d := by
  simp only [partialBase, Nat.add_le_add_iff_left]
  obtain ⟨k, rfl⟩ := Nat.exists_eq_add_of_le h
  rw [List.take_add, List.map_append, List.sum_append]
  exact Nat.le_add_right _ _

@[step]
theorem baselines_loop.spec {N : Usize} (bits : Array U8 N) (base : Array U32 N) (value : U32)
    (code : Usize) (first : Nat) (hcode : code.val ≤ N.val)
    (hvalue : value.val = partialBase bits.val first code.val)
    (htotal : partialBase bits.val first N.val < 2 ^ 32)
    (hbits : ∀ i < N.val, bits[i]!.val < 32)
    (hdone : ∀ j < code.val, base[j]!.val = partialBase bits.val first j) :
    baselines_loop bits base value code ⦃ (r : Array U32 N) =>
      ∀ j < N.val, r[j]!.val = partialBase bits.val first j ⦄ := by
  unfold baselines_loop
  split
  · have hlen : code.val < bits.val.length := by scalar_tac
    step as ⟨ a, a_post ⟩
    step as ⟨ i, i_post ⟩
    have hi : i.val < 32 := by
      rw [i_post]; have := hbits code.val (by scalar_tac)
      simpa [List.getElem!_eq_getElem?_getD, List.getElem?_eq_getElem hlen] using this
    step as ⟨ i1, i1_post ⟩
    have hi1 : i1.val = 2 ^ bits[code.val]!.val := by
      have : 2 ^ i.val < 2 ^ 32 := Nat.pow_lt_pow_right (by decide) hi
      rw [i1_post, Nat.one_shiftLeft, Nat.mod_eq_of_lt (by scalar_tac), i_post]
      simp [Array.getElem!_Nat_eq, List.getElem!_eq_getElem?_getD, List.getElem?_eq_getElem hlen]
    have hnext := partialBase_succ bits.val first code.val hlen
    have hle := partialBase_mono bits.val first (code.val + 1) N.val (by scalar_tac)
    step as ⟨ value1, value1_post ⟩
    step as ⟨ code1, code1_post ⟩
    apply baselines_loop.spec bits a value1 code1 first (by scalar_tac) (by agrind) htotal hbits
    intro j hj
    rw [a_post]
    by_cases hjc : j = code.val
    · rw [Array.getElem!_Nat_set_eq _ _ _ _ ⟨hjc.symm, by scalar_tac⟩, hjc, hvalue]
    · rw [Array.getElem!_Nat_set_ne _ _ _ _ (Ne.symm hjc)]
      exact hdone j (by scalar_tac)
  · have : code.val = N.val := by scalar_tac
    simp only [WP.spec_ok]
    intro j hj
    exact hdone j (by rw [this]; exact hj)
termination_by N.val - code.val
decreasing_by scalar_decr_tac

theorem LL_BITS_rfc :
    ∀ c < 36, shared.format.LL_BITS[c]!.val = (code literalsLengthCodes c).bits := by
  rw [shared.format.LL_BITS]
  decide +kernel

theorem ML_BITS_rfc :
    ∀ c < 53, shared.format.ML_BITS[c]!.val = (code matchLengthCodes c).bits := by
  rw [shared.format.ML_BITS]
  decide +kernel

theorem LL_partialBase_rfc :
    ∀ c < 36, partialBase shared.format.LL_BITS.val 0 c = (code literalsLengthCodes c).baseline := by
  rw [shared.format.LL_BITS]
  decide +kernel

theorem ML_partialBase_rfc :
    ∀ c < 53, partialBase shared.format.ML_BITS.val 3 c = (code matchLengthCodes c).baseline := by
  rw [shared.format.ML_BITS]
  decide +kernel

theorem LL_BITS_lt : ∀ c < 36, shared.format.LL_BITS[c]!.val < 32 := by
  rw [shared.format.LL_BITS]
  decide +kernel

theorem ML_BITS_lt : ∀ c < 53, shared.format.ML_BITS[c]!.val < 32 := by
  rw [shared.format.ML_BITS]
  decide +kernel

theorem LL_total : partialBase shared.format.LL_BITS.val 0 36 < 2 ^ 32 := by
  rw [shared.format.LL_BITS]
  decide +kernel

theorem ML_total : partialBase shared.format.ML_BITS.val 3 53 < 2 ^ 32 := by
  rw [shared.format.ML_BITS]
  decide +kernel

/-- **Spec theorem for `mzstd::decompressor::core::sequences::LL_BASE`**
Entry `c` is the `Baseline` of literals length code `c` (RFC 8878, Table 16). -/
@[step]
theorem LL_BASE.spec :
    LL_BASE ⦃ (t : Array U32 36#usize) =>
      ∀ c < 36, t[c]!.val = (code literalsLengthCodes c).baseline ⦄ := by
  unfold LL_BASE baselines
  apply WP.spec_mono (baselines_loop.spec _ _ _ _ 0 (by scalar_tac) (by simp [partialBase])
    LL_total LL_BITS_lt (fun _ h => absurd h (by scalar_tac)))
  intro t ht c hc
  rw [ht c hc, LL_partialBase_rfc c hc]

/-- **Spec theorem for `mzstd::decompressor::core::sequences::ML_BASE`**
Entry `c` is the `Baseline` of match length code `c` (RFC 8878, Table 17). -/
@[step]
theorem ML_BASE.spec :
    ML_BASE ⦃ (t : Array U32 53#usize) =>
      ∀ c < 53, t[c]!.val = (code matchLengthCodes c).baseline ⦄ := by
  unfold ML_BASE baselines
  step as ⟨ i, i_post ⟩
  apply WP.spec_mono (baselines_loop.spec _ _ _ _ 3 (by scalar_tac)
    (by rw [i_post]; simp [partialBase]) ML_total ML_BITS_lt (fun _ h => absurd h (by scalar_tac)))
  intro t ht c hc
  rw [ht c hc, ML_partialBase_rfc c hc]

/-! ## Offset codes -/

@[step]
theorem OF_BITS_loop.spec (bits : Array U8 32#usize) (code : Usize) (hcode : code.val ≤ 32)
    (hdone : ∀ j < code.val, bits[j]!.val = j) :
    OF_BITS_loop bits code ⦃ (r : Array U8 32#usize) => ∀ j < 32, r[j]!.val = j ⦄ := by
  unfold OF_BITS_loop
  step as ⟨ s, s_post ⟩
  split
  · step as ⟨ i1, i1_post ⟩
    step as ⟨ a, a_post ⟩
    step as ⟨ code1, code1_post ⟩
    apply OF_BITS_loop.spec a code1 (by scalar_tac)
    intro j hj
    rw [a_post]
    by_cases hjc : j = code.val
    · rw [Array.getElem!_Nat_set_eq _ _ _ _ ⟨hjc.symm, by scalar_tac⟩, hjc, i1_post,
        UScalar.cast_val_eq, Nat.mod_eq_of_lt (by scalar_tac)]
    · rw [Array.getElem!_Nat_set_ne _ _ _ _ (Ne.symm hjc)]
      exact hdone j (by scalar_tac)
  · simp only [WP.spec_ok]
    intro j hj
    exact hdone j (by scalar_tac)
termination_by 32 - code.val
decreasing_by scalar_decr_tac

/-- **Spec theorem for `mzstd::decompressor::core::sequences::OF_BITS`**
"An offset code is also the number of additional bits to read" (RFC 8878,
section 3.1.1.3.2.1.1). -/
@[step]
theorem OF_BITS.spec : OF_BITS ⦃ (t : Array U8 32#usize) => ∀ c < 32, t[c]!.val = c ⦄ := by
  unfold OF_BITS
  exact OF_BITS_loop.spec _ _ (by scalar_tac) (fun _ h => absurd h (by scalar_tac))

@[step]
theorem OF_BASE_loop.spec (base : Array U32 32#usize) (code : Usize) (h2 : 2 ≤ code.val)
    (hcode : code.val ≤ 32) (h01 : base[0]!.val = 0 ∧ base[1]!.val = 1)
    (hdone : ∀ j < code.val, 2 ≤ j → base[j]!.val + 3 = 2 ^ j) :
    OF_BASE_loop base code ⦃ (r : Array U32 32#usize) =>
      r[0]!.val = 0 ∧ r[1]!.val = 1 ∧ ∀ j < 32, 2 ≤ j → r[j]!.val + 3 = 2 ^ j ⦄ := by
  unfold OF_BASE_loop
  step as ⟨ s, s_post ⟩
  split
  · have hc : code.val < 32 := by scalar_tac
    step as ⟨ i1, i1_post ⟩
    have hi1 : i1.val = 2 ^ code.val := by
      rw [i1_post, Nat.one_shiftLeft, Nat.mod_eq_of_lt (by
        have : 2 ^ code.val < 2 ^ 32 := Nat.pow_lt_pow_right (by decide) hc
        scalar_tac)]
    have h4 : 4 ≤ 2 ^ code.val := by
      calc 4 = 2 ^ 2 := by norm_num
        _ ≤ 2 ^ code.val := Nat.pow_le_pow_right (by decide) h2
    step as ⟨ i2, i2_post ⟩
    step as ⟨ a, a_post ⟩
    step as ⟨ code1, code1_post ⟩
    apply OF_BASE_loop.spec a code1 (by scalar_tac) (by scalar_tac)
    · rw [a_post, Array.getElem!_Nat_set_ne _ _ _ _ (by scalar_tac),
        Array.getElem!_Nat_set_ne _ _ _ _ (by scalar_tac)]
      exact h01
    · intro j hj hj2
      rw [a_post]
      by_cases hjc : j = code.val
      · rw [Array.getElem!_Nat_set_eq _ _ _ _ ⟨hjc.symm, by scalar_tac⟩, hjc, i2_post, hi1]
        omega
      · rw [Array.getElem!_Nat_set_ne _ _ _ _ (Ne.symm hjc)]
        exact hdone j (by scalar_tac) hj2
  · simp only [WP.spec_ok]
    exact ⟨h01.1, h01.2, fun j hj hj2 => hdone j (by scalar_tac) hj2⟩
termination_by 32 - code.val
decreasing_by scalar_decr_tac

/-- **Spec theorem for `mzstd::decompressor::core::sequences::OF_BASE`**
For an offset code `c ≥ 2` the decoder adds the extra field to `OF_BASE[c]`,
which is `2 ^ c - 3`: the offset `Offset_Value - 3` of RFC 8878 section
3.1.1.3.2.1.1. Codes 0 and 1 keep `0` and `1`, which the repeat offset logic
reads. -/
@[step]
theorem OF_BASE.spec :
    OF_BASE ⦃ (t : Array U32 32#usize) =>
      t[0]!.val = 0 ∧ t[1]!.val = 1 ∧
      ∀ c < 32, 2 ≤ c → ∀ extra, t[c]!.val + extra = offset (offsetValue c extra) ⦄ := by
  unfold OF_BASE
  step as ⟨ a, a_post ⟩
  apply WP.spec_mono (OF_BASE_loop.spec a 2#usize (by scalar_tac) (by scalar_tac) ?_ ?_)
  · rintro t ⟨h0, h1, h⟩
    refine ⟨h0, h1, fun c hc hc2 extra => ?_⟩
    have := h c hc hc2
    simp only [offset, offsetValue]
    omega
  · rw [a_post]
    simp
  · intro j hj hj2; scalar_tac

/-! ## Default distributions -/

theorem LL_DEFAULT_NORM_rfc :
    shared.format.LL_DEFAULT_NORM.val.map (fun x => x.val) = literalsLengthDefault.probabilities ∧
      shared.format.LL_DEFAULT_NORM_LOG.val = literalsLengthDefault.accuracyLog := by
  rw [shared.format.LL_DEFAULT_NORM, shared.format.LL_DEFAULT_NORM_LOG]
  decide +kernel

theorem ML_DEFAULT_NORM_rfc :
    shared.format.ML_DEFAULT_NORM.val.map (fun x => x.val) = matchLengthDefault.probabilities ∧
      shared.format.ML_DEFAULT_NORM_LOG.val = matchLengthDefault.accuracyLog := by
  rw [shared.format.ML_DEFAULT_NORM, shared.format.ML_DEFAULT_NORM_LOG]
  decide +kernel

theorem OF_DEFAULT_NORM_rfc :
    shared.format.OF_DEFAULT_NORM.val.map (fun x => x.val) = offsetDefault.probabilities ∧
      shared.format.OF_DEFAULT_NORM_LOG.val = offsetDefault.accuracyLog := by
  rw [shared.format.OF_DEFAULT_NORM, shared.format.OF_DEFAULT_NORM_LOG]
  decide +kernel

/-! ## Constants -/

/-- The initial repeat offsets are `1, 4, 8` (RFC 8878, section 3.1.1.5). -/
theorem REP_START_rfc :
    shared.format.REP_START.val.map (fun x => x.val) =
      [initialRepeats.r1, initialRepeats.r2, initialRepeats.r3] := by
  rw [shared.format.REP_START]
  decide +kernel

/-- The frame magic number and the first skippable one (sections 3.1.1 and
3.1.2); the mask keeps all but the low four bits, the sixteen values of the
skippable family. -/
theorem magic_rfc :
    shared.format.MAGIC_NUMBER.val = magicNumber ∧
      shared.format.MAGIC_SKIPPABLE_START.val = 0x184D2A50 ∧
      shared.format.MAGIC_SKIPPABLE_MASK.val = 2 ^ 32 - 16 := by
  simp [shared.format.MAGIC_NUMBER, shared.format.MAGIC_SKIPPABLE_START,
    shared.format.MAGIC_SKIPPABLE_MASK, magicNumber]

/-! ## Two-bit mode fields -/

/-- The compression mode each `EncodingType` names. -/
def shared.format.EncodingType.mode : shared.format.EncodingType → CompressionMode
  | .Basic => .predefined
  | .Rle => .rle
  | .Compressed => .fseCompressed
  | .Repeat => .repeat

/-- **Spec theorem for `mzstd::decompressor::core::literals::encoding_type`**
The two low bits name the mode of Table 15 (and the literals block type of
section 3.1.1.3.1.1, which uses the same values). -/
@[step]
theorem decompressor.core.literals.encoding_type.spec (bits : U8) :
    decompressor.core.literals.encoding_type bits ⦃ (r : shared.format.EncodingType) =>
      r.mode = compressionMode bits.val ⦄ := by
  unfold decompressor.core.literals.encoding_type
  step as ⟨ i, i_post ⟩
  have hi : i.val = bits.val % 4 := by
    rw [i_post, UScalar.val_and]; exact Nat.and_two_pow_sub_one_eq_mod _ 2
  split
  · change 0 = bits.val % 4 at hi; simp [compressionMode, ← hi, shared.format.EncodingType.mode]
  · change 1 = bits.val % 4 at hi; simp [compressionMode, ← hi, shared.format.EncodingType.mode]
  · change 2 = bits.val % 4 at hi; simp [compressionMode, ← hi, shared.format.EncodingType.mode]
  · rename_i h0 h1 h2
    have h3 : bits.val % 4 = 3 := by
      have n0 : i.val ≠ 0 := fun h => h0 (UScalar.eq_of_val_eq (h.trans rfl))
      have n1 : i.val ≠ 1 := fun h => h1 (UScalar.eq_of_val_eq (h.trans rfl))
      have n2 : i.val ≠ 2 := fun h => h2 (UScalar.eq_of_val_eq (h.trans rfl))
      omega
    simp [compressionMode, h3, shared.format.EncodingType.mode]

end mzstd
