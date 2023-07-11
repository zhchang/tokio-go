mod prelude {
    pub use std::sync::{Arc, RwLock};
    pub use tokio::runtime::Runtime;
    pub use tokio::sync::oneshot::{channel, Sender};
    pub use tokio::time::{sleep, timeout, Duration};
}
use prelude::*;

#[macro_use]
extern crate lazy_static;

const RUNTIME_INIT: Option<Runtime> = None;

lazy_static! {
    static ref RUNTIMES: Arc<RwLock<[Option<Runtime>; 255]>> =
        Arc::new(RwLock::new([RUNTIME_INIT; 255]));
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
            match timeout($c.timeout, receiver).await {
                Err(_) => Err("timeout"),
                Ok(v) => Ok(v.unwrap()),
            }
        }
    };
}

#[cfg(test)]
mod tests {
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
                if let Err(_) = sender.send("shit".to_string()) {
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
        assert_eq!(r2, "shit");
        let r3 = go!(|sender: Sender<()>| async move {
            println!("Thread id: {:?}", thread::current().id());
            if let Err(_) = sender.send(()) {
                println!("the receiver dropped");
            }
        })
        .await
        .unwrap();
        assert_eq!(r3, ());
    }
}
