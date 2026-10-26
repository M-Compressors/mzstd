import Mzstd.Basic

/-! # FSE state transitions

RFC 8878 section 4.1.1 gives each state of a symbol with probability `p` a
`Number_of_Bits` and a `Baseline` (Table 21): the states, sorted in natural
order, share the table's `2 ^ Accuracy_Log` next states; the
`nextPowerOfTwo p - p` lowest take a double share and one more bit, and the
baselines start at the higher states. The decoder's table builder
(`SeqTable::fill` in `decompressor/core/fse.rs`) visits the cells in natural
order and passes the `k`-th state of a symbol to `transition` as the counter
`p + k`. `transition` computes the RFC's pair from that counter.
-/

open Aeneas Aeneas.Std Result
open Mzstd.Rfc

namespace mzstd
open decompressor.core.fse

/-- Setting the low bit leaves the floor of the logarithm of a positive number
unchanged. -/
theorem log_or_one (n : Nat) (h : 1 ≤ n) : Nat.log 2 (n ||| 1) = Nat.log 2 n := by
  have hn : n ≠ 0 := by omega
  apply Nat.log_eq_of_pow_le_of_lt_pow
  · exact le_trans (Nat.pow_log_le_self 2 hn) Nat.left_le_or
  · have hlt := Nat.lt_pow_succ_log_self (by decide : 1 < 2) n
    have h1 : 1 < 2 ^ (Nat.log 2 n + 1) := Nat.one_lt_two_pow (by omega)
    exact Nat.or_lt_two_pow hlt h1

/-- The RFC's pair for the state of order `k` of a symbol with probability `p`
is what the counter `n = p + k` gives directly: `Accuracy_Log - log2 n` bits and
the baseline `n * 2 ^ bits - 2 ^ Accuracy_Log`. -/
theorem fseState_eq (L p k : Nat) (hp : 1 ≤ p) (hpL : p ≤ 2 ^ L) (hk : k < p) :
    fseState L p k =
      (L - Nat.log 2 (p + k), (p + k) * 2 ^ (L - Nat.log 2 (p + k)) - 2 ^ L) := by
  set c := Nat.clog 2 p with hc
  have hcL : c ≤ L := Nat.clog_le_of_le_pow hpL
  have hpc : p ≤ 2 ^ c := Nat.le_pow_clog (by decide) p
  have hshare : 2 ^ L / 2 ^ c = 2 ^ (L - c) := by
    rw [Nat.pow_div hcL (by decide)]
  simp only [fseState, nextPowerOfTwo, ← hc, hshare]
  split
  · -- A doubled state: `p ≤ p + k < 2 ^ c`, so `p ≥ 2` and `log2 (p + k) = c - 1`.
    rename_i hlt
    have hp2 : 1 < p := by
      rcases Nat.lt_or_ge 1 p with h | h
      · exact h
      · have : p = 1 := by omega
        subst this; simp [hc] at hlt
    have hc1 : 1 ≤ c := Nat.clog_pos (by decide) hp2
    have hlow : 2 ^ (c - 1) < p := by
      have := Nat.pow_pred_clog_lt_self (by decide : 1 < 2) hp2
      simpa [← hc] using this
    have hlog : Nat.log 2 (p + k) = c - 1 := by
      apply Nat.log_eq_of_pow_le_of_lt_pow (by omega)
      rw [Nat.sub_add_cancel hc1]; omega
    rw [hlog]
    have hcc : 2 ^ c = 2 * 2 ^ (c - 1) := by
      rw [← pow_succ', Nat.sub_add_cancel hc1]
    have hL1 : L - (c - 1) = L - c + 1 := by omega
    have hpowL : 2 ^ L = 2 ^ c * 2 ^ (L - c) := by rw [← pow_add, Nat.add_sub_cancel' hcL]
    rw [hL1]
    refine Prod.ext (by simp) ?_
    simp only
    rw [hpowL, pow_succ]
    -- Both sides are `(2 (p + k) - 2 ^ c) * 2 ^ (L - c)`.
    have e1 : (p - (2 ^ c - p)) * 2 ^ (L - c) + 2 * k * 2 ^ (L - c) =
        (2 * (p + k) - 2 ^ c) * 2 ^ (L - c) := by
      rw [← Nat.add_mul]; congr 1; omega
    have e2 : (p + k) * (2 ^ (L - c) * 2) - 2 ^ c * 2 ^ (L - c) =
        (2 * (p + k) - 2 ^ c) * 2 ^ (L - c) := by
      rw [Nat.sub_mul]; congr 1; ring
    rw [e1, e2]
  · -- A single state: `2 ^ c ≤ p + k < 2 p ≤ 2 ^ (c + 1)`, so `log2 (p + k) = c`.
    rename_i hge
    have hlog : Nat.log 2 (p + k) = c := by
      apply Nat.log_eq_of_pow_le_of_lt_pow (by omega)
      rw [pow_succ]; omega
    rw [hlog]
    have hpowL : 2 ^ L = 2 ^ c * 2 ^ (L - c) := by rw [← pow_add, Nat.add_sub_cancel' hcL]
    refine Prod.ext rfl ?_
    simp only
    rw [hpowL, ← Nat.sub_mul]
    congr 1
    omega

/-- **Spec theorem for `mzstd::decompressor::core::fse::transition`**
For the counter `p + k` of the `k`-th state (in natural order) of a symbol with
probability `p ≥ 1` in a table of `2 ^ table_log` states, the `Number_of_Bits`
and `Baseline` of RFC 8878 Table 21. A "less than 1" symbol takes one state
with counter 1, the `p = 1` case: a full state reset of `Accuracy_Log` bits from
baseline 0. -/
@[step]
theorem transition.spec (next : U16) (table_log : U32) (p k : Nat)
    (hL : table_log.val ≤ 15) (hp : 1 ≤ p) (hpL : p ≤ 2 ^ table_log.val) (hk : k < p)
    (hn : next.val = p + k) :
    transition next table_log ⦃ (r : U8 × U16) =>
      (r.1.val, r.2.val) = fseState table_log.val p k ⦄ := by
  rw [fseState_eq _ _ _ hp hpL hk, ← hn]
  have hn1 : 1 ≤ next.val := by omega
  have hnL : next.val < 2 ^ (table_log.val + 1) := by rw [pow_succ]; omega
  have hlog := Nat.log_lt_of_lt_pow (by omega) hnL
  have hlogle : Nat.log 2 next.val ≤ table_log.val := by omega
  have hpow := Nat.pow_log_le_self 2 (by omega : next.val ≠ 0)
  have hlt := Nat.lt_pow_succ_log_self (by decide : 1 < 2) next.val
  have hL16 : 2 ^ table_log.val ≤ 2 ^ 15 := Nat.pow_le_pow_right (by decide) hL
  unfold transition
  step as ⟨ state, state_post ⟩
  have hstate : state.val = next.val := by
    rw [state_post, UScalar.cast_val_eq, Nat.mod_eq_of_lt (by scalar_tac)]
  step as ⟨ i, i_post ⟩
  have hi : i.val = next.val ||| 1 := by rw [i_post, UScalar.val_or, hstate]; rfl
  have hi0 : i.val ≠ 0 := by rw [hi]; exact Nat.ne_of_gt (Nat.lt_of_lt_of_le hn1 Nat.left_le_or)
  step as ⟨ i1, i1_post ⟩
  rw [hi, log_or_one _ hn1] at i1_post
  step as ⟨ i2, i2_post ⟩
  step as ⟨ nb_bits, nb_post ⟩
  have hnb : nb_bits.val = table_log.val - Nat.log 2 next.val := by
    rw [nb_post, UScalar.val_and, i2_post, core.num.U32.wrapping_sub_val_eq, i1_post, U32_size_eq]
    have : (table_log.val + (2 ^ 32 - Nat.log 2 next.val)) % 2 ^ 32 =
        table_log.val - Nat.log 2 next.val := by omega
    have h31 : (31#u32 : U32).val = 2 ^ 5 - 1 := rfl
    rw [this, h31, Nat.and_two_pow_sub_one_eq_mod, Nat.mod_eq_of_lt (by omega)]
  step as ⟨ i3, i3_post ⟩
  -- `next * 2 ^ nb_bits` lies in `[2 ^ table_log, 2 ^ (table_log + 1))`.
  have hprod_lo : 2 ^ table_log.val ≤ next.val * 2 ^ nb_bits.val := by
    rw [hnb]
    calc 2 ^ table_log.val = 2 ^ Nat.log 2 next.val * 2 ^ (table_log.val - Nat.log 2 next.val) := by
          rw [← pow_add, Nat.add_sub_cancel' hlogle]
      _ ≤ _ := Nat.mul_le_mul_right _ hpow
  have hprod_hi : next.val * 2 ^ nb_bits.val < 2 ^ (table_log.val + 1) := by
    rw [hnb]
    calc next.val * 2 ^ (table_log.val - Nat.log 2 next.val)
        < 2 ^ (Nat.log 2 next.val + 1) * 2 ^ (table_log.val - Nat.log 2 next.val) :=
          Nat.mul_lt_mul_of_pos_right hlt (Nat.two_pow_pos _)
      _ = 2 ^ (table_log.val + 1) := by rw [← pow_add]; congr 1; omega
  have hL17 : 2 ^ (table_log.val + 1) ≤ 2 ^ 16 := Nat.pow_le_pow_right (by decide) (by omega)
  step as ⟨ i4, i4_post ⟩
  have hi4 : i4.val = next.val * 2 ^ nb_bits.val := by
    rw [i4_post, Nat.shiftLeft_eq, hstate, Nat.mod_eq_of_lt (by scalar_tac)]
  step as ⟨ i5, i5_post ⟩
  have hi5 : i5.val = 2 ^ table_log.val := by
    rw [i5_post, Nat.one_shiftLeft, Nat.mod_eq_of_lt (by scalar_tac)]
  step as ⟨ i6, i6_post ⟩
  have hi6 : i6.val = next.val * 2 ^ nb_bits.val - 2 ^ table_log.val := by
    rw [i6_post, core.num.U32.wrapping_sub_val_eq, hi4, hi5, U32_size_eq]
    omega
  step as ⟨ i7, i7_post ⟩
  refine ⟨?_, ?_⟩
  · rw [i3_post, UScalar.cast_val_eq, Nat.mod_eq_of_lt (by scalar_tac), hnb]
  · rw [i7_post, UScalar.cast_val_eq, hi6, Nat.mod_eq_of_lt (by scalar_tac), ← hnb]

end mzstd
