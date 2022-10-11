use std::error::Error;
use std::future::Future;
use std::sync::Arc;
use tokio::sync::Mutex;

type DynErrorSend = Box<dyn Error + Send>;
type FutureResult<T> = Result<T, DynErrorSend>;

/// Allows to use into_boxed for map_err constructs more easily.
/// Blanked implementation for all `Error + Send + 'static` exists.
pub trait ToDynSendBox {
    /// turns this value into a `Box<dyn Error + Send>`
    fn into_boxed(self) -> Box<dyn Error + Send>;
}

impl<E: Error + Send + 'static> ToDynSendBox for E {
    fn into_boxed(self) -> Box<dyn Error + Send> {
        let boxed: Box<dyn Error + Send> = Box::new(self);
        boxed
    }
}

/// # A promise which can be easily created and stored.
/// Will spawn a task to resolve the future immediately. No possibility to read out interim values.
/// ```rust, no_run
/// use std::fs::File;
/// use std::thread;
/// use std::time::Duration;
/// use lazy_async_promise::{ImmediateValuePromise, ImmediateValueState, ToDynSendBox};
/// let mut oneshot_val = ImmediateValuePromise::new(async {
///     tokio::time::sleep(Duration::from_millis(50)).await;
///     let test_error_handling = false;
///     if test_error_handling {
///       // We can use `into_boxed` for most errors to use the ?-Operator in our futures
///       let _file = File::open("I_DONT_EXIST_ERROR").map_err(|e| e.into_boxed())?;
///     }
///     // return the value wrapped in Ok for the result here
///     Ok(34)
/// });
/// assert!(matches!(
///     oneshot_val.poll_state(),
///     ImmediateValueState::Updating
/// ));
/// thread::sleep(Duration::from_millis(100));
/// let result = oneshot_val.poll_state();
/// if let ImmediateValueState::Success(val) = result {
///     assert_eq!(*val, 34);
/// } else {
///     unreachable!();
/// }
/// ```
pub struct ImmediateValuePromise<T: Send + 'static> {
    value_arc: Arc<Mutex<Option<FutureResult<T>>>>,
    state: ImmediateValueState<T>,
}

/// The return state of a [`ImmediateValuePromise`], contains the error, the value or that it is still updating
pub enum ImmediateValueState<T> {
    /// future is not yet resolved
    Updating,
    /// future resolved successfully
    Success(T),
    /// resolving the future failed somehow
    Error(Box<dyn Error + Send>),
}

impl<T: Send> ImmediateValuePromise<T> {
    /// Creator, supply a future which returns `Result<T, Box<dyn Error + Send>`. Will be immediately spawned.
    pub fn new<U: Future<Output = Result<T, Box<dyn Error + Send>>> + Send + 'static>(
        updater: U,
    ) -> Self {
        let arc = Arc::new(Mutex::new(None));
        let arc_clone = arc.clone();
        tokio::spawn(async move {
            let mut val = arc_clone.lock().await;
            *val = Some(updater.await);
        });
        Self {
            value_arc: arc,
            state: ImmediateValueState::Updating,
        }
    }

    /// Poll the state, will return the data or error if ready or updating otherwise.
    pub fn poll_state(&mut self) -> &ImmediateValueState<T> {
        if matches!(self.state, ImmediateValueState::Updating) {
            let value = self.value_arc.try_lock();
            if let Ok(mut guard) = value {
                if let Some(result) = guard.take() {
                    match result {
                        Ok(value) => self.state = ImmediateValueState::Success(value),
                        Err(e) => self.state = ImmediateValueState::Error(e),
                    };
                }
            }
        }
        &self.state
    }
}

#[cfg(test)]
mod test {
    use crate::immediatevalue::{ImmediateValuePromise, ImmediateValueState, ToDynSendBox};
    use std::error::Error;
    use std::fmt::{Display, Formatter};
    use std::thread;
    use std::time::Duration;
    use tokio::runtime::Runtime;

    #[derive(Debug)]
    struct TestError {
        code: i32,
    }
    unsafe impl Send for TestError {}
    unsafe impl Sync for TestError {}

    impl Display for TestError {
        fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
            write!(f, "code: {}", self.code)
        }
    }
    impl Error for TestError {}

    #[test]
    fn default() {
        Runtime::new().unwrap().block_on(async {
            let mut oneshot_val = ImmediateValuePromise::new(async {
                tokio::time::sleep(Duration::from_millis(50)).await;
                Ok(34)
            });
            assert!(matches!(
                oneshot_val.poll_state(),
                ImmediateValueState::Updating
            ));
            thread::sleep(Duration::from_millis(100));
            let result = oneshot_val.poll_state();
            if let ImmediateValueState::Success(val) = result {
                assert_eq!(*val, 34);
            } else {
                unreachable!();
            }
        });
    }

    #[test]
    fn error() {
        Runtime::new().unwrap().block_on(async {
            let mut oneshot_val = ImmediateValuePromise::new(async {
                let some_result = Err(TestError { code: 0 });
                some_result.map_err(|e| e.into_boxed())?;
                Ok("bla".to_string())
            });
            assert!(matches!(
                oneshot_val.poll_state(),
                ImmediateValueState::Updating
            ));
            thread::sleep(Duration::from_millis(50));
            let result = oneshot_val.poll_state();
            assert!(matches!(result, ImmediateValueState::Error(_)));
        });
    }
}
