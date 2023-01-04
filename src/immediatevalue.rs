use std::error::Error;
use std::future::Future;
use std::mem;
use std::ops::Deref;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Strong type to keep the boxed error. You can just deref it to get the inside box.
pub struct BoxedSendError(Box<dyn Error + Send>);
type FutureResult<T> = Result<T, BoxedSendError>;

impl<E: Error + Send + 'static> From<E> for BoxedSendError {
    fn from(e: E) -> Self {
        BoxedSendError(Box::new(e))
    }
}

impl Deref for BoxedSendError {
    type Target = Box<dyn Error + Send>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// # A promise which can be easily created and stored.
/// ## Introduction
/// Will spawn a task to resolve the future immediately. No possibility to read out intermediate values or communicate progress.
/// One can use `Option<ImmediateValuePromise<T>>` inside state structs to make this class somewhat lazy.
/// That may be an option if you don't need any progress indication or intermediate values.
/// After the calculation is done, `ImmediateValuePromise<T>` can be read out without mutability requirement using
/// [`ImmediateValuePromise::get_state`] which also yields an [`ImmediateValueState`] but without requiring `mut`.
/// Another useful feature after calculation is finished,
/// is that you can use [`ImmediateValuePromise::poll_mut`] to get a mutable [`ImmediateValueState`]
/// which allows you to take ownership of inner values with [`ImmediateValueState::take`] or get a mutable reference
/// to the inner via [`ImmediateValueState::as_mut`].
/// ## Examples
/// ### Basic usage
/// ```rust, no_run
/// use std::fs::File;
/// use std::thread;
/// use std::time::Duration;
/// use lazy_async_promise::{ImmediateValuePromise, ImmediateValueState};
/// let mut oneshot_val = ImmediateValuePromise::new(async {
///     tokio::time::sleep(Duration::from_millis(50)).await;
///     let test_error_handling = false;
///     if test_error_handling {
///       // We can use the ?-operator for most errors in our futures
///       let _file = File::open("I_DONT_EXIST_ERROR")?;
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
/// ### Modifying inner values or taking ownership
/// ```rust, no_run
/// use std::thread;
/// use std::time::Duration;
/// use lazy_async_promise::{ImmediateValuePromise, ImmediateValueState};
/// let mut oneshot_val = ImmediateValuePromise::new(async {
///     Ok(34)
/// });
/// thread::sleep(Duration::from_millis(50));
/// assert!(matches!(
///     oneshot_val.poll_state(),
///     ImmediateValueState::Success(_)
/// ));
/// let result = oneshot_val.poll_state_mut();
/// // we got the value, take a mutable ref
/// if let ImmediateValueState::Success(inner) = result {
///   *inner=32;
/// }
/// else {
///   unreachable!();
/// }
/// assert!(result.as_mut().is_some());
/// // take it out
/// let value = result.take();
/// assert_eq!(value.unwrap(), 32);
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
    Error(BoxedSendError),
    /// value has been taken out
    Empty,
}

impl<T> ImmediateValueState<T> {
    /// gets a mutable reference to the local cache if existing
    pub fn as_mut(&mut self) -> Option<&mut T> {
        match self {
            ImmediateValueState::Success(payload) => Some(payload),
            _ => None,
        }
    }
    /// Get the value if possible, [`None`] otherwise
    pub fn get_inner(&self) -> Option<&T> {
        if let ImmediateValueState::Success(inner) = self {
            Some(inner)
        }
        else {
            None
        }
    }

    /// Takes ownership of the inner value if ready, leaving self in state [`ImmediateValueState::Empty`].
    /// Does nothing if we are in any other state.
    pub fn take(&mut self) -> Option<T> {
        if matches!(self, ImmediateValueState::Success(_)) {
            let val = mem::replace(self, ImmediateValueState::Empty);
            return match val {
                ImmediateValueState::Success(inner) => { Some(inner) },
                _ => { None }
            }
        }
        None
    }
}

impl<T: Send> ImmediateValuePromise<T> {
    /// Creator, supply a future which returns `Result<T, Box<dyn Error + Send>`. Will be immediately spawned via tokio.
    pub fn new<U: Future<Output = Result<T, BoxedSendError>> + Send + 'static>(updater: U) -> Self {
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

    /// Poll the state updating the internal state from the running thread if possible, will return the data or error if ready or updating otherwise.
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

    /// Poll the state, return a mutable ref to to the state
    pub fn poll_state_mut(&mut self) -> &mut ImmediateValueState<T> {
        let _ = self.poll_state();
        &mut self.state
    }

    /// Get the current state without pulling. No mutability required
    pub fn get_state(&self) -> &ImmediateValueState<T> {
        &self.state
    }
}

#[cfg(test)]
mod test {
    use crate::immediatevalue::{ImmediateValuePromise, ImmediateValueState};
    use std::fs::File;
    use std::time::Duration;
    use tokio::runtime::Runtime;

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
            tokio::time::sleep(Duration::from_millis(100)).await;
            let result = oneshot_val.poll_state();
            if let ImmediateValueState::Success(val) = result {
                assert_eq!(*val, 34);
                return;
            }
            unreachable!();
        });
    }

    #[test]
    fn error() {
        Runtime::new().unwrap().block_on(async {
            let mut oneshot_val = ImmediateValuePromise::new(async {
                let some_result = File::open("DOES_NOT_EXIST");
                some_result?;
                Ok("bla".to_string())
            });
            assert!(matches!(
                oneshot_val.poll_state(),
                ImmediateValueState::Updating
            ));
            tokio::time::sleep(Duration::from_millis(50)).await;
            let result = oneshot_val.poll_state();
            if let ImmediateValueState::Error(e) = result {
                let _ = format!("{}", **e);
                return;
            }
            unreachable!();
        });
    }

    #[test]
    fn get_state() {
        Runtime::new().unwrap().block_on(async {
            let mut oneshot_val = ImmediateValuePromise::new(async {
                Ok("bla".to_string())
            });
            // get value does not trigger any polling
            let state = oneshot_val.get_state();
            assert!(matches!(
                state,
                ImmediateValueState::Updating
            ));
            tokio::time::sleep(Duration::from_millis(50)).await;
            let state = oneshot_val.get_state();
            assert!(matches!(
                state,
                ImmediateValueState::Updating
            ));

            let polled = oneshot_val.poll_state();
            assert_eq!(polled.get_inner().unwrap(), "bla");
        });
    }

    #[test]
    fn get_mut_take_value() {
        Runtime::new().unwrap().block_on(async {
            let mut oneshot_val = ImmediateValuePromise::new(async {
                Ok("bla".to_string())
            });
            tokio::time::sleep(Duration::from_millis(50)).await;
            {
                // get value does not trigger any polling
                let result = oneshot_val.poll_state_mut();
                // we got the value
                if let Some(inner) = result.as_mut() {
                    assert_eq!(inner, "bla");
                    // write back
                    *inner = "changed".to_string();
                }
                else {
                    unreachable!();
                }
                let result = oneshot_val.poll_state_mut();
                // take it out, should be changed and owned
                let value = result.take();
                assert_eq!(value.unwrap().as_str(), "changed");
                assert!(matches!(result, ImmediateValueState::Empty));
            }
            // afterwards we are empty on get and poll
            assert!(matches!(oneshot_val.get_state(), ImmediateValueState::Empty));
            assert!(matches!(oneshot_val.poll_state(), ImmediateValueState::Empty));
        });
    }
}
