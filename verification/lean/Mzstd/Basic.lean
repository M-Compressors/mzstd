import Mzstd.Code.Funs
import Mzstd.Rfc

/-! # Facts shared by the proofs about the generated code

Platform bounds that hold on every target this crate supports (a `usize` of 32
or 64 bits), the value of the constants the specifications rely on, the
specification of `shared::format::highbit32`, and the bytes `to_le_bytes`
produces, read back with the RFC's little-endian convention `Mzstd.Rfc.le`.
-/

open Aeneas Aeneas.Std Result

namespace mzstd

/-- `usize` has at least 32 bits on every platform the crate builds for. -/
theorem numBits_ge : 32 ≤ System.Platform.numBits := by
  cases System.Platform.numBits_eq with
  | inl h => simp [h]
  | inr h => simp [h]

theorem usize_size_ge : 2 ^ 32 ≤ Usize.size := by
  rw [Usize.size_def, Usize.numBits_def]
  cases System.Platform.numBits_eq with
  | inl h => simp [h]
  | inr h => simp [h]

@[simp, scalar_tac_simps, agrind =, grind =, bvify]
theorem shared.format.REP_NUM_val : shared.format.REP_NUM.val = 3 := by
  simp [shared.format.REP_NUM]

@[simp, scalar_tac_simps, agrind =, grind =]
theorem shared.format.REP_NUM_cast_val :
    (UScalar.cast .U32 shared.format.REP_NUM).val = 3 := by
  simp [shared.format.REP_NUM]

@[simp, scalar_tac_simps, agrind =, grind =, bvify]
theorem shared.format.LONG_NB_SEQ_val : shared.format.LONG_NB_SEQ.val = 0x7F00 := by
  simp [shared.format.LONG_NB_SEQ]

@[simp, scalar_tac_simps, agrind =, grind =, bvify]
theorem shared.format.WINDOW_LOG_ABSOLUTE_MIN_val :
    shared.format.WINDOW_LOG_ABSOLUTE_MIN.val = 10 := by
  simp [shared.format.WINDOW_LOG_ABSOLUTE_MIN]

@[simp, scalar_tac_simps, agrind =, grind =, bvify]
theorem shared.format.FRAME_HEADER_SIZE_MAX_val :
    shared.format.FRAME_HEADER_SIZE_MAX.val = 18 := by
  simp [shared.format.FRAME_HEADER_SIZE_MAX]

@[simp, scalar_tac_simps, agrind =, grind =, bvify]
theorem shared.format.MIN_MATCH_val : shared.format.MIN_MATCH.val = 3 := by
  simp [shared.format.MIN_MATCH]

theorem U32_size_eq : UScalar.size UScalarTy.U32 = 2 ^ 32 := by simp [U32.size_def, U32.numBits_def]

theorem leading_zeros_val (value : U32) (h : value.val ≠ 0) :
    (core.num.U32.leading_zeros value).val = 32 - Nat.log 2 value.val - 1 := by
  simp only [core.num.U32.leading_zeros, BitVec.leadingZeros]
  have hne : ¬ (value.bv = 0#32) := by
    intro hz; apply h; simpa using congrArg BitVec.toNat hz
  have hl : Nat.log 2 value.val < 32 := Nat.log_lt_of_lt_pow h (by simpa using value.hBounds)
  split
  · exact absurd ‹_› hne
  · have : value.bv.toNat = value.val := rfl
    rw [this]
    change (BitVec.ofNat 32 _).toNat = _
    rw [BitVec.toNat_ofNat, Nat.mod_eq_of_lt (by omega)]

/-- **Spec theorem for `mzstd::shared::format::highbit32`**
The position of the highest set bit: the floor of the base-2 logarithm. -/
@[step]
theorem shared.format.highbit32.spec (value : U32) (h : value.val ≠ 0) :
    shared.format.highbit32 value ⦃ (r : U32) => r.val = Nat.log 2 value.val ⦄ := by
  have hz := leading_zeros_val value h
  have hl : Nat.log 2 value.val < 32 := Nat.log_lt_of_lt_pow h (by simpa using value.hBounds)
  unfold shared.format.highbit32
  step*

/-- The bytes of a bit vector, least significant first. -/
theorem toLEBytes_toNat (w : Nat) (b : BitVec w) (i : Nat) (hi : i < (w + 7) / 8) :
    (b.toLEBytes[i]!).toNat = b.toNat / 256 ^ i % 256 := by
  induction i generalizing w b with
  | zero =>
    unfold BitVec.toLEBytes
    have hw : w > 0 := by omega
    simp [hw, BitVec.toNat_setWidth]
  | succ i ih =>
    unfold BitVec.toLEBytes
    have hw : w > 0 := by omega
    simp only [hw, ite_true, List.getElem!_cons_succ]
    rw [ih (w - 8) _ (by omega)]
    simp only [BitVec.toNat_setWidth, BitVec.toNat_ushiftRight]
    have hb : b.toNat < 2 ^ w := b.isLt
    have : b.toNat >>> 8 % 2 ^ (w - 8) = b.toNat >>> 8 := by
      apply Nat.mod_eq_of_lt
      rw [Nat.shiftRight_eq_div_pow]
      rcases Nat.lt_or_ge w 8 with h8 | h8
      · have : b.toNat / 2 ^ 8 = 0 := Nat.div_eq_of_lt (lt_of_lt_of_le hb (Nat.pow_le_pow_right (by decide) h8.le))
        rw [this]; exact Nat.two_pow_pos _
      · rw [Nat.div_lt_iff_lt_mul (Nat.two_pow_pos _), ← Nat.pow_add, Nat.sub_add_cancel h8]
        exact hb
    rw [this, Nat.shiftRight_eq_div_pow, Nat.div_div_eq_div_mul, pow_succ]
    congr 2
    ring

/-- `fromLEBytes` reads bytes with the RFC's little-endian convention. -/
theorem fromLEBytes_toNat (l : List (BitVec 8)) :
    (BitVec.fromLEBytes l).toNat = Mzstd.Rfc.le (l.map BitVec.toNat) := by
  induction l with
  | nil => simp [BitVec.fromLEBytes, Mzstd.Rfc.le]
  | cons b l ih =>
    unfold BitVec.fromLEBytes
    simp only [List.map_cons, Mzstd.Rfc.le, List.length_cons]
    rw [BitVec.toNat_or, BitVec.toNat_shiftLeft, BitVec.toNat_setWidth, BitVec.toNat_setWidth, ih]
    have hb : b.toNat < 2 ^ 8 := b.isLt
    have hr : Mzstd.Rfc.le (l.map BitVec.toNat) < 2 ^ (8 * l.length) := by
      rw [← ih]; exact (BitVec.fromLEBytes l).isLt
    have hw : 2 ^ (8 * l.length) * 2 ^ 8 = 2 ^ (8 * (l.length + 1)) := by
      rw [← pow_add]; ring_nf
    have h1 : b.toNat % 2 ^ (8 * (l.length + 1)) = b.toNat :=
      Nat.mod_eq_of_lt (lt_of_lt_of_le hb (Nat.pow_le_pow_right (by decide) (by omega)))
    have h2 : Mzstd.Rfc.le (l.map BitVec.toNat) % 2 ^ (8 * (l.length + 1)) =
        Mzstd.Rfc.le (l.map BitVec.toNat) :=
      Nat.mod_eq_of_lt (lt_of_lt_of_le hr (Nat.pow_le_pow_right (by decide) (by omega)))
    have h3 : Mzstd.Rfc.le (l.map BitVec.toNat) <<< 8 % 2 ^ (8 * (l.length + 1)) =
        2 ^ 8 * Mzstd.Rfc.le (l.map BitVec.toNat) := by
      rw [Nat.shiftLeft_eq, Nat.mod_eq_of_lt (by rw [← hw]; exact Nat.mul_lt_mul_of_pos_right hr (by decide))]
      ring
    rw [h1, h2, h3, Nat.or_comm, ← Nat.two_pow_add_eq_or_of_lt hb]
    ring

/-- The first `k` bytes of `toLEBytes b`, read little-endian, are `b` modulo
`2 ^ (8 k)`. -/
theorem le_take_toLEBytes (w : Nat) (b : BitVec w) (k : Nat) (hk : 8 * k ≤ w) :
    Mzstd.Rfc.le ((b.toLEBytes.take k).map BitVec.toNat) = b.toNat % 2 ^ (8 * k) := by
  induction k generalizing w b with
  | zero => simp [Mzstd.Rfc.le, Nat.mod_one]
  | succ k ih =>
    unfold BitVec.toLEBytes
    have hw : w > 0 := by omega
    simp only [hw, ite_true, List.take_succ_cons, List.map_cons, Mzstd.Rfc.le]
    rw [ih (w - 8) _ (by omega)]
    simp only [BitVec.toNat_setWidth, BitVec.toNat_ushiftRight]
    have hb : b.toNat < 2 ^ w := b.isLt
    have : b.toNat >>> 8 % 2 ^ (w - 8) = b.toNat >>> 8 := by
      apply Nat.mod_eq_of_lt
      rw [Nat.shiftRight_eq_div_pow, Nat.div_lt_iff_lt_mul (Nat.two_pow_pos _), ← Nat.pow_add,
        Nat.sub_add_cancel (by omega)]
      exact hb
    rw [this, Nat.shiftRight_eq_div_pow, show 8 * (k + 1) = 8 + 8 * k by ring, pow_add, Nat.mod_mul]
    norm_num

/-- All the bytes of a whole number of bytes read back to the value. -/
theorem le_toLEBytes (w : Nat) (b : BitVec w) (hw : w % 8 = 0) :
    Mzstd.Rfc.le (b.toLEBytes.map BitVec.toNat) = b.toNat := by
  have h := le_take_toLEBytes w b (w / 8) (by omega)
  rw [List.take_of_length_le (by rw [BitVec.toLEBytes_length]; omega)] at h
  rw [h, show 8 * (w / 8) = w by omega, Nat.mod_eq_of_lt b.isLt]

end mzstd
