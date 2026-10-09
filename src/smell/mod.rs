//! Architectural smells (U12) — advisory recommendations over the merged graph.
//!
//! A "smell" here is never a verdict (R5): it flags a *candidate* for a human to
//! weigh and surfaces it in a lens's failure-modes section. Nothing in this
//! module changes a fact or can fail the run. In Tier 0 the only smell is the
//! seed cohesion/coupling detector; the catalog grows additively.

pub mod cohesion;

pub use cohesion::cohesion_coupling_notes;
