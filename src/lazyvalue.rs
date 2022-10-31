use crate::{box_future_factory, BoxedFutureFactory, DataState, Message, Promise};
use std::fmt::Debug;
use std::future::Future;
use tokio::sync::mpsc::{channel, Receiver, Sender};

/// # A single lazy-async updated value
/// Create one with the [`LazyValuePromise::new`] method and supply an updater.
/// It's Updated only on first try to poll it making it scale nicely on more complex UIs.
/// While still updating, the value can be read out already, this can make sense for iterative algorithms where an intermediate result can be used as a preview.
/// Examples:
/// ```rust, no_run
/// use std::time::Duration;
/// use tokio::sync::mpsc::Sender;
/// use lazy_async_promise::{DataState, Message, Promise};
/// use lazy_async_promise::LazyValuePromise;
/// use lazy_async_promise::unpack_result;
/// // updater-future:
/// let updater = |tx: Sender<Message<i32>>| async move {
///   tx.send(Message::NewData(1337)).await.unwrap();
///   // how to handle results and propagate the error to the future? Use `unpack_result!`:
///   let string = unpack_result!(std::fs::read_to_string("whatever.txt"), tx);
///   tokio::time::sleep(Duration::from_millis(100)).await;
///   tx.send(Message::StateChange(DataState::UpToDate)).await.unwrap();
/// };
/// // direct usage:
/// let promise = LazyValuePromise::new(updater, 10);
///
/// fn main_loop(lazy_promise: &mut  LazyValuePromise<i32>) {
///   loop {
///     match lazy_promise.poll_state() {
///       DataState::Error(er)  => { println!("Error {} occurred! Retrying!", er); std::thread::sleep(Duration::from_millis(500)); lazy_promise.update(); }
///       DataState::UpToDate   => { println!("Value up2date: {}", lazy_promise.value().unwrap()); }
///                           _ => { println!("Still updating... might be in strange state! (current state: {:?}", lazy_promise.value()); }  
///     }
///   }
/// }
/// ```
///
///

pub struct LazyValuePromise<T: Debug> {
    cache: Option<T>,
    updater: BoxedFutureFactory<T>,
    state: DataState,
    rx: Receiver<Message<T>>,
    tx: Sender<Message<T>>,
}
impl<T: Debug> LazyValuePromise<T> {
    /// Creates a new LazyValuePromise given an Updater and a tokio buffer size
    pub fn new<
        U: Fn(Sender<Message<T>>) -> Fut + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    >(
        future_factory: U,
        buffer_size: usize,
    ) -> Self {
        let (tx, rx) = channel::<Message<T>>(buffer_size);

        Self {
            cache: None,
            state: DataState::Uninitialized,
            rx,
            tx,
            updater: box_future_factory(future_factory),
        }
    }

    /// get current value, may be incomplete depending on status
    pub fn value(&self) -> Option<&T> {
        self.cache.as_ref()
    }

    #[cfg(test)]
    pub fn is_uninitialized(&self) -> bool {
        self.state == DataState::Uninitialized
    }
}

impl<T: Debug> Promise for LazyValuePromise<T> {
    fn poll_state(&mut self) -> &DataState {
        if self.state == DataState::Uninitialized {
            self.update();
        }

        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Message::NewData(data) => {
                    self.cache = Some(data);
                }
                Message::StateChange(new_state) => {
                    self.state = new_state;
                }
            }
        }

        &self.state
    }

    fn update(&mut self) {
        if matches!(self.state, DataState::Updating(_)) {
            return;
        }
        self.cache = None;

        self.state = DataState::Updating(0.0.into());
        let future = (self.updater)(self.tx.clone());
        tokio::spawn(future);
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::unpack_result;
    use std::time::Duration;
    use tokio::runtime::{Builder, Runtime};
    use tokio::sync::mpsc::Sender;

    #[test]
    fn basic_usage_cycle() {
        let string_maker = |tx: Sender<Message<String>>| async move {
            for i in 0..2 {
                tx.send(Message::NewData(i.to_string())).await.unwrap();
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            tx.send(Message::StateChange(DataState::UpToDate))
                .await
                .unwrap();
        };

        Builder::new_multi_thread()
            .worker_threads(2)
            .enable_time()
            .max_blocking_threads(2)
            .build()
            .unwrap()
            .block_on(async {
                let mut delayed_value = LazyValuePromise::new(string_maker, 6);
                //be lazy:
                assert!(delayed_value.is_uninitialized());
                //start sets updating
                assert_eq!(*delayed_value.poll_state(), DataState::Updating(0.0.into()));
                assert!(delayed_value.value().is_none());
                //after wait, value is there
                tokio::time::sleep(Duration::from_millis(150)).await;
                assert_eq!(*delayed_value.poll_state(), DataState::UpToDate);
                assert_eq!(delayed_value.value().unwrap(), "1");
                //update resets
                delayed_value.update();
                assert_eq!(*delayed_value.poll_state(), DataState::Updating(0.0.into()));
                assert!(delayed_value.value().is_none());
                //after wait, value is there again and identical
                tokio::time::sleep(Duration::from_millis(150)).await;
                assert_eq!(*delayed_value.poll_state(), DataState::UpToDate);
                assert_eq!(delayed_value.value().unwrap(), "1");
            });
    }

    #[test]
    fn error_propagation() {
        let error_maker = |tx: Sender<Message<String>>| async move {
            let _ = unpack_result!(std::fs::read_to_string("FILE_NOT_EXISTING"), tx);
            tx.send(Message::StateChange(DataState::UpToDate))
                .await
                .unwrap();
        };

        Runtime::new().unwrap().block_on(async {
            let mut delayed_vec = LazyValuePromise::new(error_maker, 1);
            assert_eq!(*delayed_vec.poll_state(), DataState::Updating(0.0.into()));
            assert!(delayed_vec.value().is_none());
            tokio::time::sleep(Duration::from_millis(150)).await;
            assert!(matches!(*delayed_vec.poll_state(), DataState::Error(_)));
            assert!(delayed_vec.value().is_none());
        });
    }
}
