//! Run an async closure on a dedicated Tokio runtime with the [`go!`] macro.
//!
//! Runtimes are initialized lazily and reused by profile. A timeout limits how
//! long the caller waits for the result; it does not abort the spawned task.

use std::fmt;
use std::time::Duration;

/// Configuration for a [`go!`] invocation.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Context {
    /// Selects the dedicated runtime used to execute the spawned task.
    ///
    /// Each of the 256 possible profiles is initialized at most once and then
    /// reused for subsequent invocations.
    pub profile: u8,
    /// Limits how long to wait for the task to send its result.
    ///
    /// [`Duration::ZERO`] disables the deadline. A positive timeout stops
    /// waiting but does not abort the detached task.
    pub timeout: Duration,
}

/// An error returned while starting or waiting for a [`go!`] task.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GoError {
    /// The configured deadline elapsed before the task sent a result.
    Timeout,
    /// Tokio could not initialize the dedicated runtime for a profile.
    RuntimeInitialization {
        /// The profile whose runtime failed to initialize.
        profile: u8,
        /// The underlying runtime-builder error.
        message: String,
    },
    /// The spawned task ended without sending a result.
    ///
    /// This includes explicitly dropping the sender and task termination such
    /// as a panic.
    TaskTerminated,
}

impl fmt::Display for GoError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Timeout => formatter.write_str("the go task timed out"),
            Self::RuntimeInitialization { profile, message } => {
                write!(
                    formatter,
                    "failed to initialize runtime for profile {profile}: {message}"
                )
            }
            Self::TaskTerminated => {
                formatter.write_str("the go task terminated without sending a result")
            }
        }
    }
}

impl std::error::Error for GoError {}

/// Common imports retained for compatibility with `tokio-go` 0.1.
pub mod prelude {
    pub use crate::{Context, GoError};
    pub use std::time::Duration;
    pub use tokio::sync::oneshot::Sender;
    pub use tokio::time::sleep;
}

/// Implementation details used by the exported [`go!`] macro.
///
/// This module is public only because macros expand in the downstream crate.
#[doc(hidden)]
pub mod __private {
    use super::GoError;
    use std::future::Future;
    use std::sync::OnceLock;
    use tokio::runtime::Runtime;
    use tokio::sync::oneshot;

    pub use std::time::Duration;
    pub use tokio::sync::oneshot::Sender;

    type RuntimeSlot = OnceLock<Result<Runtime, String>>;

    static RUNTIMES: OnceLock<[RuntimeSlot; 256]> = OnceLock::new();

    fn runtimes() -> &'static [RuntimeSlot; 256] {
        RUNTIMES.get_or_init(|| std::array::from_fn(|_| OnceLock::new()))
    }

    fn runtime(profile: u8) -> Result<&'static Runtime, GoError> {
        let runtime = runtimes()[profile as usize]
            .get_or_init(|| Runtime::new().map_err(|error| error.to_string()));

        match runtime {
            Ok(runtime) => Ok(runtime),
            Err(message) => Err(GoError::RuntimeInitialization {
                profile,
                message: message.clone(),
            }),
        }
    }

    pub async fn run<T, Build, Task>(
        profile: u8,
        timeout: Duration,
        build: Build,
    ) -> Result<T, GoError>
    where
        T: Send + 'static,
        Build: FnOnce(Sender<T>) -> Task,
        Task: Future<Output = ()> + Send + 'static,
    {
        let (sender, receiver) = oneshot::channel();

        {
            let runtime = runtime(profile)?;
            runtime.spawn(build(sender));
        }

        if timeout.is_zero() {
            receiver.await.map_err(|_| GoError::TaskTerminated)
        } else {
            match tokio::time::timeout(timeout, receiver).await {
                Ok(Ok(value)) => Ok(value),
                Ok(Err(_)) => Err(GoError::TaskTerminated),
                Err(_) => Err(GoError::Timeout),
            }
        }
    }
}

/// Runs an async closure on the default or a profile-specific Tokio runtime.
///
/// The form without a [`Context`] uses profile `0` with no timeout:
///
/// ```
/// use tokio_go::go;
///
/// # #[tokio::main]
/// # async fn main() -> Result<(), tokio_go::GoError> {
/// let value = go!(|sender: Sender<i32>| async move {
///     let _ = sender.send(1);
/// })
/// .await?;
/// assert_eq!(value, 1);
/// # Ok(())
/// # }
/// ```
///
/// Pass a [`Context`] to select a profile and deadline. Macro internals are
/// hygienic, so the `Sender` token in the call form does not require importing
/// this crate's prelude.
///
/// ```
/// use std::time::Duration;
/// use tokio_go::{go, Context, GoError};
///
/// # #[tokio::main]
/// # async fn main() {
/// let result = go!(
///     |sender: Sender<()>| async move {
///         std::future::pending::<()>().await;
///         let _ = sender.send(());
///     },
///     Context {
///         profile: 1,
///         timeout: Duration::from_millis(1),
///     }
/// )
/// .await;
/// assert_eq!(result, Err(GoError::Timeout));
/// # }
/// ```
///
/// A positive timeout only stops waiting for the result. The spawned task is
/// detached and continues running on its profile runtime.
#[macro_export]
macro_rules! go {
    (|$sender:ident : Sender<$output:ty>|$task:expr) => {
        $crate::__private::run(
            0,
            $crate::__private::Duration::ZERO,
            |$sender: $crate::__private::Sender<$output>| $task,
        )
    };
    (|$sender:ident : Sender<$output:ty>|$task:expr,$context:expr) => {{
        let context = $context;
        $crate::__private::run(
            context.profile,
            context.timeout,
            |$sender: $crate::__private::Sender<$output>| $task,
        )
    }};
}
