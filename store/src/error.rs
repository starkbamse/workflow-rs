//!
//!  Errors produced by this crate.
//! 

use base64::DecodeError;
use thiserror::Error;
use wasm_bindgen::prelude::*;

#[derive(Error, Debug)]
pub enum Error {
    #[error("I/O error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("JavaScript error: {0}")]
    JsValue(String),

    #[error("Base64 decode error: {0}")]
    DecodeError(DecodeError),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Not a string: {0}")]
    DataIsNotAString(String),

    #[error(transparent)]
    SerdeJson(#[from] serde_json::Error),
}

impl From<Error> for JsValue {
    fn from(err: Error) -> Self {
        JsValue::from_str(&err.to_string())
    }
}

impl From<JsValue> for Error {
    fn from(error: JsValue) -> Error {
        Error::JsValue(format!("{error:?}"))
    }
}

impl From<DecodeError> for Error {
    fn from(error: DecodeError) -> Error {
        Error::DecodeError(error)
    }
}
