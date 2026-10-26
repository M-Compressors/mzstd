import Mzstd.Basic

/-! # `Number_of_Sequences`

RFC 8878 section 3.1.1.3.2.1 opens the sequences section with
`Number_of_Sequences`, one to three bytes chosen by the first byte. The decoder
reads it with `sequence_count` (`decompressor/core/sequences.rs`) and the
encoder writes it with `write_sequence_count` (`compressor/core/block.rs`).
Both are proved against the RFC's rule `Mzstd.Rfc.numberOfSequences`: the
decoder computes exactly the RFC's count and size and rejects exactly the
truncated fields, the encoder writes a field the RFC reads back as its count,
in the shortest of the RFC's forms, and the two compose.
-/

open Aeneas Aeneas.Std Result
open Mzstd.Rfc

namespace mzstd
open decompressor.core.sequences

/-- The decoder's result agrees with the RFC: the error exactly when the RFC's
rule finds the field truncated, otherwise the RFC's count and size. -/
def countResult (src : List Nat)
    (r : core.result.Result (Usize × Usize) decompressor.error.InvalidDataKind) : Prop :=
  match numberOfSequences src with
  | none => r = .Err .SequencesSection
  | some (n, u) => ∃ c used : Usize, r = .Ok (c, used) ∧ c.val = n ∧ used.val = u

/-- `u16::from_le_bytes` reads its two bytes with the RFC's little-endian
convention. -/
theorem U16_from_le_bytes_val (a : Std.Array U8 2#usize) :
    (core.num.U16.from_le_bytes a).val = le (a.val.map (·.val)) := by
  simp only [core.num.U16.from_le_bytes, UScalar.val, BitVec.toNat_cast, fromLEBytes_toNat,
    List.map_map]
  rfl

/-- The two-byte form, `((byte0 - 128) << 8) + byte1`, without overflow. -/
theorem two_byte_count (b0 b1 : U8) (h0 : ¬ b0.val < 128) (h1 : b0.val < 255) :
    (do
      let i1 ← core.convert.num.FromUsizeU8.from b0 - 128#usize
      let i2 ← i1 <<< 8#i32
      let i4 ← i2 + core.convert.num.FromUsizeU8.from b1
      ok (core.result.Result.Ok (i4, 2#usize) :
        core.result.Result (Usize × Usize) decompressor.error.InvalidDataKind)) ⦃ r =>
      ∃ c used : Usize, r = .Ok (c, used) ∧ c.val = (b0.val - 128) * 256 + b1.val ∧ used.val = 2 ⦄ := by
  have hbits := numBits_ge
  have hsize := usize_size_ge
  have hv0 := core.convert.num.FromUsizeU8.from_val_eq b0
  have hv1 := core.convert.num.FromUsizeU8.from_val_eq b1
  step as ⟨ i1, i1_post ⟩
  step as ⟨ i2, i2_post ⟩
  have hi2 : i2.val = (b0.val - 128) * 256 := by
    rw [i2_post, Nat.shiftLeft_eq, i1_post, hv0, Nat.mod_eq_of_lt (by scalar_tac)]
  step as ⟨ i4, i4_post ⟩
  exact ⟨i4, 2#usize, rfl, by rw [i4_post, hi2, hv1], rfl⟩

/-- **Spec theorem for `mzstd::decompressor::core::sequences::sequence_count`**
For every input, the RFC's `Number_of_Sequences` and the bytes it uses, or
`InvalidDataKind::SequencesSection` exactly when the field is truncated. -/
@[step]
theorem sequence_count.spec (src : Slice U8) :
    sequence_count src ⦃ r => countResult (src.val.map (·.val)) r ⦄ := by
  unfold sequence_count
  rcases hs : src.val with _ | ⟨b0, _ | ⟨b1, _ | ⟨b2, rest⟩⟩⟩ <;>
  simp [core.slice.Slice.first, core.result.Result.Insts.CoreOpsTry.branch,
    core.result.Result.Insts.CoreOpsTry_traitFromResidualResult.from_residual,
    core.convert.FromSame.from, core.slice.Slice.get,
    core.slice.index.SliceIndexRangeUsizeSlice.get, hs, countResult, numberOfSequences, lift]
  · by_cases h0 : b0.val < 128 <;> simp [h0]
  · by_cases h0 : b0.val < 128
    · simp [h0]
    · simp only [h0, ite_false]
      by_cases h1 : b0.val < 255
      · simp only [h1, ite_true]
        exact two_byte_count b0 b1 h0 h1
      · simp [h1]
  · by_cases h0 : b0.val < 128
    · simp [h0]
    · simp only [h0, ite_false]
      by_cases h1 : b0.val < 255
      · simp only [h1, ite_true]
        exact two_byte_count b0 b1 h0 h1
      · simp only [h1, ite_false]
        step*
        refine ⟨i3, 3#usize, rfl, ?_, rfl⟩
        rw [i3_post, core.convert.num.FromUsizeU16.from_val_eq, U16_from_le_bytes_val]
        simp [Mzstd.Rfc.le, i_post, i1_post]
        ring

/-- The RFC's one-, two- and three-byte forms, whatever follows them. -/
theorem numberOfSequences_one (b0 : Nat) (rest : List Nat) (h : b0 < 128) :
    numberOfSequences (b0 :: rest) = some (b0, 1) := by
  rcases rest with _ | ⟨_, _ | ⟨_, _⟩⟩ <;> simp [numberOfSequences, h]

theorem numberOfSequences_two (b0 b1 : Nat) (rest : List Nat) (h0 : 128 ≤ b0) (h1 : b0 < 255) :
    numberOfSequences (b0 :: b1 :: rest) = some ((b0 - 128) * 256 + b1, 2) := by
  have : ¬ b0 < 128 := by omega
  rcases rest with _ | ⟨_, _⟩ <;> simp [numberOfSequences, this, h1]

theorem numberOfSequences_three (b1 b2 : Nat) (rest : List Nat) :
    numberOfSequences (255 :: b1 :: b2 :: rest) = some (b1 + b2 * 256 + 0x7F00, 3) := by
  simp [numberOfSequences]

open compressor.core.block in
/-- **Spec theorem for `mzstd::compressor::core::block::write_sequence_count`**
For a destination of at least three bytes and every count below
`0x7F00 + 0x10000`, the shortest RFC form: one byte below 128, two below
`0x7F00`, three otherwise. Exactly the field's bytes `out` are written, the rest
of the destination is unchanged, and the RFC's rule reads `out` back as the
count and its size, whatever bytes follow. -/
@[step]
theorem write_sequence_count.spec (dst : Slice U8) (nb_seq : Usize) (hlen : 3 ≤ dst.length)
    (hn : nb_seq.val < 0x7F00 + 0x10000) :
    write_sequence_count dst nb_seq ⦃ (size : Usize) (dst' : Slice U8) =>
      size.val = (if nb_seq.val < 128 then 1 else if nb_seq.val < 0x7F00 then 2 else 3) ∧
      ∃ out : List U8, out.length = size.val ∧ dst'.val = out ++ dst.val.drop size.val ∧
        ∀ rest, numberOfSequences (out.map (·.val) ++ rest) = some (nb_seq.val, size.val) ⦄ := by
  have hbits := numBits_ge
  have hsize := usize_size_ge
  unfold write_sequence_count
  rcases hd : dst.val with _ | ⟨a, _ | ⟨b, _ | ⟨c, tail⟩⟩⟩
  · simp [Slice.length, hd] at hlen
  · simp [Slice.length, hd] at hlen
  · simp [Slice.length, hd] at hlen
  have hdl : dst.length = tail.length + 3 := by simp [Slice.length, hd]
  split
  · -- One byte: the count itself.
    rename_i hlt
    step as ⟨ i, i_post ⟩
    step as ⟨ dst1, dst1_post ⟩
    have hi : i.val = nb_seq.val := by
      rw [i_post, UScalar.cast_val_eq, Nat.mod_eq_of_lt (by scalar_tac)]
    have hlt' : nb_seq.val < 128 := by scalar_tac
    refine ⟨by simp [hlt'], [i], rfl, ?_, fun rest => ?_⟩
    · rw [dst1_post]; simp [hd]
    · simp only [List.map_cons, List.map_nil, List.cons_append, List.nil_append, hi]
      exact numberOfSequences_one nb_seq.val rest hlt'
  · split
    · -- Two bytes: `128 + count / 256`, then the low byte.
      rename_i hge hlt
      have hge' : 128 ≤ nb_seq.val := by scalar_tac
      have hlt' : nb_seq.val < 0x7F00 := by scalar_tac
      step as ⟨ i, i_post ⟩
      step as ⟨ i1, i1_post ⟩
      step as ⟨ i2, i2_post ⟩
      step as ⟨ dst1, dst1_post ⟩
      step as ⟨ i3, i3_post ⟩
      step as ⟨ dst2, dst2_post ⟩
      have hi2 : i2.val = nb_seq.val / 256 + 128 := by
        rw [i2_post, UScalar.cast_val_eq, i1_post, i_post, Nat.shiftRight_eq_div_pow,
          Nat.mod_eq_of_lt (by simp; omega)]
      have hi3 : i3.val = nb_seq.val % 256 := by
        rw [i3_post, UScalar.cast_val_eq]; rfl
      refine ⟨by simp [show ¬ nb_seq.val < 128 by omega, hlt'], [i2, i3], rfl, ?_, fun rest => ?_⟩
      · rw [dst2_post, dst1_post]; simp [hd]
      · simp only [List.map_cons, List.map_nil, List.cons_append, List.nil_append]
        rw [numberOfSequences_two _ _ _ (by omega) (by omega), hi2, hi3]
        simp only [Option.some.injEq, Prod.mk.injEq]
        constructor
        · omega
        · rfl
    · -- Three bytes: 255, then `count - 0x7F00` in two little-endian bytes.
      rename_i hge hge2
      have hge' : 0x7F00 ≤ nb_seq.val := by scalar_tac
      step*
      have hi1 : i1.val = nb_seq.val - 0x7F00 := by
        rw [i1_post, UScalar.cast_val_eq, i_post, Nat.mod_eq_of_lt (by simp; omega)]; simp
      have hlen2 : a.val.length = 2 := by simp
      have two : ∀ l : List U8, l.length = 2 → ∃ x y, l = [x, y] := by
        intro l hl; match l, hl with | [x, y], _ => exact ⟨x, y, rfl⟩
      obtain ⟨x, y, hxy⟩ := two _ hlen2
      have hle : x.val + 256 * y.val = nb_seq.val - 0x7F00 := by
        have h := le_toLEBytes 16 i1.bv (by decide)
        have hmap : a.val.map (·.val) = i1.bv.toLEBytes.map BitVec.toNat := by
          rw [a_post, List.map_map]; rfl
        rw [← hmap, hxy] at h
        have hv : i1.bv.toNat = i1.val := rfl
        simp only [List.map_cons, List.map_nil, Mzstd.Rfc.le, hv] at h
        omega
      refine ⟨by simp [show ¬ nb_seq.val < 128 by omega, show ¬ nb_seq.val < 32512 by omega],
        [255#u8, x, y], rfl, ?_, fun rest => ?_⟩
      · rw [s_post2, s2_post, s1_post, dst1_post]
        simp [hd, Array.to_slice, hxy, List.setSlice!]
      · simp only [List.map_cons, List.map_nil, List.cons_append, List.nil_append]
        rw [show (255#u8 : U8).val = 255 from rfl, numberOfSequences_three]
        simp only [Option.some.injEq, Prod.mk.injEq]
        constructor
        · omega
        · rfl

/-- No encoding of the RFC is shorter than the one the encoder picks: a field
of `u` bytes that decodes to `n` has `u` at least the encoder's size for `n`. -/
theorem numberOfSequences_shortest (l : List Nat) (n u : Nat) (hl : ∀ b ∈ l, b < 256)
    (h : numberOfSequences l = some (n, u)) :
    (if n < 128 then 1 else if n < 0x7F00 then 2 else 3) ≤ u := by
  rcases l with _ | ⟨b0, _ | ⟨b1, _ | ⟨b2, rest⟩⟩⟩
  · simp [numberOfSequences] at h
  · simp only [numberOfSequences] at h
    split at h <;> simp_all
  · have h1 : b1 < 256 := hl b1 (by simp)
    simp only [numberOfSequences] at h
    split at h
    · simp_all
    · split at h <;> simp at h
      obtain ⟨rfl, rfl⟩ := h
      split <;> [omega; split <;> omega]
  · have h1 : b1 < 256 := hl b1 (by simp)
    simp only [numberOfSequences] at h
    split at h
    · simp_all
    · split at h <;> simp at h <;> obtain ⟨rfl, rfl⟩ := h
      · split <;> [omega; split <;> omega]
      · split <;> [omega; split <;> omega]

open compressor.core.block in
/-- The decoder reads back what the encoder wrote: for every count a block can
hold, `sequence_count` on the bytes `write_sequence_count` produced returns the
count and the field's size, whatever follows the field. -/
theorem sequence_count_round_trip (dst : Slice U8) (nb_seq : Usize) (hlen : 3 ≤ dst.length)
    (hn : nb_seq.val < 0x7F00 + 0x10000) :
    (do
      let (size, dst') ← write_sequence_count dst nb_seq
      let r ← sequence_count dst'
      ok (size, r)) ⦃ (p : Usize × core.result.Result (Usize × Usize) decompressor.error.InvalidDataKind) =>
        p.2 = .Ok (nb_seq, p.1) ⦄ := by
  step as ⟨ size, dst', hsize, out, hout, hdst', hdec ⟩
  step as ⟨ r, hr ⟩
  simp only [countResult, hdst', List.map_append, hdec] at hr
  obtain ⟨c, used, hr, hc, hu⟩ := hr
  rw [hr, UScalar.eq_of_val_eq hc, UScalar.eq_of_val_eq hu]

end mzstd
