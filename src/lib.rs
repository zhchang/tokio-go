//! Run owned async work immediately on a reusable, profile-dedicated Tokio
//! runtime with the [`go!`] macro.
//!
//! The preferred form returns the async block's value directly:
//!
//! ```
//! use tokio_go::go;
//!
//! # #[tokio::main]
//! # async fn main() -> Result<(), tokio_go::GoError> {
//! let value = go!(async move { 42 }).await?;
//! assert_eq!(value, 42);
//! # Ok(())
//! # }
//! ```
//!
//! Calling [`go!`] schedules work immediately. Awaiting the returned
//! [`GoTask`] only waits for its result. Dropping the handle, calling
//! [`GoTask::detach`], or reaching a configured timeout leaves the spawned
//! work running; [`GoTask::abort`] is the only handle-driven cancellation
//! operation.

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context as TaskContext, Poll};
use std::time::Duration;

/// Configuration for a [`go!`] invocation.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Context {
    /// Selects the dedicated runtime used to execute the spawned task.
    ///
    /// Each of the 256 possible profiles is initialized at most once and then
    /// reused for subsequent invocations.
    pub profile: u8,
    /// Limits how long polling the task handle waits for a result.
    ///
    /// [`Duration::ZERO`] disables the deadline. A positive timeout stops
    /// waiting but does not abort the detached task.
    pub timeout: Duration,
}

impl Context {
    /// Creates a context for profile `0` with no timeout.
    pub const fn new() -> Self {
        Self {
            profile: 0,
            timeout: Duration::ZERO,
        }
    }

    /// Selects the dedicated runtime profile.
    pub const fn profile(mut self, profile: u8) -> Self {
        self.profile = profile;
        self
    }

    /// Sets how long polling the task handle waits for its result.
    pub const fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

/// An error returned while starting or waiting for a [`go!`] task.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GoError {
    /// The configured deadline elapsed before the task produced a result.
    Timeout,
    /// Tokio could not initialize the dedicated runtime for a profile.
    RuntimeInitialization {
        /// The profile whose runtime failed to initialize.
        profile: u8,
        /// The underlying runtime-builder error.
        message: String,
    },
    /// The spawned task ended without producing its expected result.
    ///
    /// This includes an explicitly aborted task, a task panic, and a legacy
    /// sender being dropped before it sends a value.
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

type WaitFuture<T> = Pin<Box<dyn Future<Output = Result<T, GoError>> + Send + 'static>>;

/// A handle to work scheduled by [`go!`].
///
/// `GoTask<T>` implements `Future<Output = Result<T, GoError>>`. Work is
/// already running when the handle is returned; polling only waits for the
/// result or the configured timeout.
///
/// Dropping a `GoTask`, calling [`detach`](Self::detach), or receiving
/// [`GoError::Timeout`] does not cancel the spawned work. Call
/// [`abort`](Self::abort) when cancellation is explicitly intended.
#[must_use = "GoTask starts immediately; await it, detach it, abort it, or explicitly drop it"]
pub struct GoTask<T> {
    wait: Option<WaitFuture<T>>,
    abort_handle: Option<tokio::task::AbortHandle>,
    initialization_error: Option<GoError>,
}

impl<T> GoTask<T> {
    fn running(wait: WaitFuture<T>, abort_handle: tokio::task::AbortHandle) -> Self {
        Self {
            wait: Some(wait),
            abort_handle: Some(abort_handle),
            initialization_error: None,
        }
    }

    fn failed(error: GoError) -> Self {
        Self {
            wait: None,
            abort_handle: None,
            initialization_error: Some(error),
        }
    }

    /// Explicitly cancels the spawned task.
    ///
    /// If cancellation prevents the task from producing its expected result,
    /// awaiting the handle returns [`GoError::TaskTerminated`]. Calling this
    /// method after the task has completed has no effect.
    pub fn abort(&self) {
        if let Some(abort_handle) = &self.abort_handle {
            abort_handle.abort();
        }
    }

    /// Discards this result handle while leaving spawned work running.
    ///
    /// A synchronous runtime-initialization error is returned because no task
    /// was started in that case. Otherwise the task is detached and this
    /// method returns `Ok(())`.
    pub fn detach(mut self) -> Result<(), GoError> {
        if let Some(error) = self.initialization_error.take() {
            return Err(error);
        }

        self.wait.take();
        self.abort_handle.take();
        Ok(())
    }
}

impl<T> Future for GoTask<T> {
    type Output = Result<T, GoError>;

    fn poll(mut self: Pin<&mut Self>, task_context: &mut TaskContext<'_>) -> Poll<Self::Output> {
        let this = self.as_mut().get_mut();

        if let Some(error) = this.initialization_error.take() {
            return Poll::Ready(Err(error));
        }

        let wait = this.wait.as_mut().expect("GoTask polled after completing");
        match wait.as_mut().poll(task_context) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(result) => {
                this.wait = None;
                this.abort_handle = None;
                Poll::Ready(result)
            }
        }
    }
}

/// Common imports retained for compatibility with `tokio-go` 0.2.
pub mod prelude {
    pub use crate::{Context, GoError, GoTask};
    pub use std::time::Duration;
    pub use tokio::sync::oneshot::Sender;
    pub use tokio::time::sleep;
}

/// Implementation details used by the exported [`go!`] macro.
///
/// This module is public only because macros expand in the downstream crate.
#[doc(hidden)]
pub mod __private {
    use super::{GoError, GoTask, WaitFuture};
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

    fn direct_wait<T>(join_handle: tokio::task::JoinHandle<T>, timeout: Duration) -> WaitFuture<T>
    where
        T: Send + 'static,
    {
        Box::pin(async move {
            if timeout.is_zero() {
                join_handle.await.map_err(|_| GoError::TaskTerminated)
            } else {
                match tokio::time::timeout(timeout, join_handle).await {
                    Ok(Ok(value)) => Ok(value),
                    Ok(Err(_)) => Err(GoError::TaskTerminated),
                    Err(_) => Err(GoError::Timeout),
                }
            }
        })
    }

    fn legacy_wait<T>(receiver: oneshot::Receiver<T>, timeout: Duration) -> WaitFuture<T>
    where
        T: Send + 'static,
    {
        Box::pin(async move {
            if timeout.is_zero() {
                receiver.await.map_err(|_| GoError::TaskTerminated)
            } else {
                match tokio::time::timeout(timeout, receiver).await {
                    Ok(Ok(value)) => Ok(value),
                    Ok(Err(_)) => Err(GoError::TaskTerminated),
                    Err(_) => Err(GoError::Timeout),
                }
            }
        })
    }

    pub fn spawn_direct<Task>(profile: u8, timeout: Duration, task: Task) -> GoTask<Task::Output>
    where
        Task: Future + Send + 'static,
        Task::Output: Send + 'static,
    {
        let runtime = match runtime(profile) {
            Ok(runtime) => runtime,
            Err(error) => return GoTask::failed(error),
        };
        let join_handle = runtime.spawn(task);
        let abort_handle = join_handle.abort_handle();
        GoTask::running(direct_wait(join_handle, timeout), abort_handle)
    }

    pub fn spawn_legacy<T, Build, Task>(profile: u8, timeout: Duration, build: Build) -> GoTask<T>
    where
        T: Send + 'static,
        Build: FnOnce(Sender<T>) -> Task,
        Task: Future<Output = ()> + Send + 'static,
    {
        let runtime = match runtime(profile) {
            Ok(runtime) => runtime,
            Err(error) => return GoTask::failed(error),
        };
        let (sender, receiver) = oneshot::channel();
        let join_handle = runtime.spawn(build(sender));
        let abort_handle = join_handle.abort_handle();
        drop(join_handle);
        GoTask::running(legacy_wait(receiver, timeout), abort_handle)
    }
}

/// Schedules owned async work immediately on a profile-dedicated Tokio
/// runtime.
///
/// The preferred forms return the async block's output directly:
///
/// ```
/// use std::time::Duration;
/// use tokio_go::{go, Context};
///
/// # #[tokio::main]
/// # async fn main() -> Result<(), tokio_go::GoError> {
/// let default_value = go!(async move { String::from("default") }).await?;
/// let profile_value = go!(
///     async move { String::from("profile") },
///     Context::new()
///         .profile(7)
///         .timeout(Duration::from_secs(1)),
/// )
/// .await?;
/// assert_eq!(default_value, "default");
/// assert_eq!(profile_value, "profile");
/// # Ok(())
/// # }
/// ```
///
/// Calls schedule immediately, so multiple handles can be created before they
/// are awaited sequentially or concurrently with `tokio::join!`:
///
/// ```
/// use tokio_go::go;
///
/// # #[tokio::main]
/// # async fn main() -> Result<(), tokio_go::GoError> {
/// let first = go!(async move { 1 });
/// let second = go!(async move { 2 });
/// let (first, second) = tokio::join!(first, second);
/// assert_eq!((first?, second?), (1, 2));
/// # Ok(())
/// # }
/// ```
///
/// Profile runtimes require the async block and its output to be `Send +
/// 'static`. Move owned values such as `String` or `Arc` into the block.
/// Borrowed caller locals and non-`Send` values such as `Rc` are intentionally
/// outside this API's contract; this crate does not provide scoped tasks.
///
/// A block that borrows a caller local does not meet the `'static` boundary:
///
/// ```compile_fail
/// use tokio_go::go;
///
/// let text = String::from("borrowed");
/// let task = go!(async { text.len() });
/// drop(task);
/// ```
///
/// Moving a non-`Send` value such as `Rc` also remains outside the contract:
///
/// ```compile_fail
/// use std::rc::Rc;
/// use tokio_go::go;
///
/// let value = Rc::new(1usize);
/// let task = go!(async move { *value });
/// drop(task);
/// ```
///
/// The two sender-based 0.2 forms remain source compatible. They also schedule
/// immediately and their [`GoTask`] completes when the sender sends, even if
/// the spawned task continues running afterward.
#[macro_export]
macro_rules! go {
    (|$sender:ident : Sender<$output:ty>|$task:expr) => {
        $crate::__private::spawn_legacy(
            0,
            $crate::__private::Duration::ZERO,
            |$sender: $crate::__private::Sender<$output>| $task,
        )
    };
    (|$sender:ident : Sender<$output:ty>|$task:expr,$context:expr $(,)?) => {{
        let context = $context;
        $crate::__private::spawn_legacy(
            context.profile,
            context.timeout,
            |$sender: $crate::__private::Sender<$output>| $task,
        )
    }};
    ($task:expr,$context:expr $(,)?) => {{
        let context = $context;
        $crate::__private::spawn_direct(context.profile, context.timeout, $task)
    }};
    ($task:expr) => {
        $crate::__private::spawn_direct(0, $crate::__private::Duration::ZERO, $task)
    };
}
