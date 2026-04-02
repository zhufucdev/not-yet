use thiserror::Error;

#[derive(Error, Debug)]
pub enum ParseHeaderError {
    #[error(
        "invalid header format, this data should have been verified before getting into database"
    )]
    FormatError,
    #[error("invalid header: {0}")]
    InvalidPair(String),
    #[error("HTTP IO error: {0}")]
    Io(#[from] reqwest::Error)
}
