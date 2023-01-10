use crate::{
    box_future_factory, BoxedFutureFactory, DataState, DirectCacheAccess, Message, Promise,
};
use std::fmt::Debug;
use std::future::Future;
use std::mem;
use tokio::sync::mpsc::{channel, Receiver, Sender};

/// # A lazy, async and partially readable vector promise
/// This promise is the right one for async acquiring of lists which should be partially readable on each frame.
/// Imagine slowly streaming data and wanting to read them out as far as they are available each frame.
/// Examples:
/// ```rust, no_run
/// use std::time::Duration;
/// use tokio::sync::mpsc::Sender;
/// use lazy_async_promise::{DataState, Message, Promise, LazyVecPromise, api_macros::*};
/// // updater-future:
/// let updater = |tx: Sender<Message<i32>>| async move {
///   const ITEM_COUNT: i32 = 100;
///   for i in 0..ITEM_COUNT {
///     send_data!(i, tx);
///     set_progress!(Progress::from_fraction(i, ITEM_COUNT), tx);
///     // how to handle results and propagate the error to the future? Use `unpack_result!`:
///     let string = unpack_result!(std::fs::read_to_string("whatever.txt"), tx);
///     if i > 100 {
///       set_error!("loop overflow".to_owned(), tx);
///     }
///    tokio::time::sleep(Duration::from_millis(100)).await;
///   }
///   set_finished!(tx);
/// };
/// // direct usage:
/// let promise = LazyVecPromise::new(updater, 200);
///
/// fn main_loop(lazy_promise: &mut LazyVecPromise<i32>) {
///   loop {
///     let state = lazy_promise.poll_state();
///     let progress = state.get_progress().unwrap_or_default();
///     match state {
///       DataState::Error(er)  => { println!("Error {} occurred! Retrying!", er); std::thread::sleep(Duration::from_millis(50)); lazy_promise.update(); }
///       DataState::UpToDate   => { println!("Data complete: {:?}", lazy_promise.as_slice()); }
///                           _ => { println!("Getting data, might be partially ready! (part: {:?} - progress: {}", lazy_promise.as_slice(), progress.as_f32()); }  
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

    /// Get the current data as mutable slice
    pub fn as_slice_mut(&mut self) -> &mut [T] {
        self.data.as_mut_slice()
    }

    #[cfg(test)]
    pub fn is_uninitialized(&self) -> bool {
        self.state == DataState::Uninitialized
    }
}

impl<T: Debug> DirectCacheAccess<Vec<T>> for LazyVecPromise<T> {
    fn get_value_mut(&mut self) -> Option<&mut Vec<T>> {
        Some(&mut self.data)
    }

    fn get_value(&self) -> Option<&Vec<T>> {
        Some(&self.data)
    }

    /// Take the current data. If state was  [`DataState::UpToDate`] it will return the value.
    /// If the state was anything else, it will return None. If data is taken successfully, will leave
    /// the object in state [`DataState::Uninitialized`]
    fn take(&mut self) -> Option<Vec<T>> {
        if self.state == DataState::UpToDate {
            self.state = DataState::Uninitialized;
            Some(mem::take(&mut self.data))
        } else {
            None
        }
    }
}

impl<T: Debug> Promise for LazyVecPromise<T> {
    fn poll_state(&mut self) -> &DataState {
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

        if self.state == DataState::Uninitialized {
            self.update();
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
    use crate::api_macros::*;
    use std::time::Duration;
    use tokio::runtime::{Builder, Runtime};
    use tokio::sync::mpsc::Sender;

    #[test]
    fn basic_usage_cycle() {
        let string_maker = |tx: Sender<Message<String>>| async move {
            const COUNT: i32 = 5;
            for i in 0..COUNT {
                send_data!(i.to_string(), tx);
                set_progress!(Progress::from_fraction(i, COUNT), tx);
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            set_finished!(tx);
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
                tokio::time::sleep(Duration::from_millis(80)).await;
                let progress = delayed_vec.poll_state().get_progress().unwrap();
                assert!(progress.as_f32() > 0.0);
                assert!(progress.as_f32() < 1.0);
                assert!(!delayed_vec.as_slice().is_empty());

                // after wait we have a result
                tokio::time::sleep(Duration::from_millis(200)).await;
                assert_eq!(*delayed_vec.poll_state(), DataState::UpToDate);
                assert_eq!(delayed_vec.as_slice().len(), 5);
                // after update it's empty again
                delayed_vec.update();
                assert_eq!(*delayed_vec.poll_state(), DataState::Updating(0.0.into()));
                assert!(delayed_vec.as_slice().is_empty());
                // finally after waiting it's full again
                tokio::time::sleep(Duration::from_millis(400)).await;
                assert_eq!(*delayed_vec.poll_state(), DataState::UpToDate);
                assert_eq!(delayed_vec.as_slice().len(), 5);
            });
    }

    #[test]
    fn error_propagation_returns_early() {
        let error_maker = |tx: Sender<Message<String>>| async move {
            let _ = unpack_result!(std::fs::read_to_string("NOT_EXISTING"), tx);
            unreachable!();
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
