pub mod prelude {
    pub use std::sync::{Arc, RwLock};
    pub use tokio::runtime::Runtime;
    pub use tokio::sync::oneshot::{channel, Sender};
    pub use tokio::time::{sleep, timeout, Duration};
    const RUNTIME_INIT: Option<Runtime> = None;
    lazy_static! {
        pub static ref RUNTIMES: Arc<RwLock<[Option<Runtime>; 256]>> =
            Arc::new(RwLock::new([RUNTIME_INIT; 256]));
    }
    #[derive(Debug)]
    pub struct Context {
        pub profile: u8,
        pub timeout: Duration,
    }

    pub fn init_runtime(profile: u8) {
        {
            let r = RUNTIMES.read().unwrap();
            if r[profile as usize].is_some() {
                return;
            }
        }
        let mut w = RUNTIMES.write().unwrap();
        if w[profile as usize].is_none() {
            w[profile as usize] = Some(Runtime::new().unwrap());
        }
    }
}
/// support running a async closure in default or specified tokio runtime
/// # Example:
/// using default runtime, without timeout
/// ```
/// use tokio_go::prelude::*;
/// use tokio_go::go;
/// #[tokio::main]
/// async fn main(){
/// let r = go!(|tx: Sender<i32>|async move{
///   tx.send(1);
/// }).await;
/// println!("{}",r.unwrap()); //1
/// }
/// ```
///
/// using specified runtime (identified by context.profile), with timeout Duration
///
/// ```
/// use tokio_go::prelude::*;
/// use tokio_go::go;
/// #[tokio::main]
/// async fn main(){
/// let r = go!(|tx: Sender<String>|async move{
///     sleep(Duration::from_secs(2));
///     tx.send("whocares".to_string());
/// },
/// Context{
///     profile: 1,
///     timeout: Duration::from_secs(1),
/// }
/// ).await;
/// println!("{:?}",r.is_ok());//false
/// }
/// ```

#[macro_export]
macro_rules! go {
    (|$x:ident : Sender<$t:ty>|$y:expr) => {
        async {
            let (sender, receiver) = channel::<$t>();
            init_runtime(0);
            let rts = RUNTIMES.read().unwrap();
            let runtime = rts[0].as_ref().unwrap();
            runtime.spawn((|$x: Sender<$t>| $y)(sender));
            match receiver.await {
                Ok(v) => Ok(v),
                Err(_) => Err("unknown error"),
            }
        }
    };
    (|$x:ident : Sender<$t:ty>|$y:expr,$c:expr) => {
        async {
            let (sender, receiver) = channel::<$t>();
            init_runtime($c.profile);
            let rts = RUNTIMES.read().unwrap();
            let runtime = rts[$c.profile as usize].as_ref().unwrap();
            runtime.spawn((|$x: Sender<$t>| $y)(sender));
            match $c.timeout {
                Duration::ZERO => match receiver.await {
                    Ok(v) => Ok(v),
                    Err(_) => Err("unknown error"),
                },
                _ => match timeout($c.timeout, receiver).await {
                    Err(_) => Err("timeout"),
                    Ok(v) => Ok(v.unwrap()),
                },
            }
        }
    };
}

#[macro_use]
extern crate lazy_static;

#[cfg(test)]
mod tests {
    use super::prelude::*;
    use super::*;
    use std::thread;

    #[tokio::test]
    async fn it_works() {
        let r1 = go!(|sender: Sender<i32>| async move {
            println!("Thread id: {:?}", thread::current().id());
            if let Err(_) = sender.send(2) {
                println!("the receiver dropped");
            }
        })
        .await
        .unwrap();
        assert_eq!(r1, 2);
        let r2 = go!(
            |sender: Sender<String>| async move {
                println!("Thread id: {:?}", thread::current().id());
                if let Err(_) = sender.send("whocares".to_string()) {
                    println!("the receiver dropped");
                }
            },
            Context {
                profile: 1,
                timeout: Duration::from_secs(1)
            }
        )
        .await
        .unwrap();
        assert_eq!(r2, "whocares");
        let r3 = go!(
            |sender: Sender<String>| async move {
                println!("Thread id: {:?}", thread::current().id());
                if let Err(_) = sender.send("whocares".to_string()) {
                    println!("the receiver dropped");
                }
            },
            Context {
                profile: 1,
                timeout: Duration::ZERO
            }
        )
        .await
        .unwrap();
        assert_eq!(r3, "whocares");
        let r4 = go!(|sender: Sender<()>| async move {
            println!("Thread id: {:?}", thread::current().id());
            if let Err(_) = sender.send(()) {
                println!("the receiver dropped");
            }
        })
        .await
        .unwrap();
        assert_eq!(r4, ());
    }
}
