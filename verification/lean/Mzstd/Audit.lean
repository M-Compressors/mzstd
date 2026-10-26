import Mzstd.SequenceCodes
import Mzstd.RepeatOffsets
import Mzstd.Fse
import Mzstd.BlockHeader
import Mzstd.SequenceCount
import Mzstd.FrameRoundTrip

/-! # Axiom audit

The headline theorems may rely only on Lean's standard axioms. A `sorry`, a
`native_decide` or an unmodelled standard-library item would add an axiom to
these lists and fail the build.
-/

/-- info: 'mzstd.LL_BASE.spec' depends on axioms: [propext, Classical.choice, Quot.sound] -/
#guard_msgs in
#print axioms mzstd.LL_BASE.spec

/-- info: 'mzstd.ML_BASE.spec' depends on axioms: [propext, Classical.choice, Quot.sound] -/
#guard_msgs in
#print axioms mzstd.ML_BASE.spec

/-- info: 'mzstd.OF_BITS.spec' depends on axioms: [propext, Classical.choice, Quot.sound] -/
#guard_msgs in
#print axioms mzstd.OF_BITS.spec

/-- info: 'mzstd.OF_BASE.spec' depends on axioms: [propext, Classical.choice, Quot.sound] -/
#guard_msgs in
#print axioms mzstd.OF_BASE.spec

/-- info: 'mzstd.LL_DEFAULT_NORM_rfc' depends on axioms: [propext, Classical.choice, Quot.sound] -/
#guard_msgs in
#print axioms mzstd.LL_DEFAULT_NORM_rfc

/-- info: 'mzstd.ML_DEFAULT_NORM_rfc' depends on axioms: [propext, Classical.choice, Quot.sound] -/
#guard_msgs in
#print axioms mzstd.ML_DEFAULT_NORM_rfc

/-- info: 'mzstd.OF_DEFAULT_NORM_rfc' depends on axioms: [propext, Classical.choice, Quot.sound] -/
#guard_msgs in
#print axioms mzstd.OF_DEFAULT_NORM_rfc

/-- info: 'mzstd.REP_START_rfc' depends on axioms: [propext, Classical.choice, Quot.sound] -/
#guard_msgs in
#print axioms mzstd.REP_START_rfc

/-- info: 'mzstd.magic_rfc' depends on axioms: [propext, Classical.choice, Quot.sound] -/
#guard_msgs in
#print axioms mzstd.magic_rfc

/--
info: 'mzstd.decompressor.core.literals.encoding_type.spec' depends on axioms: [propext, Classical.choice, Quot.sound]
-/
#guard_msgs in
#print axioms mzstd.decompressor.core.literals.encoding_type.spec

/--
info: 'mzstd.literals_length_round_trip' depends on axioms: [propext, Classical.choice, Quot.sound]
-/
#guard_msgs in
#print axioms mzstd.literals_length_round_trip

/--
info: 'mzstd.match_length_round_trip' depends on axioms: [propext, Classical.choice, Quot.sound]
-/
#guard_msgs in
#print axioms mzstd.match_length_round_trip

/-- info: 'mzstd.update_rep.spec' depends on axioms: [propext, Classical.choice, Quot.sound] -/
#guard_msgs in
#print axioms mzstd.update_rep.spec

/-- info: 'mzstd.transition.spec' depends on axioms: [propext, Classical.choice, Quot.sound] -/
#guard_msgs in
#print axioms mzstd.transition.spec

/--
info: 'mzstd.BlockHeader.parse.spec' depends on axioms: [propext, Classical.choice, Quot.sound]
-/
#guard_msgs in
#print axioms mzstd.BlockHeader.parse.spec

/--
info: 'mzstd.block_header_round_trip' depends on axioms: [propext, Classical.choice, Quot.sound]
-/
#guard_msgs in
#print axioms mzstd.block_header_round_trip

/--
info: 'mzstd.sequence_count_round_trip' depends on axioms: [propext, Classical.choice, Quot.sound]
-/
#guard_msgs in
#print axioms mzstd.sequence_count_round_trip

/-- info: 'mzstd.numberOfSequences_shortest' depends on axioms: [propext, Quot.sound] -/
#guard_msgs in
#print axioms mzstd.numberOfSequences_shortest

/--
info: 'mzstd.FrameHeader.parse.spec' depends on axioms: [propext, Classical.choice, Quot.sound]
-/
#guard_msgs in
#print axioms mzstd.FrameHeader.parse.spec

/--
info: 'mzstd.write_frame_header_rfc' depends on axioms: [propext, Classical.choice, Quot.sound]
-/
#guard_msgs in
#print axioms mzstd.write_frame_header_rfc

/--
info: 'mzstd.frame_header_round_trip' depends on axioms: [propext, Classical.choice, Quot.sound]
-/
#guard_msgs in
#print axioms mzstd.frame_header_round_trip

/-- info: 'Mzstd.Rfc.updateRepeats_table18' does not depend on any axioms -/
#guard_msgs in
#print axioms Mzstd.Rfc.updateRepeats_table18

/-- info: 'Mzstd.Rfc.fseState_table21' does not depend on any axioms -/
#guard_msgs in
#print axioms Mzstd.Rfc.fseState_table21
