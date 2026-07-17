use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio_go::{go, Context, GoError, GoTask};

const BUILT_CONTEXT: Context = Context::new().profile(17).timeout(Duration::ZERO);

#[derive(Debug, Eq, PartialEq)]
struct OwnedValue {
    label: String,
    count: usize,
}

struct DropSignal(Option<tokio::sync::oneshot::Sender<()>>);

impl Drop for DropSignal {
    fn drop(&mut self) {
        if let Some(sender) = self.0.take() {
            let _ = sender.send(());
        }
    }
}

fn assert_go_task_future<T, Task>(_: &Task)
where
    Task: Future<Output = Result<T, GoError>>,
{
}

#[tokio::test]
async fn direct_form_infers_owned_results_and_captures() {
    let number: i32 = go!(async move { 42 }).await.expect("number task failed");
    assert_eq!(number, 42);

    let text = String::from("owned");
    let returned: String = go!(async move { format!("{text}-value") })
        .await
        .expect("String task failed");
    assert_eq!(returned, "owned-value");

    let value = OwnedValue {
        label: String::from("custom"),
        count: 3,
    };
    let returned: OwnedValue = go!(async move { value })
        .await
        .expect("custom value task failed");
    assert_eq!(
        returned,
        OwnedValue {
            label: String::from("custom"),
            count: 3,
        }
    );

    let shared = Arc::new(AtomicUsize::new(0));
    let task_shared = Arc::clone(&shared);
    let returned: Arc<AtomicUsize> = go!(async move {
        task_shared.fetch_add(1, Ordering::SeqCst);
        task_shared
    })
    .await
    .expect("Arc task failed");
    assert_eq!(returned.load(Ordering::SeqCst), 1);
    assert_eq!(shared.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn context_builders_are_const_and_public_fields_remain_supported() {
    assert_eq!(BUILT_CONTEXT.profile, 17);
    assert_eq!(BUILT_CONTEXT.timeout, Duration::ZERO);
    assert_eq!(Context::default(), Context::new());

    let built = go!(async move { "built" }, BUILT_CONTEXT)
        .await
        .expect("builder context task failed");
    assert_eq!(built, "built");

    let fields = go!(
        async move { "fields" },
        Context {
            profile: 18,
            timeout: Duration::ZERO,
        }
    )
    .await
    .expect("public-field context task failed");
    assert_eq!(fields, "fields");
}

#[tokio::test]
async fn direct_task_starts_before_handle_is_polled() {
    let (started_sender, started_receiver) = tokio::sync::oneshot::channel();
    let (release_sender, release_receiver) = tokio::sync::oneshot::channel();

    let task = go!(async move {
        let _ = started_sender.send(());
        let _ = release_receiver.await;
        7usize
    });

    started_receiver
        .await
        .expect("task must start before its GoTask is polled");
    release_sender
        .send(())
        .expect("task should still hold the release receiver");
    assert_eq!(task.await, Ok(7));
}

#[tokio::test]
async fn dropping_handle_detaches_running_work() {
    let (started_sender, started_receiver) = tokio::sync::oneshot::channel();
    let (release_sender, release_receiver) = tokio::sync::oneshot::channel();
    let (done_sender, done_receiver) = tokio::sync::oneshot::channel();

    let task = go!(async move {
        let _ = started_sender.send(());
        let _ = release_receiver.await;
        let _ = done_sender.send(());
    });
    started_receiver.await.expect("task did not start");

    drop(task);
    release_sender
        .send(())
        .expect("dropped handle must not drop task work");
    done_receiver
        .await
        .expect("task must continue after its handle is dropped");
}

#[tokio::test]
async fn explicit_detach_leaves_work_running() {
    let (started_sender, started_receiver) = tokio::sync::oneshot::channel();
    let (release_sender, release_receiver) = tokio::sync::oneshot::channel();
    let (done_sender, done_receiver) = tokio::sync::oneshot::channel();

    let task = go!(async move {
        let _ = started_sender.send(());
        let _ = release_receiver.await;
        let _ = done_sender.send(());
    });
    task.detach().expect("runtime initialized successfully");

    started_receiver
        .await
        .expect("detached task must start without handle polling");
    release_sender
        .send(())
        .expect("detached task must retain its release receiver");
    done_receiver
        .await
        .expect("detached task must run to completion");
}

#[tokio::test(start_paused = true)]
async fn timeout_detaches_work_after_the_exact_deadline() {
    let (started_sender, started_receiver) = tokio::sync::oneshot::channel();
    let (release_sender, release_receiver) = tokio::sync::oneshot::channel();
    let (done_sender, done_receiver) = tokio::sync::oneshot::channel();

    let task = go!(
        async move {
            let _ = started_sender.send(());
            let _ = release_receiver.await;
            let _ = done_sender.send(());
            9usize
        },
        Context::new().profile(19).timeout(Duration::from_secs(1)),
    );
    started_receiver
        .await
        .expect("timed task must start before handle polling");
    tokio::pin!(task);

    tokio::select! {
        result = &mut task => panic!("timeout completed immediately: {result:?}"),
        _ = tokio::task::yield_now() => {}
    }
    tokio::time::advance(Duration::from_millis(999)).await;
    tokio::select! {
        result = &mut task => panic!("timeout completed before its deadline: {result:?}"),
        _ = tokio::task::yield_now() => {}
    }
    tokio::time::advance(Duration::from_millis(1)).await;
    assert_eq!(task.await, Err(GoError::Timeout));

    release_sender
        .send(())
        .expect("timed-out task must remain attached to its work");
    done_receiver
        .await
        .expect("timed-out task must continue after waiting stops");
}

#[tokio::test]
async fn abort_cancels_work_and_returns_task_terminated() {
    let (started_sender, started_receiver) = tokio::sync::oneshot::channel();
    let (dropped_sender, dropped_receiver) = tokio::sync::oneshot::channel();
    let completed = Arc::new(AtomicBool::new(false));
    let task_completed = Arc::clone(&completed);

    let task = go!(async move {
        let _drop_signal = DropSignal(Some(dropped_sender));
        let _ = started_sender.send(());
        std::future::pending::<()>().await;
        task_completed.store(true, Ordering::SeqCst);
        1usize
    });
    started_receiver.await.expect("task did not start");

    task.abort();
    assert_eq!(task.await, Err(GoError::TaskTerminated));
    dropped_receiver
        .await
        .expect("abort must drop the task future");
    assert!(!completed.load(Ordering::SeqCst));
}

#[tokio::test]
async fn direct_task_panic_returns_task_terminated() {
    let task = go!(async move {
        if std::hint::black_box(true) {
            panic!("intentional direct task panic");
        }
        1usize
    });

    assert_eq!(task.await, Err(GoError::TaskTerminated));
}

#[tokio::test]
async fn direct_context_expression_is_evaluated_once() {
    let evaluations = Arc::new(AtomicUsize::new(0));
    let context_evaluations = Arc::clone(&evaluations);

    let task = go!(async move { 21usize }, {
        context_evaluations.fetch_add(1, Ordering::SeqCst);
        Context::new().profile(21)
    });

    assert_eq!(evaluations.load(Ordering::SeqCst), 1);
    assert_eq!(task.await, Ok(21));
    assert_eq!(evaluations.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn legacy_default_starts_before_handle_is_polled() {
    let (started_sender, started_receiver) = tokio::sync::oneshot::channel();
    let (release_sender, release_receiver) = tokio::sync::oneshot::channel();

    let task = go!(|sender: Sender<i32>| async move {
        let _ = started_sender.send(());
        let _ = release_receiver.await;
        let _ = sender.send(22);
    });

    started_receiver
        .await
        .expect("legacy task must start before GoTask polling");
    release_sender
        .send(())
        .expect("legacy task should retain the release receiver");
    assert_eq!(task.await, Ok(22));
}

#[tokio::test(start_paused = true)]
async fn legacy_handle_returns_on_send_before_task_tail_finishes() {
    let (release_sender, release_receiver) = tokio::sync::oneshot::channel();
    let (tail_done_sender, tail_done_receiver) = tokio::sync::oneshot::channel();
    let (sent_sender, sent_receiver) = tokio::sync::oneshot::channel();

    let task = go!(|sender: Sender<i32>| async move {
        let _ = sender.send(23);
        let _ = sent_sender.send(());
        let _ = release_receiver.await;
        let _ = tail_done_sender.send(());
    });

    sent_receiver
        .await
        .expect("legacy task did not send before blocking its tail");
    let value = tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .expect("legacy handle waited for blocked task tail")
        .expect("legacy sender failed");
    assert_eq!(value, 23);

    release_sender
        .send(())
        .expect("legacy tail should remain alive after send");
    tail_done_receiver
        .await
        .expect("legacy task tail should finish after release");
}

#[tokio::test]
async fn legacy_context_form_is_compatible_and_evaluated_once() {
    let evaluations = Arc::new(AtomicUsize::new(0));
    let context_evaluations = Arc::clone(&evaluations);

    let task = go!(
        |sender: Sender<String>| async move {
            let _ = sender.send(String::from("legacy-context"));
        },
        {
            context_evaluations.fetch_add(1, Ordering::SeqCst);
            Context {
                profile: 24,
                timeout: Duration::ZERO,
            }
        }
    );

    assert_eq!(evaluations.load(Ordering::SeqCst), 1);
    assert_eq!(task.await, Ok(String::from("legacy-context")));
    assert_eq!(evaluations.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn legacy_sender_drop_returns_task_terminated() {
    let task = go!(|sender: Sender<()>| async move {
        drop(sender);
    });

    assert_eq!(task.await, Err(GoError::TaskTerminated));
}

#[tokio::test]
async fn same_profile_supports_concurrent_initialization_and_reuse() {
    let mut tasks = Vec::new();

    for value in 0..32usize {
        tasks.push(go!(async move { value }, Context::new().profile(42)));
    }

    for (expected, task) in tasks.into_iter().enumerate() {
        assert_eq!(task.await, Ok(expected));
    }

    let repeated = go!(async move { "reused" }, Context::new().profile(42));
    assert_eq!(repeated.await, Ok("reused"));
}

#[tokio::test]
async fn different_profiles_run_independently() {
    let profile_43 = go!(async move { 43u8 }, Context::new().profile(43));
    let profile_44 = go!(async move { 44u8 }, Context::new().profile(44));

    assert_eq!(profile_43.await, Ok(43));
    assert_eq!(profile_44.await, Ok(44));
}

#[tokio::test]
async fn go_task_implements_the_documented_future_contract() {
    let task: GoTask<usize> = go!(async move { 25usize });
    assert_go_task_future::<usize, _>(&task);
    assert_eq!(task.await, Ok(25));
}

#[test]
fn typed_errors_are_matchable_and_have_stable_display() {
    assert!(matches!(GoError::Timeout, GoError::Timeout));
    assert_eq!(GoError::Timeout.to_string(), "the go task timed out");
    assert_eq!(
        GoError::TaskTerminated.to_string(),
        "the go task terminated without sending a result"
    );

    let initialization = GoError::RuntimeInitialization {
        profile: 3,
        message: String::from("builder failed"),
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
