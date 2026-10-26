import Mzstd.Tables

/-! # Literals length and match length codes

The encoder's `ll_code` and `ml_code` (`compressor/core/seq_store.rs`) pick, for
every length the format can carry, the code of RFC 8878 Tables 16 and 17 whose
range holds it. Because the RFC's ranges are contiguous, that code is the only
one that holds the length, so any encoder writes the same code. The decoder's
baseline tables then read the length back from the code and the extra field
the encoder writes, `length - Baseline`, which fits in `Number_of_Bits` bits.
-/

open Aeneas Aeneas.Std Result
open Mzstd.Rfc

namespace mzstd
open compressor.core.seq_store

/-! ## Uniqueness, from the RFC tables alone -/

/-- In a table whose ranges follow one another, at most one code holds a
length. -/
theorem holds_unique (table : List Code) (n : Nat)
    (hcont : ∀ c < n - 1, (code table c).baseline + 2 ^ (code table c).bits = (code table (c + 1)).baseline)
    (c c' length : Nat) (hc : c < n) (hc' : c' < n)
    (h : holds table c length) (h' : holds table c' length) : c = c' := by
  -- Baselines only grow past a code's range.
  have mono : ∀ a b, a < b → b < n →
      (code table a).baseline + 2 ^ (code table a).bits ≤ (code table b).baseline := by
    intro a b hab hb
    induction b with
    | zero => omega
    | succ b ih =>
      rcases Nat.lt_succ_iff_lt_or_eq.mp hab with hlt | heq
      · have := ih hlt (by omega)
        have := hcont b (by omega)
        have : 0 < 2 ^ (code table b).bits := Nat.two_pow_pos _
        omega
      · subst heq; exact le_of_eq (hcont a (by omega))
  unfold holds at h h'
  rcases Nat.lt_trichotomy c c' with hlt | heq | hgt
  · have := mono c c' hlt hc'; omega
  · exact heq
  · have := mono c' c hgt hc; omega

theorem literalsLength_unique (c c' length : Nat) (hc : c < 36) (hc' : c' < 36)
    (h : holds literalsLengthCodes c length) (h' : holds literalsLengthCodes c' length) : c = c' :=
  holds_unique literalsLengthCodes 36 literalsLengthCodes_contiguous c c' length hc hc' h h'

theorem matchLength_unique (c c' length : Nat) (hc : c < 53) (hc' : c' < 53)
    (h : holds matchLengthCodes c length) (h' : holds matchLengthCodes c' length) : c = c' :=
  holds_unique matchLengthCodes 53 matchLengthCodes_contiguous c c' length hc hc' h h'

/-! ## The encoder's codes -/

theorem LL_CODE_rfc :
    ∀ l < 64, ll_code.LL_CODE[l]!.val < 36 ∧ holds literalsLengthCodes ll_code.LL_CODE[l]!.val l := by
  rw [ll_code.LL_CODE]
  decide +kernel

theorem ML_CODE_rfc :
    ∀ l < 128, ml_code.ML_CODE[l]!.val < 53 ∧ holds matchLengthCodes ml_code.ML_CODE[l]!.val (l + 3) := by
  rw [ml_code.ML_CODE]
  decide +kernel

/-- Past code 24, literals length code `k + 19` is the power of two `2 ^ k`. -/
theorem literalsLength_pow (k : Nat) (h6 : 6 ≤ k) (h16 : k ≤ 16) :
    code literalsLengthCodes (k + 19) = ⟨2 ^ k, k⟩ := by
  revert k; decide

/-- Past code 42, match length code `k + 36` starts at `2 ^ k + 3`. -/
theorem matchLength_pow (k : Nat) (h7 : 7 ≤ k) (h16 : k ≤ 16) :
    code matchLengthCodes (k + 36) = ⟨2 ^ k + 3, k⟩ := by
  revert k; decide

theorem log_bounds (l : Nat) (h : l ≠ 0) : 2 ^ Nat.log 2 l ≤ l ∧ l < 2 ^ Nat.log 2 l * 2 := by
  refine ⟨Nat.pow_log_le_self 2 h, ?_⟩
  rw [← pow_succ]; exact Nat.lt_pow_succ_log_self (by decide) l

/-- **Spec theorem for `mzstd::compressor::core::seq_store::ll_code`**
For every literals length up to 131071, the code whose range in RFC 8878
Table 16 holds it. -/
@[step]
theorem ll_code.spec (lit_length : U32) (h : lit_length.val ≤ 131071) :
    ll_code lit_length ⦃ (c : U8) =>
      c.val < 36 ∧ holds literalsLengthCodes c.val lit_length.val ⦄ := by
  unfold ll_code
  split
  · have hz : lit_length.val ≠ 0 := by scalar_tac
    have hlog6 : 6 ≤ Nat.log 2 lit_length.val :=
      Nat.le_log_of_pow_le (by decide) (by scalar_tac)
    have hlog16 : Nat.log 2 lit_length.val ≤ 16 :=
      Nat.le_of_lt_succ (Nat.log_lt_of_lt_pow hz (by scalar_tac))
    step as ⟨ i, i_post ⟩
    step as ⟨ i1, i1_post ⟩
    have hc : (UScalar.cast .U8 i1).val = Nat.log 2 lit_length.val + 19 := by
      rw [UScalar.cast_val_eq, Nat.mod_eq_of_lt (by scalar_tac)]; agrind
    rw [hc]
    refine ⟨by omega, ?_⟩
    simp only [holds, literalsLength_pow _ hlog6 hlog16]
    have := log_bounds _ hz
    exact ⟨this.1, by omega⟩
  · step as ⟨ i, i_post ⟩
    step as ⟨ c, c_post ⟩
    have hl : lit_length.val < 64 := by scalar_tac
    have hi : i.val = lit_length.val := by
      rw [i_post, UScalar.cast_val_eq, Nat.mod_eq_of_lt (by scalar_tac)]
    have hc : c = ll_code.LL_CODE[lit_length.val]! := by
      subst c_post; simp [Array.getElem!_Nat_eq, hi, List.getElem!_eq_getElem?_getD,
        List.getElem?_eq_getElem (by scalar_tac : lit_length.val < ll_code.LL_CODE.val.length)]
    rw [hc]
    exact LL_CODE_rfc lit_length.val hl

/-- **Spec theorem for `mzstd::compressor::core::seq_store::ml_code`**
For every match length from 3 to 131074, passed as `ml_base = length - 3`,
the code whose range in RFC 8878 Table 17 holds the length. -/
@[step]
theorem ml_code.spec (ml_base : U32) (h : ml_base.val ≤ 131071) :
    ml_code ml_base ⦃ (c : U8) =>
      c.val < 53 ∧ holds matchLengthCodes c.val (ml_base.val + 3) ⦄ := by
  unfold ml_code
  split
  · have hz : ml_base.val ≠ 0 := by scalar_tac
    have hlog7 : 7 ≤ Nat.log 2 ml_base.val :=
      Nat.le_log_of_pow_le (by decide) (by scalar_tac)
    have hlog16 : Nat.log 2 ml_base.val ≤ 16 :=
      Nat.le_of_lt_succ (Nat.log_lt_of_lt_pow hz (by scalar_tac))
    step as ⟨ i, i_post ⟩
    step as ⟨ i1, i1_post ⟩
    have hc : (UScalar.cast .U8 i1).val = Nat.log 2 ml_base.val + 36 := by
      rw [UScalar.cast_val_eq, Nat.mod_eq_of_lt (by scalar_tac)]; agrind
    rw [hc]
    refine ⟨by omega, ?_⟩
    simp only [holds, matchLength_pow _ hlog7 hlog16]
    have := log_bounds _ hz
    exact ⟨by omega, by omega⟩
  · step as ⟨ i, i_post ⟩
    step as ⟨ c, c_post ⟩
    have hl : ml_base.val < 128 := by scalar_tac
    have hi : i.val = ml_base.val := by
      rw [i_post, UScalar.cast_val_eq, Nat.mod_eq_of_lt (by scalar_tac)]
    have hc : c = ml_code.ML_CODE[ml_base.val]! := by
      subst c_post; simp [Array.getElem!_Nat_eq, hi, List.getElem!_eq_getElem?_getD,
        List.getElem?_eq_getElem (by scalar_tac : ml_base.val < ml_code.ML_CODE.val.length)]
    rw [hc]
    exact ML_CODE_rfc ml_base.val hl

/-! ## Round trips through the decoder's tables -/

/-- Every literals length up to 131071 survives the encoder's code and the
decoder's table: the decoder's baseline for the encoder's code plus the extra
field `length - baseline`, which fits in the code's `Number_of_Bits`
(`LL_BITS`), is the length. The code is the only one whose RFC range holds the
length. -/
theorem literals_length_round_trip (lit_length : U32) (h : lit_length.val ≤ 131071) :
    (do let c ← ll_code lit_length; let t ← decompressor.core.sequences.LL_BASE; ok (c, t))
      ⦃ (r : U8 × Std.Array U32 36#usize) =>
        r.1.val < 36 ∧
        r.2[r.1.val]!.val ≤ lit_length.val ∧
        lit_length.val - r.2[r.1.val]!.val < 2 ^ shared.format.LL_BITS[r.1.val]!.val ∧
        ∀ c' < 36, holds literalsLengthCodes c' lit_length.val → c' = r.1.val ⦄ := by
  step as ⟨ c, hc, hholds ⟩
  step as ⟨ t, ht ⟩
  refine ⟨hc, ?_, ?_, fun c' hc' h' => literalsLength_unique c' c.val _ hc' hc h' hholds⟩
  · rw [ht _ hc]; exact hholds.1
  · rw [ht _ hc, LL_BITS_rfc _ hc]; unfold holds at hholds; omega

/-- Every match length from 3 to 131074 survives the encoder's code and the
decoder's table in the same way. -/
theorem match_length_round_trip (ml_base : U32) (h : ml_base.val ≤ 131071) :
    (do let c ← ml_code ml_base; let t ← decompressor.core.sequences.ML_BASE; ok (c, t))
      ⦃ (r : U8 × Std.Array U32 53#usize) =>
        r.1.val < 53 ∧
        r.2[r.1.val]!.val ≤ ml_base.val + 3 ∧
        ml_base.val + 3 - r.2[r.1.val]!.val < 2 ^ shared.format.ML_BITS[r.1.val]!.val ∧
        ∀ c' < 53, holds matchLengthCodes c' (ml_base.val + 3) → c' = r.1.val ⦄ := by
  step as ⟨ c, hc, hholds ⟩
  step as ⟨ t, ht ⟩
  refine ⟨hc, ?_, ?_, fun c' hc' h' => matchLength_unique c' c.val _ hc' hc h' hholds⟩
  · rw [ht _ hc]; exact hholds.1
  · rw [ht _ hc, ML_BITS_rfc _ hc]; unfold holds at hholds; omega

end mzstd
