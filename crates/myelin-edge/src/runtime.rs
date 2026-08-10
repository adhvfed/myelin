use std::future::Future;

use tokio::runtime::{Handle, RuntimeFlavor};

use crate::EdgeError;

pub(crate) fn drive_edge_future<F, T>(
    runtime: &Handle,
    future: F,
    subsystem: &str,
) -> Result<T, EdgeError>
where
    F: Future<Output = T>,
{
    match Handle::try_current() {
        Ok(current) if current.runtime_flavor() == RuntimeFlavor::MultiThread => {
            Ok(tokio::task::block_in_place(|| runtime.block_on(future)))
        }
        Ok(_) => Err(EdgeError::Internal(format!(
            "{subsystem} requires the Edge multi-thread runtime"
        ))),
        Err(_) => Ok(runtime.block_on(future)),
    }
}
