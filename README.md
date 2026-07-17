# tokio-go

`tokio-go` provides a compact `go!` macro for running an async closure on a
reusable, profile-dedicated Tokio runtime. The caller awaits a value sent over
a Tokio oneshot channel.

The crate requires Rust 1.71 or newer.

## Quick start

The default form uses profile `0` and waits without a timeout. Macro internals
are hygienic, so `Sender` in the call form does not require the prelude or a
Tokio import:

```rust
use tokio_go::go;

#[tokio::main]
async fn main() -> Result<(), tokio_go::GoError> {
    let value = go!(|sender: Sender<i32>| async move {
        let _ = sender.send(42);
    })
    .await?;

    assert_eq!(value, 42);
    Ok(())
}
```

Use `Context` to select one of the 256 runtime profiles and configure how long
to wait for a result:

```rust
use std::time::Duration;
use tokio_go::{go, Context, GoError};

#[tokio::main]
async fn main() {
    let result = go!(
        |sender: Sender<()>| async move {
            std::future::pending::<()>().await;
            let _ = sender.send(());
        },
        Context {
            profile: 7,
            timeout: Duration::from_millis(10),
        }
    )
    .await;

    assert_eq!(result, Err(GoError::Timeout));
}
```

Each profile runtime is initialized at most once and reused. A
`Duration::ZERO` timeout means no deadline. A positive timeout limits only how
long the caller waits: the spawned task is detached and continues running
after `GoError::Timeout`.

## Errors

`go!` returns `Result<T, GoError>`:

- `GoError::Timeout` means the configured positive deadline elapsed.
- `GoError::RuntimeInitialization { .. }` means Tokio could not build the
  selected profile runtime.
- `GoError::TaskTerminated` means the sender was dropped or the spawned task
  otherwise ended before sending a value.

## Migrating from 0.1 to 0.2

Version 0.2.0 intentionally changes the error type from string literals to
`GoError`. Code that compares `Err("timeout")` or `Err("unknown error")`
should match the corresponding enum variant instead. The `go!` invocation
forms, `Context.profile`, profile-dedicated runtime behavior, and the prelude's
common imports remain available.

The runtime registry is now private. Code that accessed `prelude::RUNTIMES` or
called `prelude::init_runtime` must stop doing so; runtimes are initialized
automatically by `go!`.

## License

Licensed under either the MIT License or the Apache License, Version 2.0, at
your option.
