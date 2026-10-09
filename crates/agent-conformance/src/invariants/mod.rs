//! 8 条跨端口不变量合规套件（`docs/agent-base-design.md` v0.2 §8.2）

pub mod inv1_single_engine;
pub mod inv2_no_global_state;
pub mod inv3_registry_single_source;
pub mod inv4_fail_direction;
pub mod inv5_receipt_structure;
pub mod inv6_event_monotonic_seq;
pub mod inv7_domain_projection_split;
pub mod inv8_cancellation_penetration;

pub use inv1_single_engine::assert_single_engine_executes_turn;
pub use inv2_no_global_state::assert_isolated_runtimes_in_same_process;
pub use inv3_registry_single_source::assert_catalog_single_source_integrity;
pub use inv4_fail_direction::assert_fail_direction_is_consumed;
pub use inv5_receipt_structure::assert_receipt_structure_integrity;
pub use inv6_event_monotonic_seq::assert_events_seq_strictly_monotonic;
pub use inv7_domain_projection_split::assert_domain_projection_split;
pub use inv8_cancellation_penetration::assert_cancellation_penetrates_turn;
