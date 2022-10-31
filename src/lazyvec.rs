use crate::{box_future_factory, BoxedFutureFactory, DataState, Message, Promise};
use std::fmt::Debug;
use std::future::Future;
use tokio::sync::mpsc::{channel, Receiver, Sender};

/// # A lazy, async and partially readable vector promise
/// This promise is the right one for async acquiring of lists which should be partially readable on each frame.
/// Imagine slowly streaming data and wanting to read them out as far as they are available each frame.
/// Examples:
/// ```rust, no_run
/// use std::time::Duration;
/// use tokio::sync::mpsc::Sender;
/// use lazy_async_promise::{DataState, Message, Progress, Promise, set_progress};
/// use lazy_async_promise::LazyVecPromise;
/// use lazy_async_promise::unpack_result;
/// // updater-future:
/// let updater = |tx: Sender<Message<i32>>| async move {
///   const ITEM_COUNT: i32 = 100;
///   for i in 0..ITEM_COUNT {
///     tx.send(Message::NewData(i)).await.unwrap();
///     set_progress!(Progress::from_fraction(i, ITEM_COUNT), tx);
///     // how to handle results and propagate the error to the future? Use `unpack_result!`:
///     let string = unpack_result!(std::fs::read_to_string("whatever.txt"), tx);
///     if i > 100 {
///       tx.send(Message::StateChange(DataState::Error("loop overflow".to_owned()))).await.unwrap();
///     }
///    tokio::time::sleep(Duration::from_millis(100)).await;
///   }
///   tx.send(Message::StateChange(DataState::UpToDate)).await.unwrap();
/// };
/// // direct usage:
/// let promise = LazyVecPromise::new(updater, 200);
///
/// fn main_loop(lazy_promise: &mut LazyVecPromise<i32>) {
///   loop {
///     match lazy_promise.poll_state() {
///       DataState::Error(er)  => { println!("Error {} occurred! Retrying!", er); std::thread::sleep(Duration::from_millis(50)); lazy_promise.update(); }
///       DataState::UpToDate   => { println!("Data complete: {:?}", lazy_promise.as_slice()); }
///                           _ => { println!("Getting data, might be partially ready! (part: {:?}", lazy_promise.as_slice()); }  
///     }
///   }
/// }
/// ```
///
///

pub struct LazyVecPromise<T: Debug> {
    data: Vec<T>,
    state: DataState,
    rx: Receiver<Message<T>>,
    tx: Sender<Message<T>>,
    updater: BoxedFutureFactory<T>,
}

impl<T: Debug> LazyVecPromise<T> {
    /// creates a new LazyVecPromise given an updater functor and a tokio buffer size
    pub fn new<
        U: Fn(Sender<Message<T>>) -> Fut + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    >(
        future_factory: U,
        buffer_size: usize,
    ) -> Self {
        let (tx, rx) = channel::<Message<T>>(buffer_size);

        Self {
            data: vec![],
            state: DataState::Uninitialized,
            rx,
            tx,
            updater: box_future_factory(future_factory),
        }
    }

    /// get current data as slice, may be incomplete depending on status
    pub fn as_slice(&self) -> &[T] {
        self.data.as_slice()
    }

    #[cfg(test)]
    pub fn is_uninitialized(&self) -> bool {
        self.state == DataState::Uninitialized
    }
}

impl<T: Debug> Promise for LazyVecPromise<T> {
    fn poll_state(&mut self) -> &DataState {
        if self.state == DataState::Uninitialized {
            self.update();
        }

        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Message::NewData(data) => {
                    self.data.push(data);
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

        self.state = DataState::Updating(0.0.into());
        self.data.clear();
        let future = (self.updater)(self.tx.clone());
        tokio::spawn(future);
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::{set_progress, unpack_result, Progress};
    use std::time::Duration;
    use tokio::runtime::{Builder, Runtime};
    use tokio::sync::mpsc::Sender;

    #[test]
    fn basic_usage_cycle() {
        let string_maker = |tx: Sender<Message<String>>| async move {
            const COUNT: i32 = 5;
            for i in 0..COUNT {
                tx.send(Message::NewData(i.to_string())).await.unwrap();
                set_progress!(Progress::from_fraction(i, COUNT), tx);
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
                let mut delayed_vec = LazyVecPromise::new(string_maker, 12);
                // start empty, polling triggers update
                assert!(delayed_vec.is_uninitialized());
                assert_eq!(*delayed_vec.poll_state(), DataState::Updating(0.0.into()));
                assert!(delayed_vec.as_slice().is_empty());
                // We have some numbers ready in between
                tokio::time::sleep(Duration::from_millis(50)).await;
                if let DataState::Updating(progress) = delayed_vec.poll_state() {
                    assert!(progress.as_f32() > 0.0);
                    assert!(progress.as_f32() < 1.0);
                    assert!(!delayed_vec.as_slice().is_empty());
                } else {
                    panic!("Was not in updating state after waiting time");
                }

                // after wait we have a result
                tokio::time::sleep(Duration::from_millis(200)).await;
                assert_eq!(*delayed_vec.poll_state(), DataState::UpToDate);
                assert_eq!(delayed_vec.as_slice().len(), 5);
                // after update it's empty again
                delayed_vec.update();
                assert_eq!(*delayed_vec.poll_state(), DataState::Updating(0.0.into()));
                assert!(delayed_vec.as_slice().is_empty());
                // finally after waiting it's full again
                tokio::time::sleep(Duration::from_millis(200)).await;
                assert_eq!(*delayed_vec.poll_state(), DataState::UpToDate);
                assert_eq!(delayed_vec.as_slice().len(), 5);
            });
    }

    #[test]
    fn error_propagation() {
        let error_maker = |tx: Sender<Message<String>>| async move {
            let _ = unpack_result!(std::fs::read_to_string("NOT_EXISTING"), tx);
            tx.send(Message::StateChange(DataState::UpToDate))
                .await
                .unwrap();
        };

        Runtime::new().unwrap().block_on(async {
            let mut delayed_vec = LazyVecPromise::new(error_maker, 1);
            assert_eq!(*delayed_vec.poll_state(), DataState::Updating(0.0.into()));
            assert!(delayed_vec.as_slice().is_empty());
            tokio::time::sleep(Duration::from_millis(200)).await;
            assert!(matches!(*delayed_vec.poll_state(), DataState::Error(_)));
            assert!(delayed_vec.as_slice().is_empty());
        });
    }
}
