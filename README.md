# tokio-go

`tokio-go` schedules owned async work immediately on reusable,
profile-dedicated Tokio runtimes. Its `go!` macro returns a typed `GoTask<T>`
whose output can be awaited directly.

The crate requires Rust 1.71 or newer.

## Direct results

The preferred form returns the async block's value without a user-visible
channel or type annotation:

```rust
use tokio_go::go;

#[tokio::main]
async fn main() -> Result<(), tokio_go::GoError> {
    let input = String::from("owned");
    let value = go!(async move { format!("{input}-result") }).await?;

    assert_eq!(value, "owned-result");
    Ok(())
}
```

Calls schedule work immediately. Creating several handles starts all their
tasks before the handles are awaited, whether you later await sequentially or
use `tokio::join!`:

```rust
use tokio_go::go;

#[tokio::main]
async fn main() -> Result<(), tokio_go::GoError> {
    let first = go!(async move { 1 });
    let second = go!(async move { 2 });
    let (first, second) = tokio::join!(first, second);

    assert_eq!((first?, second?), (1, 2));
    Ok(())
}
```

## Profiles and timeouts

Use the const builder API or public fields to select one of 256 reusable
runtime profiles and configure result-waiting:

```rust
use std::time::Duration;
use tokio_go::{go, Context};

const BACKGROUND: Context = Context::new()
    .profile(7)
    .timeout(Duration::from_secs(1));

#[tokio::main]
async fn main() -> Result<(), tokio_go::GoError> {
    let value = go!(async move { 7usize }, BACKGROUND).await?;
    assert_eq!(value, 7);
    Ok(())
}
```

`Duration::ZERO` means no deadline. A positive timeout limits only how long
the `GoTask` waits for its result; it never aborts the detached work.

## Detach and cancellation

Dropping `GoTask` detaches it. `detach()` makes that intent explicit and
reports a synchronous runtime-initialization failure if the task could not be
started:

```rust
use tokio_go::go;

# fn example() -> Result<(), tokio_go::GoError> {
go!(async move {
    // owned background work
})
.detach()?;
# Ok(())
# }
```

`abort(&self)` is the only handle-driven cancellation operation. If abort
prevents the expected result, awaiting the handle returns
`GoError::TaskTerminated`. Task panic and other runtime termination can produce
the same typed error without an explicit abort.

## Ownership boundary

Profile runtimes require task futures and outputs to be `Send + 'static`.
Move owned values such as `String`, custom structs, and `Arc` into the async
block. Borrowed caller locals and non-`Send` values such as `Rc` are not
supported; `tokio-go` does not promise unsafe or scoped borrowed tasks.

## Legacy sender forms

Both 0.2 sender forms remain source compatible and macro-hygienic:

```rust
use tokio_go::{go, Context};

#[tokio::main]
async fn main() -> Result<(), tokio_go::GoError> {
    let default = go!(|sender: Sender<i32>| async move {
        let _ = sender.send(1);
    })
    .await?;
    let profiled = go!(
        |sender: Sender<i32>| async move {
            let _ = sender.send(2);
        },
        Context::new().profile(2),
    )
    .await?;

    assert_eq!((default, profiled), (1, 2));
    Ok(())
}
```

Legacy work now schedules immediately, and its `GoTask` returns when the
sender sends even if the spawned task continues afterward.

## Errors

Every form returns `Result<T, GoError>` when awaited:

- `GoError::Timeout` means the positive result-waiting deadline elapsed.
- `GoError::RuntimeInitialization { .. }` means Tokio could not build the
  selected profile runtime.
- `GoError::TaskTerminated` means the task did not produce its expected result,
  including abort, panic, or a legacy sender being dropped.

## Migrating from 0.1.4 to 0.3.0

Version 0.2.0 was an unpublished development version and was never released to
crates.io. Public users therefore upgrade directly from 0.1.4 to 0.3.0.

Version 0.3.0 adds the preferred direct-result syntax, immediate scheduling,
`GoTask<T>`, explicit `detach()`/`abort()`, and const `Context` builders. The
two sender-based forms remain available, but their scheduling is deliberately
eager: task work begins when `go!` is evaluated instead of when its returned
future is first polled.

Sender-based `.await` call sites remain available, but 0.1.4 string errors are
replaced by `GoError`. Code that intentionally ignores a returned task should
now call `.detach()?` or explicitly `drop(task)` to satisfy `GoTask`'s
`#[must_use]` contract.

During development, the unpublished 0.2.0 version replaced 0.1 string errors
with `GoError` and made the runtime registry private. Those changes are part of
the public 0.1.4-to-0.3.0 migration.

## License

Licensed under either the MIT License or the Apache License, Version 2.0, at
your option.
