//! Audio sample formats shared by the GameCube libraries. Mixing, sequencing and the games'
//! sound engines stay in their owners.

pub mod afc;
/// The output stream that follows the default device (feature `cpal`).
#[cfg(feature = "cpal")]
pub mod out;
