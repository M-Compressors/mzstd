module

public import Mathlib.Data.Nat.Log

/-! # RFC 8878, transcribed

The parts of RFC 8878 the proofs state conformance against, written as the RFC
prints them. Nothing here is derived from the Rust code or from the reference C
library. Tables keep the RFC's own columns, so that a transcription error shows
up as a failure of the consistency checks at the end of the file rather than as
a silently wrong specification. Bytes are natural numbers below 256, and a
multi-byte field is read with `le`, the RFC's little-endian convention.
-/

@[expose] public section

namespace Mzstd.Rfc

/-- Little-endian value of a list of bytes (section 2: "little-endian"). -/
def le : List Nat → Nat
  | [] => 0
  | b :: bs => b + 256 * le bs

/-! ## Section 3.1.1: Zstandard frames, and section 3.1.2: skippable frames -/

/-- "Magic_Number: 4 bytes, little-endian format. Value: 0xFD2FB528." -/
def magicNumber : Nat := 0xFD2FB528

/-- "0x184D2A5?, which means any value from 0x184D2A50 to 0x184D2A5F." -/
def skippableMagic (m : Nat) : Prop := 0x184D2A50 ≤ m ∧ m ≤ 0x184D2A5F

instance (m : Nat) : Decidable (skippableMagic m) := by
  unfold skippableMagic; infer_instance

/-! ## Section 3.1.1.1: frame header -/

/-- Table 3, bits 7-6: `Frame_Content_Size_Flag`. -/
def fcsFlag (descriptor : Nat) : Nat := descriptor / 64 % 4

/-- Table 3, bit 5: `Single_Segment_Flag`. -/
def singleSegment (descriptor : Nat) : Bool := descriptor / 32 % 2 = 1

/-- Table 3, bit 3: the reserved bit, which "must be zero". -/
def reservedBit (descriptor : Nat) : Nat := descriptor / 8 % 2

/-- Table 3, bit 2: `Content_Checksum_Flag`. -/
def checksumFlag (descriptor : Nat) : Bool := descriptor / 4 % 2 = 1

/-- Table 3, bits 1-0: `Dictionary_ID_Flag`. -/
def didFlag (descriptor : Nat) : Nat := descriptor % 4

/-- Table 4: `FCS_Field_Size` from `Frame_Content_Size_Flag`; flag 0 gives 1
byte with `Single_Segment_Flag` and none without. -/
def fcsFieldSize (flag : Nat) (single : Bool) : Nat :=
  match flag with
  | 0 => if single then 1 else 0
  | 1 => 2
  | 2 => 4
  | _ => 8

/-- Table 5: `DID_Field_Size` from `Dictionary_ID_Flag`. -/
def didFieldSize (flag : Nat) : Nat := [0, 1, 2, 4].getD flag 0

/-- Section 3.1.1.1.2: "When Single_Segment_Flag is set, Window_Descriptor is
not present." -/
def windowDescriptorSize (single : Bool) : Nat := if single then 0 else 1

/-- Table 2: the header is the descriptor and the three optional fields, whose
sizes "decoding this byte is enough to tell". -/
def headerSize (descriptor : Nat) : Nat :=
  1 + windowDescriptorSize (singleSegment descriptor) + didFieldSize (didFlag descriptor) +
    fcsFieldSize (fcsFlag descriptor) (singleSegment descriptor)

/-- Section 3.1.1.1.2, the window formula:
`windowLog = 10 + Exponent`, `windowBase = 1 << windowLog`,
`windowAdd = (windowBase / 8) * Mantissa`, `Window_Size = windowBase + windowAdd`,
with the exponent in bits 7-3 and the mantissa in bits 2-0. -/
def windowSize (descriptor : Nat) : Nat :=
  let windowLog := 10 + descriptor / 8
  let windowBase := 2 ^ windowLog
  let windowAdd := windowBase / 8 * (descriptor % 8)
  windowBase + windowAdd

/-- Section 3.1.1.1.4: "When FCS_Field_Size is 1, 4, or 8 bytes, the value is
read directly. When FCS_Field_Size is 2, the offset of 256 is added." -/
def frameContentSize (field : List Nat) : Nat :=
  if field.length = 2 then le field + 256 else le field

/-- What a frame header declares. -/
structure FrameHeader where
  windowSize : Nat
  contentSize : Option Nat
  dictionaryId : Nat
  checksum : Bool
deriving DecidableEq

/-- Section 3.1.1.1: reads a frame header (after the magic number) of exactly
the size its descriptor declares; `none` when the size is wrong or the reserved
bit is set. The unused bit 4 is not interpreted. With `Single_Segment_Flag`,
"Window_Size is Frame_Content_Size". -/
def parseFrameHeader (header : List Nat) : Option FrameHeader :=
  match header with
  | [] => none
  | descriptor :: _ =>
    if header.length ≠ headerSize descriptor ∨ reservedBit descriptor = 1 then none
    else
      let single := singleSegment descriptor
      let didStart := 1 + windowDescriptorSize single
      let fcsStart := didStart + didFieldSize (didFlag descriptor)
      let did := (header.drop didStart).take (didFieldSize (didFlag descriptor))
      let fcs := header.drop fcsStart
      let contentSize := if fcs = [] then none else some (frameContentSize fcs)
      some {
        windowSize := if single then contentSize.getD 0 else windowSize (header.getD 1 0)
        contentSize
        dictionaryId := le did
        checksum := checksumFlag descriptor }

/-! ## Section 3.1.1.2: blocks -/

/-- Table 10: the block types; value 3 is reserved and is "not a block". -/
inductive BlockType where
  | raw
  | rle
  | compressed
deriving DecidableEq

/-- Table 10, the value of each type. -/
def blockTypeValue : BlockType → Nat
  | .raw => 0
  | .rle => 1
  | .compressed => 2

/-- Table 10 read back; `none` for the reserved value. -/
def blockType (value : Nat) : Option BlockType :=
  match value with
  | 0 => some .raw
  | 1 => some .rle
  | 2 => some .compressed
  | _ => none

/-- A block header's three fields. -/
structure BlockHeader where
  last : Bool
  type : BlockType
  size : Nat
deriving DecidableEq

/-- Table 9: `Last_Block` in bit 0, `Block_Type` in bits 1-2 and `Block_Size`
in bits 3-23 of the three-byte little-endian header. -/
def parseBlockHeader (b0 b1 b2 : Nat) : Option BlockHeader :=
  let value := le [b0, b1, b2]
  (blockType (value / 2 % 4)).map fun type => { last := value % 2 = 1, type, size := value / 8 }

/-- Section 3.1.1.2.4: "Block_Maximum_Size, which is the smallest of
Window_Size [and] 128 KB". -/
def blockMaximumSize (windowSize : Nat) : Nat := min windowSize (128 * 1024)

/-- Section 3.1.1.2.3: the bytes of `Block_Content`; an `RLE_Block` holds a
single byte. -/
def blockContentSize (header : BlockHeader) : Nat :=
  if header.type = .rle then 1 else header.size

/-! ## Section 3.1.1.3: compressed blocks -/

/-- Table 15 (and, with the same values, the four `Literals_Block_Type`s of
section 3.1.1.3.1.1). -/
inductive CompressionMode where
  | predefined
  | rle
  | fseCompressed
  | repeat
deriving DecidableEq

/-- Table 15, a two-bit mode field. -/
def compressionMode (bits : Nat) : CompressionMode :=
  match bits % 4 with
  | 0 => .predefined
  | 1 => .rle
  | 2 => .fseCompressed
  | _ => .repeat

/-- Section 3.1.1.3.2.1, `Number_of_Sequences` at the start of `bytes`, and
the number of bytes it uses; `none` when the bytes it needs are missing. -/
def numberOfSequences : List Nat → Option (Nat × Nat)
  | [] => none
  | [byte0] => if byte0 < 128 then some (byte0, 1) else none
  | [byte0, byte1] =>
    if byte0 < 128 then some (byte0, 1)
    else if byte0 < 255 then some ((byte0 - 128) * 256 + byte1, 2)
    else none
  | byte0 :: byte1 :: byte2 :: _ =>
    if byte0 < 128 then some (byte0, 1)
    else if byte0 < 255 then some ((byte0 - 128) * 256 + byte1, 2)
    else some (byte1 + byte2 * 256 + 0x7F00, 3)

/-- A row of a length code table: the code's `Baseline` and `Number_of_Bits`. -/
structure Code where
  baseline : Nat
  bits : Nat
deriving DecidableEq

/-- Table 16, literals length codes 0 to 35 ("0-15: length, 0"). -/
def literalsLengthCodes : List Code :=
  (List.range 16).map (fun c => ⟨c, 0⟩) ++ [
    ⟨16, 1⟩, ⟨18, 1⟩, ⟨20, 1⟩, ⟨22, 1⟩, ⟨24, 2⟩, ⟨28, 2⟩, ⟨32, 3⟩, ⟨40, 3⟩,
    ⟨48, 4⟩, ⟨64, 6⟩, ⟨128, 7⟩, ⟨256, 8⟩, ⟨512, 9⟩, ⟨1024, 10⟩, ⟨2048, 11⟩,
    ⟨4096, 12⟩, ⟨8192, 13⟩, ⟨16384, 14⟩, ⟨32768, 15⟩, ⟨65536, 16⟩]

/-- Table 17, match length codes 0 to 52 ("0-31: Match_Length_Code + 3, 0"). -/
def matchLengthCodes : List Code :=
  (List.range 32).map (fun c => ⟨c + 3, 0⟩) ++ [
    ⟨35, 1⟩, ⟨37, 1⟩, ⟨39, 1⟩, ⟨41, 1⟩, ⟨43, 2⟩, ⟨47, 2⟩, ⟨51, 3⟩, ⟨59, 3⟩,
    ⟨67, 4⟩, ⟨83, 4⟩, ⟨99, 5⟩, ⟨131, 7⟩, ⟨259, 8⟩, ⟨515, 9⟩, ⟨1027, 10⟩,
    ⟨2051, 11⟩, ⟨4099, 12⟩, ⟨8195, 13⟩, ⟨16387, 14⟩, ⟨32771, 15⟩, ⟨65539, 16⟩]

/-- Row `code` of a table; codes past the end read as an empty row. -/
def code (table : List Code) (c : Nat) : Code := table.getD c ⟨0, 0⟩

/-- Whether `code c` of `table` holds `length`: the baseline plus a field of
`Number_of_Bits` bits reaches it. -/
def holds (table : List Code) (c length : Nat) : Prop :=
  (code table c).baseline ≤ length ∧ length < (code table c).baseline + 2 ^ (code table c).bits

instance (table : List Code) (c length : Nat) : Decidable (holds table c length) := by
  unfold holds; infer_instance

/-- Section 3.1.1.3.2.1.1: "Offset_Value = (1 << offsetCode) +
readNBits(offsetCode)". -/
def offsetValue (offsetCode extra : Nat) : Nat := 2 ^ offsetCode + extra

/-- Section 3.1.1.3.2.1.1: "if (Offset_Value > 3) Offset = Offset_Value - 3". -/
def offset (offsetValue : Nat) : Nat := offsetValue - 3

/-- Section 3.1.1.3.2.2: a default distribution and its accuracy log. -/
structure Distribution where
  probabilities : List Int
  accuracyLog : Nat

/-- Section 3.1.1.3.2.2.1, `literalsLength_defaultDistribution[36]`, accuracy
log 6. -/
def literalsLengthDefault : Distribution :=
  ⟨[4, 3, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 1, 1, 1,
    2, 2, 2, 2, 2, 2, 2, 2, 2, 3, 2, 1, 1, 1, 1, 1,
    -1, -1, -1, -1], 6⟩

/-- Section 3.1.1.3.2.2.2, `matchLengths_defaultDistribution[53]`, accuracy
log 6. -/
def matchLengthDefault : Distribution :=
  ⟨[1, 4, 3, 2, 2, 2, 2, 2, 2, 1, 1, 1, 1, 1, 1, 1,
    1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
    1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, -1, -1,
    -1, -1, -1, -1, -1], 6⟩

/-- Section 3.1.1.3.2.2.3, `offsetCodes_defaultDistribution[29]`, accuracy
log 5. -/
def offsetDefault : Distribution :=
  ⟨[1, 1, 1, 1, 1, 1, 2, 2, 2, 1, 1, 1, 1, 1, 1, 1,
    1, 1, 1, 1, 1, 1, 1, 1, -1, -1, -1, -1, -1], 5⟩

/-- Section 4.1.1: a probability of -1 ("less than 1") "counts as 1". -/
def points (p : Int) : Nat := if p = -1 then 1 else p.toNat

/-! ## Section 3.1.1.5: repeat offsets -/

/-- `Repeated_Offset1`, `Repeated_Offset2` and `Repeated_Offset3`, most recent
first. -/
structure Repeats where
  r1 : Nat
  r2 : Nat
  r3 : Nat
deriving DecidableEq

/-- "the starting offset history is populated with the following values:
Repeated_Offset1 (1), Repeated_Offset2 (4), and Repeated_Offset3 (8)". -/
def initialRepeats : Repeats := ⟨1, 4, 8⟩

/-- The offset a sequence uses: `Offset_Value - 3` above 3; otherwise the
repeated offset it names, shifted by one when `literals_length` is 0, where 3
"means Repeated_Offset1 - 1_byte". -/
def usedOffset (reps : Repeats) (offsetValue : Nat) (literalsLengthZero : Bool) : Nat :=
  if offsetValue > 3 then offset offsetValue
  else if literalsLengthZero then
    match offsetValue with
    | 1 => reps.r2
    | 2 => reps.r3
    | _ => reps.r1 - 1
  else
    match offsetValue with
    | 1 => reps.r1
    | 2 => reps.r2
    | _ => reps.r3

/-- The update after a sequence. When the offset value does not refer to a
repeated offset (above 3, or 3 with a zero `literals_length`), the offsets "are
shifted back one, and Repeated_Offset1 takes on the value of the offset that
was just used". Otherwise the used one moves to the front and those before it
are "pushed back from the first Repeated_Offset through to the Repeated_Offset
selected". -/
def updateRepeats (reps : Repeats) (offsetValue : Nat) (literalsLengthZero : Bool) : Repeats :=
  let used := usedOffset reps offsetValue literalsLengthZero
  -- Which repeated offset is used: 1, 2 or 3, or 0 for none.
  let selected :=
    if offsetValue > 3 then 0
    else if literalsLengthZero then (if offsetValue = 3 then 0 else offsetValue + 1)
    else offsetValue
  match selected with
  | 1 => reps
  | 2 => ⟨used, reps.r1, reps.r3⟩
  | 3 => ⟨used, reps.r1, reps.r2⟩
  | _ => ⟨used, reps.r1, reps.r2⟩

/-! ## Section 4.1.1: FSE table description -/

/-- The smallest power of two at least `p`: "The next power of 2". -/
def nextPowerOfTwo (p : Nat) : Nat := 2 ^ Nat.clog 2 p

/-- Table 21: the `Number_of_Bits` and `Baseline` of the state of order `k`
(states of a symbol sorted in natural order) of a symbol with probability `p`,
in a table of `2 ^ accuracyLog` states. The space is divided into
`nextPowerOfTwo p` shares; the `nextPowerOfTwo p - p` lowest states count
double and need one more bit. "Baseline is assigned starting from the higher
states using fewer bits, and proceeding naturally, then resuming at the first
state, each taking its allocated width from Baseline." -/
def fseState (accuracyLog p k : Nat) : Nat × Nat :=
  let share := 2 ^ accuracyLog / nextPowerOfTwo p
  let doubled := nextPowerOfTwo p - p
  if k < doubled then
    (accuracyLog - Nat.clog 2 p + 1, (p - doubled) * share + 2 * k * share)
  else
    (accuracyLog - Nat.clog 2 p, (k - doubled) * share)

/-! ## Consistency of the transcription -/

theorem literalsLengthCodes_length : literalsLengthCodes.length = 36 := by decide
theorem matchLengthCodes_length : matchLengthCodes.length = 53 := by decide

/-- Each length code starts where the range of the previous one ends, so the
codes cover their lengths without gaps or overlaps. -/
theorem literalsLengthCodes_contiguous :
    ∀ c < 35, (code literalsLengthCodes c).baseline + 2 ^ (code literalsLengthCodes c).bits =
      (code literalsLengthCodes (c + 1)).baseline := by decide

theorem matchLengthCodes_contiguous :
    ∀ c < 52, (code matchLengthCodes c).baseline + 2 ^ (code matchLengthCodes c).bits =
      (code matchLengthCodes (c + 1)).baseline := by decide

/-- "They define lengths from 0 to 131071 bytes." -/
theorem literalsLengthCodes_span :
    (code literalsLengthCodes 0).baseline = 0 ∧
      (code literalsLengthCodes 35).baseline + 2 ^ (code literalsLengthCodes 35).bits - 1 = 131071 := by
  decide

/-- "They define lengths from 3 to 131074 bytes." -/
theorem matchLengthCodes_span :
    (code matchLengthCodes 0).baseline = 3 ∧
      (code matchLengthCodes 52).baseline + 2 ^ (code matchLengthCodes 52).bits - 1 = 131074 := by
  decide

/-- Each default distribution sums to its table size, counting -1 as 1. -/
theorem literalsLengthDefault_sum :
    (literalsLengthDefault.probabilities.map points).sum = 2 ^ literalsLengthDefault.accuracyLog := by
  decide

theorem matchLengthDefault_sum :
    (matchLengthDefault.probabilities.map points).sum = 2 ^ matchLengthDefault.accuracyLog := by
  decide

theorem offsetDefault_sum :
    (offsetDefault.probabilities.map points).sum = 2 ^ offsetDefault.accuracyLog := by decide

/-- The offset default "supports a maximum N value of 28, allowing offset
values up to 536,870,908". -/
theorem offsetDefault_max :
    offsetDefault.probabilities.length = 29 ∧ offset (offsetValue 28 (2 ^ 28 - 1)) = 536870908 := by
  decide

/-- The worked example of Table 18, row by row. RFC 8878 prints the last two
rows as offset value 1 giving `2221, 2222, 1111` and offset value 1 giving
`2222, 2221, 3333`; neither follows from its own text, and the current Zstandard
format document corrects them to offset value 3 (`Repeated_Offset1 - 1` with a
zero literals length) and `2222, 2221, 1111`. The rows below are the corrected
ones, which the prose above produces. -/
theorem updateRepeats_table18 :
    let s0 := initialRepeats
    let s1 := updateRepeats s0 1114 false
    let s2 := updateRepeats s1 1 false
    let s3 := updateRepeats s2 2225 false
    let s4 := updateRepeats s3 1114 false
    let s5 := updateRepeats s4 3336 false
    let s6 := updateRepeats s5 2 false
    let s7 := updateRepeats s6 3 false
    let s8 := updateRepeats s7 3 true
    let s9 := updateRepeats s8 1 true
    s1 = ⟨1111, 1, 4⟩ ∧ s2 = ⟨1111, 1, 4⟩ ∧ s3 = ⟨2222, 1111, 1⟩ ∧ s4 = ⟨1111, 2222, 1111⟩ ∧
      s5 = ⟨3333, 1111, 2222⟩ ∧ s6 = ⟨1111, 3333, 2222⟩ ∧ s7 = ⟨2222, 1111, 3333⟩ ∧
      s8 = ⟨2221, 2222, 1111⟩ ∧ s9 = ⟨2222, 2221, 1111⟩ := by
  decide

/-- The worked example of Table 21: probability 5 in a table of 128 states. -/
theorem fseState_table21 :
    (List.range 5).map (fseState 7 5) = [(5, 32), (5, 64), (5, 96), (4, 0), (4, 16)] := by
  decide

end Mzstd.Rfc

end
