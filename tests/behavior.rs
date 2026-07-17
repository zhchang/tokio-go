use std::time::Duration;

use tokio_go::{go, Context, GoError};

#[tokio::test]
async fn default_profile_returns_value() {
    let result = go!(|sender: Sender<i32>| async move {
        let _ = sender.send(42);
    })
    .await;

    assert_eq!(result, Ok(42));
}

#[tokio::test]
async fn nonzero_profile_returns_value() {
    let result = go!(
        |sender: Sender<&'static str>| async move {
            let _ = sender.send("profile-7");
        },
        Context {
            profile: 7,
            timeout: Duration::from_secs(1),
        }
    )
    .await;

    assert_eq!(result, Ok("profile-7"));
}

#[tokio::test]
async fn zero_duration_waits_without_a_deadline() {
    let result = go!(
        |sender: Sender<i32>| async move {
            tokio::task::yield_now().await;
            let _ = sender.send(7);
        },
        Context {
            profile: 8,
            timeout: Duration::ZERO,
        }
    )
    .await;

    assert_eq!(result, Ok(7));
}

#[tokio::test(start_paused = true)]
async fn positive_timeout_waits_until_the_deadline() {
    let result = go!(
        |sender: Sender<()>| async move {
            std::future::pending::<()>().await;
            let _ = sender.send(());
        },
        Context {
            profile: 9,
            timeout: Duration::from_secs(1),
        }
    );
    tokio::pin!(result);

    tokio::select! {
        result = &mut result => panic!("timeout completed immediately: {result:?}"),
        _ = tokio::task::yield_now() => {}
    }

    tokio::time::advance(Duration::from_millis(999)).await;
    tokio::select! {
        result = &mut result => panic!("timeout completed before its deadline: {result:?}"),
        _ = tokio::task::yield_now() => {}
    }

    tokio::time::advance(Duration::from_millis(1)).await;
    assert_eq!(result.await, Err(GoError::Timeout));
}

#[tokio::test(start_paused = true)]
async fn timed_out_task_continues_running() {
    let (continue_sender, continue_receiver) = tokio::sync::oneshot::channel();
    let (done_sender, done_receiver) = tokio::sync::oneshot::channel();

    let result = go!(
        |sender: Sender<()>| async move {
            let _ = continue_receiver.await;
            let _ = done_sender.send(());
            let _ = sender.send(());
        },
        Context {
            profile: 10,
            timeout: Duration::from_secs(1),
        }
    );
    tokio::pin!(result);

    tokio::select! {
        result = &mut result => panic!("timeout completed immediately: {result:?}"),
        _ = tokio::task::yield_now() => {}
    }
    tokio::time::advance(Duration::from_secs(1)).await;
    assert_eq!(result.await, Err(GoError::Timeout));

    continue_sender
        .send(())
        .expect("detached task should still hold the control receiver");
    done_receiver
        .await
        .expect("detached task should continue after the caller times out");
}

#[tokio::test]
async fn dropped_sender_returns_task_terminated() {
    let result = go!(|sender: Sender<()>| async move {
        drop(sender);
    })
    .await;

    assert_eq!(result, Err(GoError::TaskTerminated));
}

#[tokio::test]
async fn panicked_task_returns_task_terminated() {
    let result = go!(|_sender: Sender<()>| async move {
        panic!("intentional task panic");
    })
    .await;

    assert_eq!(result, Err(GoError::TaskTerminated));
}

#[tokio::test]
async fn same_profile_supports_concurrent_initialization_and_reuse() {
    let mut tasks = Vec::new();

    for value in 0..32 {
        tasks.push(tokio::spawn(async move {
            go!(
                |sender: Sender<usize>| async move {
                    let _ = sender.send(value);
                },
                Context {
                    profile: 42,
                    timeout: Duration::ZERO,
                }
            )
            .await
        }));
    }

    for (expected, task) in tasks.into_iter().enumerate() {
        assert_eq!(
            task.await.expect("caller task should not panic"),
            Ok(expected)
        );
    }

    let repeated = go!(
        |sender: Sender<&'static str>| async move {
            let _ = sender.send("reused");
        },
        Context {
            profile: 42,
            timeout: Duration::ZERO,
        }
    )
    .await;
    assert_eq!(repeated, Ok("reused"));
}

#[tokio::test]
async fn different_profiles_run_independently() {
    let profile_11 = tokio::spawn(async {
        go!(
            |sender: Sender<u8>| async move {
                let _ = sender.send(11);
            },
            Context {
                profile: 11,
                timeout: Duration::ZERO,
            }
        )
        .await
    });
    let profile_12 = tokio::spawn(async {
        go!(
            |sender: Sender<u8>| async move {
                let _ = sender.send(12);
            },
            Context {
                profile: 12,
                timeout: Duration::ZERO,
            }
        )
        .await
    });

    assert_eq!(
        profile_11.await.expect("profile 11 caller panicked"),
        Ok(11)
    );
    assert_eq!(
        profile_12.await.expect("profile 12 caller panicked"),
        Ok(12)
    );
}

#[test]
fn typed_errors_are_matchable_and_documented_by_display() {
    assert!(matches!(GoError::Timeout, GoError::Timeout));
    assert_eq!(GoError::Timeout.to_string(), "the go task timed out");
    assert_eq!(
        GoError::TaskTerminated.to_string(),
        "the go task terminated without sending a result"
    );

    let initialization = GoError::RuntimeInitialization {
        profile: 3,
        message: "builder failed".to_owned(),
    };
    assert!(matches!(
        initialization,
        GoError::RuntimeInitialization { profile: 3, .. }
    ));
    assert_eq!(
        initialization.to_string(),
        "failed to initialize runtime for profile 3: builder failed"
    );
}
