use crate::{DirectCacheAccess, Progress};
use crate::{ImmediateValuePromise, ImmediateValueState};
use std::borrow::Cow;
use tokio::sync::mpsc::Receiver;
use tokio::sync::mpsc::Sender;
use tokio::time::Instant;

#[derive(Debug)]
pub struct Status<M> {
    pub time: Instant,
    pub progress: Progress,
    pub message: M,
}

impl<M> Status<M> {
    pub fn new(progress: Progress, message: M) -> Self {
        Self {
            progress,
            message,
            time: Instant::now(),
        }
    }
}

pub type StringStatus = Status<Cow<'static, str>>;

impl StringStatus {
    pub fn from_str(progress: Progress, static_message: &'static str) -> Self {
        StringStatus {
            message: Cow::Borrowed(static_message),
            time: Instant::now(),
            progress,
        }
    }
    pub fn from_string(progress: Progress, message: String) -> Self {
        StringStatus {
            message: Cow::Owned(message),
            time: Instant::now(),
            progress,
        }
    }
}

/// # A progress and status enabling wrapper for [`ImmediateValuePromise`]
/// This struct allows to use the [`Progress`] type and any kind of status message
///
pub struct ProgressTrackedImValProm<T: Send, M> {
    promise: ImmediateValuePromise<T>,
    status: Vec<Status<M>>,
    receiver: Receiver<Status<M>>,
}

impl<T: Send + 'static, M> ProgressTrackedImValProm<T, M> {
    pub fn new(
        creator: impl FnOnce(Sender<Status<M>>) -> ImmediateValuePromise<T>,
        buffer: usize,
    ) -> Self {
        let (sender, receiver) = tokio::sync::mpsc::channel(buffer);
        ProgressTrackedImValProm {
            receiver,
            status: Vec::new(),
            promise: creator(sender),
        }
    }

    pub fn finished(&self) -> bool {
        self.promise.get_value().is_some()
    }

    pub fn poll_state(&mut self) -> &ImmediateValueState<T> {
        while let Ok(msg) = self.receiver.try_recv() {
            self.status.push(msg);
        }
        self.promise.poll_state()
    }

    pub fn progress(&self) -> Progress {
        self.status
            .last()
            .map(|p| p.progress)
            .unwrap_or(Progress::default())
    }
}

impl<T: Send + 'static, M> DirectCacheAccess<T> for ProgressTrackedImValProm<T, M> {
    fn get_value_mut(&mut self) -> Option<&mut T> {
        self.promise.get_value_mut()
    }
    fn get_value(&self) -> Option<&T> {
        self.promise.get_value()
    }
    fn take_value(&mut self) -> Option<T> {
        self.promise.take_value()
    }
}
#[cfg(test)]
mod test {
    use super::*;
    use crate::ImmediateValuePromise;
    use std::time::Duration;
    use tokio::runtime::Runtime;
    #[test]
    fn basic_usage_cycle() {
        Runtime::new().unwrap().block_on(async {
            let mut oneshot_progress = ProgressTrackedImValProm::new(
                |s| {
                    ImmediateValuePromise::new(async move {
                        s.send(StringStatus::new(
                            Progress::from_percent(0.0),
                            Cow::Borrowed("Initializing"),
                        ))
                        .await
                        .unwrap();
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        s.send(StringStatus::new(
                            Progress::from_percent(100.0),
                            Cow::Borrowed("Done"),
                        ))
                        .await
                        .unwrap();
                        Ok(34)
                    })
                },
                2000,
            );
            assert!(matches!(
                oneshot_progress.poll_state(),
                ImmediateValueState::Updating
            ));
            assert_eq!(*oneshot_progress.progress(), 0.0);
            tokio::time::sleep(Duration::from_millis(100)).await;
            let _ = oneshot_progress.poll_state();
            assert_eq!(*oneshot_progress.progress(), 1.0);
            let result = oneshot_progress.poll_state();

            if let ImmediateValueState::Success(val) = result {
                assert_eq!(*val, 34);
                return;
            }

            unreachable!();
        });
    }
}
