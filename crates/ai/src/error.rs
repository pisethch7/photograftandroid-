//! Every way a brief can be wrong ends here, never in a panic.

/// A rejected brief or design.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// The brief carries a schema version this build does not read.
    #[error("this build reads brief version {expected}, not {got}")]
    Version { got: u32, expected: u32 },
    /// A field is missing, of the wrong type, or outside its range.
    #[error("{field}: {problem}")]
    Field { field: String, problem: String },
    /// A colour is not `#rrggbb`.
    #[error("{field}: {value:?} is not a #rrggbb colour")]
    Color { field: String, value: String },
    /// A size, dpi or preset does not name a usable format.
    #[error("format: {0}")]
    Format(String),
    /// A template id the build does not have.
    #[error("template: {0}")]
    Template(String),
    /// The JSON was not an object-shaped brief at all.
    #[error("brief is not valid JSON for a poster: {0}")]
    Json(String),
    /// The provider's settings, the request, or the model's answer is unusable. Always a sentence
    /// a person can act on; never contains a key.
    #[error("{0}")]
    Provider(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    /// A field error, without repeating `Error::Field { .. }` at every call site.
    pub fn field(field: impl Into<String>, problem: impl Into<String>) -> Self {
        Self::Field { field: field.into(), problem: problem.into() }
    }
}
