//! The material vocabulary `ashlar` and `ashlar-material` share.
//!
//! A building recipe binds a slot to a material key. A material graph declares
//! parameters and bakes into texture sets. What joins the two is a
//! [`MaterialDefinition`]: constants, a repeat size, and a [`Surface`] that
//! says whether the varying detail is nothing, files, a bake of a named graph
//! or a shader compiled from one. This crate is that join and nothing else, so
//! that both halves can name it without either naming the other.
//!
//! It holds no graph and no geometry. A definition names a graph by content
//! key and a [`Binding`] names a definition the same way; whatever holds both
//! libraries checks one against the other.
//!
//! `ashlar` and `ashlar-material` both re-export every name here, so a caller
//! of either writes the paths it always wrote.

mod material;

pub use material::{
    Bake, Binding, BoundSlots, MaterialDefinition, MaterialLibrary, ParamValue, StrandSettings,
    Surface,
};

/// A validation error with a path suitable for an editor or command-line diagnostic.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
#[error("{path}: {reason}")]
pub struct ValidationError {
    /// Location within the document, including the relevant part, instance or material.
    pub path: String,
    /// The violated invariant.
    pub reason: String,
}

/// `Ok` where `condition` holds, and otherwise the error at `path` for `reason`.
///
/// Public because the crates above this one report their own invariants in the
/// same shape, and one helper is what keeps the shape one shape.
pub fn require(condition: bool, path: &str, reason: &str) -> Result<(), ValidationError> {
    if condition {
        Ok(())
    } else {
        Err(ValidationError {
            path: path.into(),
            reason: reason.into(),
        })
    }
}

/// Refuse a blank name or content key.
pub fn name(value: &str, path: &str) -> Result<(), ValidationError> {
    require(!value.trim().is_empty(), path, "name must not be blank")
}
