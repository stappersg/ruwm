use core::fmt::{Debug, Display};

use embedded_svc::errors;

pub type Result<T> = anyhow::Result<T>;
pub type Error = anyhow::Error;

pub trait HalError: Debug {}

impl<E> HalError for E where E: Debug {}

pub fn svc(e: impl errors::Error) -> Error {
    anyhow::anyhow!(e)
}

pub fn hal(e: impl HalError) -> Error {
    debug(e)
}

pub fn display(e: impl Display) -> Error {
    anyhow::anyhow!("Error: {}", e)
}

pub fn debug(e: impl Debug) -> Error {
    anyhow::anyhow!("Error: {:?}", e)
}
