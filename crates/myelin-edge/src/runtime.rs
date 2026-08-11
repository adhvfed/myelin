use std::future::Future;

use tokio::runtime::{Handle, RuntimeFlavor};

use crate::EdgeError;

pub(crate) fn drive_future_on_runtime<F, T, E>(
    runtime: &Handle,
    future: F,
    current_thread_error: E,
) -> Result<T, E>
where
    F: Future<Output = T>,
{
    match Handle::try_current() {
        Ok(current) if current.runtime_flavor() == RuntimeFlavor::MultiThread => {
            Ok(tokio::task::block_in_place(|| runtime.block_on(future)))
        }
        Ok(_) => Err(current_thread_error),
        Err(_) => Ok(runtime.block_on(future)),
    }
}

pub(crate) fn drive_result_on_runtime<F, T, E>(
    runtime: &Handle,
    future: F,
    current_thread_error: E,
) -> Result<T, E>
where
    F: Future<Output = Result<T, E>>,
{
    drive_future_on_runtime(runtime, future, current_thread_error)?
}

pub(crate) fn drive_edge_future<F, T>(
    runtime: &Handle,
    future: F,
    subsystem: &str,
) -> Result<T, EdgeError>
where
    F: Future<Output = T>,
{
    drive_future_on_runtime(
        runtime,
        future,
        EdgeError::Internal(format!(
            "{subsystem} requires the Edge multi-thread runtime"
        )),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drives_a_future_from_a_synchronous_handler() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();

        assert_eq!(
            drive_future_on_runtime(runtime.handle(), async { 42 }, "wrong runtime"),
            Ok(42)
        );
    }

    #[test]
    fn drives_a_result_from_the_edge_runtime() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let handle = runtime.handle().clone();

        runtime.block_on(async move {
            assert_eq!(
                drive_result_on_runtime(
                    &handle,
                    async { Ok::<_, &'static str>(42) },
                    "wrong runtime",
                ),
                Ok(42)
            );
        });
    }

    #[test]
    fn refuses_to_block_a_current_thread_runtime() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let current = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let target = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let polled = std::sync::Arc::new(AtomicBool::new(false));
        let polled_by_future = polled.clone();

        let result = current.block_on(async {
            drive_future_on_runtime(
                target.handle(),
                async move {
                    polled_by_future.store(true, Ordering::SeqCst);
                    42
                },
                "wrong runtime",
            )
        });

        assert_eq!(result, Err("wrong runtime"));
        assert!(!polled.load(Ordering::SeqCst));
    }
}
