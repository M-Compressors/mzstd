import Mzstd.Basic

/-! # Block headers

RFC 8878 section 3.1.1.2: a block header is three little-endian bytes holding
`Last_Block` in bit 0, `Block_Type` in bits 1-2 and `Block_Size` in bits 3-23.
The decoder's `BlockHeader::parse` (`decompressor/core/block.rs`) reads exactly
those fields, rejects the reserved type and sizes above the frame's
`block_max`; the encoder's `write_block_header` (`compressor/core/cctx.rs`)
writes them, and the decoder reads back what the encoder wrote.
-/

open Aeneas Aeneas.Std Result
open Mzstd.Rfc

namespace mzstd
open decompressor.core.block

/-- The RFC block type each decoder `BlockKind` stands for. -/
def decompressor.core.block.BlockKind.rfc : BlockKind → BlockType
  | .Raw => .raw
  | .Rle => .rle
  | .Compressed => .compressed

/-- **Spec theorem for `mzstd::decompressor::core::block::BlockHeader::parse`**
The reserved `Block_Type` 3 fails with `BlockType`; a `Block_Size` above
`block_max` fails with `BlockSize`; any other header gives the RFC's
`Last_Block`, `Block_Type` and `Block_Size` (Table 9). -/
@[step]
theorem BlockHeader.parse.spec (bytes : Std.Array U8 3#usize) (block_max : Usize) :
    BlockHeader.parse bytes block_max ⦃ (r : core.result.Result decompressor.core.block.BlockHeader decompressor.error.InvalidDataKind) =>
      match parseBlockHeader bytes[0]!.val bytes[1]!.val bytes[2]!.val with
      | none => r = .Err .BlockType
      | some h =>
        if block_max.val < h.size then r = .Err .BlockSize
        else ∃ out, r = .Ok out ∧ out.last = h.last ∧ out.kind.rfc = h.type ∧ out.size.val = h.size ⦄ := by
  unfold BlockHeader.parse
  step as ⟨ i, i_post ⟩
  step as ⟨ i1, i1_post ⟩
  step as ⟨ i2, i2_post ⟩
  step as ⟨ value, value_post ⟩
  have hv : value.val = le [bytes[0]!.val, bytes[1]!.val, bytes[2]!.val] := by
    have : value.val = value.bv.toNat := rfl
    rw [this, value_post, BitVec.toNat_cast, fromLEBytes_toNat]
    simp [Array.make, i_post, i1_post, i2_post, le, Array.getElem!_Nat_eq]
  generalize hvv : le [bytes[0]!.val, bytes[1]!.val, bytes[2]!.val] = v at hv
  simp only [parseBlockHeader, hvv]
  step as ⟨ i3, i3_post ⟩
  step as ⟨ i4, i4_post ⟩
  have hi4 : i4.val = v / 2 % 4 := by
    rw [i4_post, UScalar.val_and, i3_post, hv, Nat.shiftRight_eq_div_pow]
    exact Nat.and_two_pow_sub_one_eq_mod _ 2
  split
  · have hk : v / 2 % 4 = 0 := by rw [← hi4]; rfl
    simp only [hk, blockType, Option.map_some]
    step as ⟨ i5, i5_post ⟩
    step as ⟨ size, size_post ⟩
    have hsize : size.val = v / 8 := by
      rw [size_post, UScalar.cast_val_eq, Nat.mod_eq_of_lt (by scalar_tac), i5_post, hv,
        Nat.shiftRight_eq_div_pow]
    split
    · have hlt : block_max.val < v / 8 := by scalar_tac
      simp [hlt]
    · have hnlt : ¬ block_max.val < v / 8 := by scalar_tac
      step as ⟨ i6, i6_post ⟩
      have h6 : i6.val = v % 2 := by
        rw [i6_post, UScalar.val_and, hv]; exact Nat.and_one_is_mod _
      simp only [hnlt, ite_false]
      refine ⟨_, rfl, ?_, rfl, hsize⟩
      show (i6 != 0#u32) = _
      rcases Nat.mod_two_eq_zero_or_one v with h | h
      · have : i6 = 0#u32 := UScalar.eq_of_val_eq (by rw [h6, h]; rfl)
        simp [this, h]
      · have : i6 ≠ 0#u32 := fun e => by rw [e] at h6; simp at h6; omega
        simp [this, h]
  · have hk : v / 2 % 4 = 1 := by rw [← hi4]; rfl
    simp only [hk, blockType, Option.map_some]
    step as ⟨ i5, i5_post ⟩
    step as ⟨ size, size_post ⟩
    have hsize : size.val = v / 8 := by
      rw [size_post, UScalar.cast_val_eq, Nat.mod_eq_of_lt (by scalar_tac), i5_post, hv,
        Nat.shiftRight_eq_div_pow]
    split
    · have hlt : block_max.val < v / 8 := by scalar_tac
      simp [hlt]
    · have hnlt : ¬ block_max.val < v / 8 := by scalar_tac
      step as ⟨ i6, i6_post ⟩
      have h6 : i6.val = v % 2 := by
        rw [i6_post, UScalar.val_and, hv]; exact Nat.and_one_is_mod _
      simp only [hnlt, ite_false]
      refine ⟨_, rfl, ?_, rfl, hsize⟩
      show (i6 != 0#u32) = _
      rcases Nat.mod_two_eq_zero_or_one v with h | h
      · have : i6 = 0#u32 := UScalar.eq_of_val_eq (by rw [h6, h]; rfl)
        simp [this, h]
      · have : i6 ≠ 0#u32 := fun e => by rw [e] at h6; simp at h6; omega
        simp [this, h]
  · have hk : v / 2 % 4 = 2 := by rw [← hi4]; rfl
    simp only [hk, blockType, Option.map_some]
    step as ⟨ i5, i5_post ⟩
    step as ⟨ size, size_post ⟩
    have hsize : size.val = v / 8 := by
      rw [size_post, UScalar.cast_val_eq, Nat.mod_eq_of_lt (by scalar_tac), i5_post, hv,
        Nat.shiftRight_eq_div_pow]
    split
    · have hlt : block_max.val < v / 8 := by scalar_tac
      simp [hlt]
    · have hnlt : ¬ block_max.val < v / 8 := by scalar_tac
      step as ⟨ i6, i6_post ⟩
      have h6 : i6.val = v % 2 := by
        rw [i6_post, UScalar.val_and, hv]; exact Nat.and_one_is_mod _
      simp only [hnlt, ite_false]
      refine ⟨_, rfl, ?_, rfl, hsize⟩
      show (i6 != 0#u32) = _
      rcases Nat.mod_two_eq_zero_or_one v with h | h
      · have : i6 = 0#u32 := UScalar.eq_of_val_eq (by rw [h6, h]; rfl)
        simp [this, h]
      · have : i6 ≠ 0#u32 := fun e => by rw [e] at h6; simp at h6; omega
        simp [this, h]
  · rename_i h0 h1 h2
    have hk : v / 2 % 4 = 3 := by
      have n0 : i4.val ≠ 0 := fun h => h0 (UScalar.eq_of_val_eq (h.trans rfl))
      have n1 : i4.val ≠ 1 := fun h => h1 (UScalar.eq_of_val_eq (h.trans rfl))
      have n2 : i4.val ≠ 2 := fun h => h2 (UScalar.eq_of_val_eq (h.trans rfl))
      omega
    simp [hk, blockType]


/-- **Spec theorem for `mzstd::decompressor::core::block::BlockHeader::content_size`**
The bytes of `Block_Content`: one for an `RLE_Block`, `Block_Size` otherwise
(section 3.1.1.2.3). -/
@[step]
theorem BlockHeader.content_size.spec (self : decompressor.core.block.BlockHeader) :
    self.content_size ⦃ (r : Usize) =>
      r.val = blockContentSize ⟨self.last, self.kind.rfc, self.size.val⟩ ⦄ := by
  unfold BlockHeader.content_size
  cases h : self.kind <;> simp [blockContentSize, decompressor.core.block.BlockKind.rfc]

/-- Writing `s'` at the start of `s` puts exactly `s'` in its first bytes. -/
theorem take_setSlice!_zero {α : Type} (s s' : List α) (h : s'.length ≤ s.length) :
    (s.setSlice! 0 s').take s'.length = s' := by
  simp only [List.setSlice!, List.take_zero, List.nil_append, Nat.sub_zero, Nat.min_eq_left h,
    List.take_length, Nat.zero_add]
  rw [List.take_append_of_le_length (by simp)]
  simp

/-- **Spec theorem for `mzstd::compressor::core::cctx::write_block_header`**
For a block type below 3 and a size that fits 21 bits, the first three bytes
of `dst`, read little-endian, become `Last_Block + 2 Block_Type + 8 Block_Size`
(Table 9); the rest of `dst` is unchanged. -/
@[step]
theorem compressor.core.cctx.write_block_header.spec (dst : Slice U8) (kind size : U32)
    (last_block : Bool) (hd : 3 ≤ dst.length) (hk : kind.val ≤ 2) (hs : size.val < 2 ^ 21) :
    compressor.core.cctx.write_block_header dst kind size last_block ⦃ (r : Slice U8) =>
      r.length = dst.length ∧ (∀ i, 3 ≤ i → r.val[i]! = dst.val[i]!) ∧
      le ((r.val.take 3).map (·.val)) = (if last_block then 1 else 0) + 2 * kind.val + 8 * size.val ⦄ := by
  unfold compressor.core.cctx.write_block_header
  generalize hi : core.convert.num.FromU32Bool.from last_block = i
  have hiv : i.val = if last_block then 1 else 0 := by
    rw [← hi]; cases last_block <;> rfl
  have hi1 : i.val ≤ 1 := by rw [hiv]; split <;> simp
  simp only [lift]
  step as ⟨ i', i'_post ⟩
  have hi' : i'.val = 2 * kind.val := by
    rw [i'_post, Nat.shiftLeft_eq, Nat.mod_eq_of_lt (by scalar_tac)]; ring
  step as ⟨ i1, i1_post ⟩
  step as ⟨ i3, i3_post ⟩
  have hi3 : i3.val = 8 * size.val := by
    rw [i3_post, Nat.shiftLeft_eq, Nat.mod_eq_of_lt (by scalar_tac)]; ring
  step as ⟨ header, header_post ⟩
  have hh : header.val = (if last_block then 1 else 0) + 2 * kind.val + 8 * size.val := by
    rw [header_post, i1_post, hiv, hi', hi3]
  step as ⟨ s, back, s_post, s_len, back_post ⟩
  step as ⟨ s1, s1_post, s1_len ⟩
  step as ⟨ s2, s2_post ⟩
  have hb := back_post s2
  have hs2 : s2.val.length = 3 := by rw [s2_post]; exact s1_len
  refine ⟨?_, ?_, ?_⟩
  · simp [Slice.length, hb]
  · intro j hj
    rw [hb, List.getElem!_setSlice!_same _ _ _ _ (Or.inr (by rw [hs2]; omega))]
  · have htake : (back s2).val.take 3 = s1.val := by
      rw [hb, ← hs2, take_setSlice!_zero _ _ (by rw [hs2]; scalar_tac), s2_post]
    rw [htake, s1_post]
    simp only [List.slice_zero_j, Array.to_slice, core.num.U32.to_le_bytes, Slice.from_val,
      Array.from_val, List.map_take, List.map_map]
    have hmap : ((fun (x : U8) => x.val) ∘ (@UScalar.mk UScalarTy.U8)) = BitVec.toNat := by
      funext b; rfl
    rw [hmap, ← List.map_take, le_take_toLEBytes 32 header.bv 3 (by norm_num)]
    have : header.bv.toNat = header.val := rfl
    rw [this, Nat.mod_eq_of_lt (by rw [hh]; split <;> omega), hh]

/-- The three bytes of an array, in order. -/
theorem le_three (bytes : Std.Array U8 3#usize) :
    [bytes[0]!.val, bytes[1]!.val, bytes[2]!.val] = bytes.val.map (·.val) := by
  have hl : bytes.val.length = 3 := by simp
  rcases hc : bytes.val with _ | ⟨a, _ | ⟨b, _ | ⟨c, _ | ⟨d, t⟩⟩⟩⟩ <;>
    simp_all [Array.getElem!_Nat_eq]

/-- The decoder reads back every header the encoder writes: for a block type
below 3, a size of at most 21 bits and at most the frame's `block_max`, `parse`
of the three bytes `write_block_header` wrote returns the same `Last_Block`,
the RFC block type of `kind` and the same `Block_Size`. -/
theorem block_header_round_trip (dst : Slice U8) (kind size : U32) (last_block : Bool)
    (block_max : Usize) (hd : 3 ≤ dst.length) (hk : kind.val ≤ 2) (hs : size.val < 2 ^ 21)
    (hmax : size.val ≤ block_max.val) :
    compressor.core.cctx.write_block_header dst kind size last_block ⦃ (out : Slice U8) =>
      ∀ bytes : Std.Array U8 3#usize, bytes.val = out.val.take 3 →
        BlockHeader.parse bytes block_max ⦃ (r : core.result.Result decompressor.core.block.BlockHeader
            decompressor.error.InvalidDataKind) =>
          ∃ h, r = .Ok h ∧ h.last = last_block ∧ blockType kind.val = some h.kind.rfc ∧
            h.size.val = size.val ⦄ ⦄ := by
  apply WP.spec_mono (compressor.core.cctx.write_block_header.spec dst kind size last_block hd hk hs)
  rintro out ⟨-, -, hle⟩ bytes hbytes
  apply WP.spec_mono (BlockHeader.parse.spec bytes block_max)
  intro r hr
  have hv : le [bytes[0]!.val, bytes[1]!.val, bytes[2]!.val] =
      (if last_block then 1 else 0) + 2 * kind.val + 8 * size.val := by
    rw [le_three, hbytes, ← hle]
  have hlast : (if last_block then 1 else 0) < 2 := by split <;> omega
  have hty : ((if last_block then 1 else 0) + 2 * kind.val + 8 * size.val) / 2 % 4 = kind.val := by
    omega
  have hsz : ((if last_block then 1 else 0) + 2 * kind.val + 8 * size.val) / 8 = size.val := by
    omega
  have hl : decide (((if last_block then 1 else 0) + 2 * kind.val + 8 * size.val) % 2 = 1) =
      last_block := by
    cases last_block <;> simp <;> omega
  simp only [parseBlockHeader, hv, hty, hsz, hl] at hr
  have hk3 : kind.val = 0 ∨ kind.val = 1 ∨ kind.val = 2 := by omega
  rcases hk3 with h | h | h <;>
    simp only [h, blockType, Option.map_some, Nat.not_lt.mpr hmax, ite_false] at hr ⊢ <;>
    obtain ⟨out, hr, h1, h2, h3⟩ := hr <;> exact ⟨out, hr, h1, by rw [h2], h3⟩

end mzstd
