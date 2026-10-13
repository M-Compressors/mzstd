//! Parallel compression of one frame with caller-run tasks.
//!
//! The input is cut into fixed segments, each compressed by its own task
//! into blocks of one frame; tasks are handed to the caller, who runs them on
//! whatever threads it likes. The frame is a standard Zstandard frame any
//! decoder accepts. Its bytes depend on the configuration and the segment
//! size, never on how many tasks there were or in which order they ran — but
//! they differ from the serial encoder's, and from the reference's
//! multithreaded mode.
//!
//! An input that fits in one segment is compressed serially by
//! [`ParallelCompressor::prepare_slice`] and matches
//! [`Compressor::compress`](crate::Compressor::compress).
//!
//! # Examples
//!
//! ```
//! use mzstd::compressor::parallel::{ParallelCompressor, ParallelConfig, TaskCount};
//! use mzstd::EncoderConfig;
//!
//! let mut compressor = ParallelCompressor::new(EncoderConfig::default(), ParallelConfig::default())?;
//! let input = vec![7u8; 10 << 20];
//! let mut batch = compressor.prepare_slice(&input, TaskCount::try_from(4)?);
//! std::thread::scope(|scope| {
//!     for task in batch.take_tasks() {
//!         scope.spawn(move || task.run());
//!     }
//! });
//! let frame = batch.finish()?;
//! assert!(frame.len() < input.len());
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

pub(crate) mod core;

use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};

use thiserror::Error;

use self::core::{FramePlan, compress_segment};
use super::config::{ConfigError, EncoderConfig, Parameter};
use super::core::cctx::{Engine, RequestedParams, compress_bound};
use super::error::EncodeError;
use super::internal::requested_params;
use super::session::{InputSize, StreamConfig};
use crate::shared::xxhash::Xxh64;

/// Size of the segments a frame is cut into.
///
/// # Examples
///
/// ```
/// use mzstd::compressor::parallel::SegmentSize;
///
/// assert_eq!(SegmentSize::DEFAULT.get(), 4 << 20);
/// assert!(SegmentSize::try_from(1000).is_err());
/// ```
#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct SegmentSize(usize);

impl SegmentSize {
    /// The smallest segment, 64 KiB.
    pub const MIN: Self = Self(64 << 10);
    /// The largest segment, 256 MiB.
    pub const MAX: Self = Self(256 << 20);
    /// The default segment, 4 MiB.
    pub const DEFAULT: Self = Self(4 << 20);

    /// The size in bytes.
    #[must_use]
    pub const fn get(self) -> usize {
        self.0
    }
}

impl Default for SegmentSize {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl TryFrom<usize> for SegmentSize {
    type Error = ConfigError;

    /// Validates a segment size.
    ///
    /// # Errors
    ///
    /// [`ConfigError::OutOfRange`] outside 64 KiB to 256 MiB.
    fn try_from(value: usize) -> Result<Self, Self::Error> {
        if !(Self::MIN.0..=Self::MAX.0).contains(&value) {
            return Err(ConfigError::OutOfRange {
                parameter: Parameter::SegmentSize,
                value: value as i64,
                min: Self::MIN.0 as i64,
                max: Self::MAX.0 as i64,
            });
        }
        Ok(Self(value))
    }
}

/// How many tasks a batch is split into, at most.
///
/// # Examples
///
/// ```
/// use mzstd::compressor::parallel::TaskCount;
///
/// assert_eq!(TaskCount::try_from(8)?.get(), 8);
/// assert!(TaskCount::try_from(0).is_err());
/// # Ok::<(), mzstd::ConfigError>(())
/// ```
#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct TaskCount(usize);

impl TaskCount {
    /// One task.
    pub const ONE: Self = Self(1);
    /// The largest task count.
    pub const MAX: Self = Self(1 << 16);

    /// The count.
    #[must_use]
    pub const fn get(self) -> usize {
        self.0
    }
}

impl Default for TaskCount {
    fn default() -> Self {
        Self::ONE
    }
}

impl TryFrom<usize> for TaskCount {
    type Error = ConfigError;

    /// Validates a task count.
    ///
    /// # Errors
    ///
    /// [`ConfigError::OutOfRange`] outside `1..=65536`.
    fn try_from(value: usize) -> Result<Self, Self::Error> {
        if value == 0 || value > Self::MAX.0 {
            return Err(ConfigError::OutOfRange {
                parameter: Parameter::TaskCount,
                value: value as i64,
                min: 1,
                max: Self::MAX.0 as i64,
            });
        }
        Ok(Self(value))
    }
}

/// Configuration of parallel compression.
///
/// # Examples
///
/// ```
/// use mzstd::compressor::parallel::{ParallelConfig, SegmentSize};
///
/// let config = ParallelConfig::default().with_segment_size(SegmentSize::try_from(1 << 20)?);
/// assert_eq!(config.segment_size().get(), 1 << 20);
/// # Ok::<(), mzstd::ConfigError>(())
/// ```
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq, Hash)]
pub struct ParallelConfig {
    segment_size: SegmentSize,
}

impl ParallelConfig {
    /// Sets the segment size.
    #[must_use]
    pub const fn with_segment_size(mut self, segment_size: SegmentSize) -> Self {
        self.segment_size = segment_size;
        self
    }

    /// The segment size.
    #[must_use]
    pub const fn segment_size(&self) -> SegmentSize {
        self.segment_size
    }
}

/// Error of a parallel compression.
///
/// # Examples
///
/// ```
/// use mzstd::compressor::parallel::{ParallelCompressor, ParallelConfig, ParallelEncodeError, TaskCount};
///
/// let mut compressor = ParallelCompressor::new(Default::default(), ParallelConfig::default())?;
/// let input = vec![1u8; 9 << 20];
/// let mut session = compressor.start(mzstd::InputSize::Exact(4).into());
/// assert!(matches!(
///     session.push(&input),
///     Err(ParallelEncodeError::InputSizeMismatch { declared: 4, .. })
/// ));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Error, Debug, Clone, Eq, PartialEq)]
#[non_exhaustive]
pub enum ParallelEncodeError {
    /// A segment failed to compress.
    #[error("a segment failed to compress")]
    Encode(#[from] EncodeError),
    /// The input does not match the declared size.
    #[error("the stream was declared as {declared} bytes but received {actual}")]
    InputSizeMismatch {
        /// The declared size.
        declared: u64,
        /// The bytes received.
        actual: u64,
    },
    /// The session has already finished or failed.
    #[error("the session cannot accept this operation in its current state")]
    InvalidState,
}

/// Engines shared by the tasks of one compressor.
#[derive(Debug, Default)]
struct EnginePool {
    engines: Mutex<Vec<Box<Engine>>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl EnginePool {
    fn take(&self) -> Box<Engine> {
        lock(&self.engines)
            .pop()
            .unwrap_or_else(|| Box::new(Engine::empty()))
    }

    fn give(&self, engine: Box<Engine>) {
        lock(&self.engines).push(engine);
    }

    fn retained_bytes(&self) -> usize {
        lock(&self.engines)
            .iter()
            .map(|engine| engine.retained_bytes())
            .sum()
    }
}

/// A compressor that splits a frame across caller-run tasks.
///
/// # Examples
///
/// ```
/// use mzstd::compressor::parallel::{ParallelCompressor, ParallelConfig};
///
/// let mut compressor = ParallelCompressor::new(Default::default(), ParallelConfig::default())?;
/// let frame = compressor.compress(b"small inputs are compressed serially")?;
/// assert_eq!(frame, mzstd::Compressor::new(Default::default())?.compress(b"small inputs are compressed serially")?);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug)]
pub struct ParallelCompressor {
    config: EncoderConfig,
    requested: RequestedParams,
    parallel: ParallelConfig,
    pool: Arc<EnginePool>,
}

impl ParallelCompressor {
    /// Creates a parallel compressor.
    ///
    /// # Errors
    ///
    /// [`ConfigError`] when a parameter is out of range.
    pub fn new(config: EncoderConfig, parallel: ParallelConfig) -> Result<Self, ConfigError> {
        config.validate()?;
        Ok(Self {
            config,
            requested: requested_params(&config),
            parallel,
            pool: Arc::new(EnginePool::default()),
        })
    }

    /// The encoder configuration.
    #[must_use]
    pub const fn config(&self) -> &EncoderConfig {
        &self.config
    }

    /// The parallel configuration.
    #[must_use]
    pub const fn parallel_config(&self) -> &ParallelConfig {
        &self.parallel
    }

    /// Bytes retained by idle engines.
    #[must_use]
    pub fn retained_bytes(&self) -> usize {
        self.pool.retained_bytes()
    }

    /// Releases every idle engine.
    pub fn release(&mut self) {
        lock(&self.pool.engines).clear();
    }

    /// Plans the compression of `input` into at most `tasks` tasks.
    ///
    /// Segments are grouped contiguously; the grouping does not change the
    /// frame.
    pub fn prepare_slice<'a>(
        &mut self,
        input: &'a [u8],
        tasks: TaskCount,
    ) -> ScopedParallelBatch<'a> {
        let segment = self.parallel.segment_size.get();
        let shared = Arc::new(BatchShared::default());
        let plan = FramePlan::new(&self.requested, Some(input.len() as u64));
        if input.len() <= segment {
            let task = ScopedParallelTask {
                work: Work::Serial {
                    requested: self.requested,
                    input,
                },
                index: 0,
                shared: Arc::clone(&shared),
                pool: Arc::clone(&self.pool),
            };
            return ScopedParallelBatch {
                tasks: vec![task],
                shared,
                expected: 1,
                serial: true,
                plan,
                input,
            };
        }
        let segments = input.len().div_ceil(segment);
        let task_count = tasks.get().min(segments);
        let per_task = segments / task_count;
        let extra = segments % task_count;
        let mut list = Vec::with_capacity(task_count);
        let mut first = 0;
        for t in 0..task_count {
            let count = per_task + usize::from(t < extra);
            list.push(ScopedParallelTask {
                work: Work::Segments {
                    plan,
                    input,
                    first,
                    count,
                    segment,
                },
                index: t,
                shared: Arc::clone(&shared),
                pool: Arc::clone(&self.pool),
            });
            first += count;
        }
        ScopedParallelBatch {
            tasks: list,
            shared,
            expected: task_count,
            serial: false,
            plan,
            input,
        }
    }

    /// Compresses `input`, running every task on the calling thread.
    ///
    /// # Errors
    ///
    /// [`ParallelEncodeError::Encode`] when a segment fails.
    pub fn compress(&mut self, input: &[u8]) -> Result<Vec<u8>, ParallelEncodeError> {
        self.prepare_slice(input, TaskCount::ONE).finish()
    }

    /// Starts a frame whose input arrives in chunks.
    ///
    /// # Examples
    ///
    /// ```
    /// use mzstd::compressor::parallel::{ParallelCompressor, ParallelConfig};
    ///
    /// let mut compressor = ParallelCompressor::new(Default::default(), ParallelConfig::default())?;
    /// let mut session = compressor.start(Default::default());
    /// let queue = session.queue();
    /// session.push(&vec![3u8; 5 << 20])?;
    /// while let Some(task) = queue.next_task() {
    ///     task.run();
    /// }
    /// let mut frame = Vec::new();
    /// session.finish_into(&mut frame)?;
    /// assert!(!frame.is_empty());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn start(&mut self, stream: StreamConfig) -> ParallelEncoderSession<'_> {
        let declared = match stream.input_size() {
            InputSize::Exact(size) => Some(size),
            InputSize::Unknown => None,
        };
        let plan = FramePlan::new(&self.requested, declared);
        ParallelEncoderSession {
            compressor: self,
            plan,
            declared,
            queue: ParallelTaskQueue::default(),
            pending: Vec::new(),
            overlap: Vec::new(),
            received: 0,
            hasher: Xxh64::new(0),
            next_segment: 0,
            delivered: 0,
            header_written: false,
            state: SessionState::Open,
        }
    }
}

/// One task's result: its index and its blocks.
type Slot = (usize, Result<Vec<u8>, EncodeError>);

/// Results of a batch, one slot per task.
#[derive(Debug, Default)]
struct BatchShared {
    slots: Mutex<Vec<Slot>>,
}

/// What a scoped task compresses.
#[derive(Debug)]
enum Work<'a> {
    /// The whole input, serially.
    Serial {
        requested: RequestedParams,
        input: &'a [u8],
    },
    /// `count` segments starting at segment `first`.
    Segments {
        plan: FramePlan,
        input: &'a [u8],
        first: usize,
        count: usize,
        segment: usize,
    },
}

/// One unit of work of a batch; run it on any thread.
#[derive(Debug)]
pub struct ScopedParallelTask<'a> {
    work: Work<'a>,
    index: usize,
    shared: Arc<BatchShared>,
    pool: Arc<EnginePool>,
}

impl ScopedParallelTask<'_> {
    /// Compresses this task's segments.
    pub fn run(self) {
        let mut engine = self.pool.take();
        let result = match self.work {
            Work::Serial { requested, input } => {
                let bound = compress_bound(input.len()).unwrap_or(0);
                let mut out = vec![0u8; bound];
                match super::core::compress_once_into(&mut engine, &requested, input, 1, &mut out) {
                    Ok(size) => {
                        out.truncate(size);
                        Ok(out)
                    }
                    // The destination holds any frame, so this arm is a core
                    // bug reported as an error rather than a panic.
                    Err(error) => Err(EncodeError::from_core(
                        error,
                        bound,
                        Some(input.len() as u64),
                    )),
                }
            }
            Work::Segments {
                plan,
                input,
                first,
                count,
                segment,
            } => {
                let overlap = plan.overlap();
                let mut out = Vec::new();
                let mut result = Ok(());
                for s in first..first + count {
                    let start = s * segment;
                    let end = (start + segment).min(input.len());
                    let from = start.saturating_sub(overlap);
                    match compress_segment(&mut engine, &plan, &input[from..end], start - from) {
                        Ok(bytes) => out.extend_from_slice(&bytes),
                        Err(error) => {
                            result = Err(EncodeError::from_core(error, 0, None));
                            break;
                        }
                    }
                }
                result.map(|()| out)
            }
        };
        self.pool.give(engine);
        lock(&self.shared.slots).push((self.index, result));
    }
}

/// A planned compression of one input.
///
/// Tasks taken with [`Self::take_tasks`] should be run before
/// [`Self::finish`]; any that were not taken are run by `finish` on the
/// calling thread.
#[derive(Debug)]
pub struct ScopedParallelBatch<'a> {
    tasks: Vec<ScopedParallelTask<'a>>,
    shared: Arc<BatchShared>,
    expected: usize,
    serial: bool,
    plan: FramePlan,
    input: &'a [u8],
}

impl<'a> ScopedParallelBatch<'a> {
    /// Takes the tasks.
    pub fn take_tasks(&mut self) -> Vec<ScopedParallelTask<'a>> {
        ::core::mem::take(&mut self.tasks)
    }

    /// Assembles the frame.
    ///
    /// # Errors
    ///
    /// [`ParallelEncodeError::InvalidState`] when taken tasks were dropped
    /// without running, and the failure of a task.
    pub fn finish(self) -> Result<Vec<u8>, ParallelEncodeError> {
        let mut out = Vec::new();
        self.finish_into(&mut out)?;
        Ok(out)
    }

    /// Appends the frame to `out`.
    ///
    /// # Errors
    ///
    /// As [`Self::finish`].
    pub fn finish_into(mut self, out: &mut Vec<u8>) -> Result<(), ParallelEncodeError> {
        for task in ::core::mem::take(&mut self.tasks) {
            task.run();
        }
        let mut slots = ::core::mem::take(&mut *lock(&self.shared.slots));
        if slots.len() != self.expected {
            return Err(ParallelEncodeError::InvalidState);
        }
        slots.sort_by_key(|(index, _)| *index);
        if self.serial {
            for (_, result) in slots {
                out.extend_from_slice(&result?);
            }
            return Ok(());
        }
        let start = out.len();
        out.extend_from_slice(&self.plan.header());
        for (_, result) in slots {
            match result {
                Ok(bytes) => out.extend_from_slice(&bytes),
                Err(error) => {
                    out.truncate(start);
                    return Err(error.into());
                }
            }
        }
        let mut hasher = Xxh64::new(0);
        if self.plan.applied.fparams.checksum {
            hasher.update(self.input);
        }
        out.extend_from_slice(&self.plan.epilogue(&hasher));
        Ok(())
    }
}

/// A segment compression queued by a streaming session.
#[derive(Debug)]
pub struct OwnedParallelTask {
    plan: FramePlan,
    data: Vec<u8>,
    overlap: usize,
    index: u64,
    queue: Arc<QueueShared>,
    pool: Arc<EnginePool>,
}

/// Records a task's completion even when the task unwinds, so a session
/// waiting for it is not left hanging.
struct Completion<'q> {
    queue: &'q QueueShared,
    index: u64,
    result: Option<Result<Vec<u8>, EncodeError>>,
}

impl Drop for Completion<'_> {
    fn drop(&mut self) {
        let result = self.result.take().unwrap_or(Err(EncodeError::Internal));
        let mut state = lock(&self.queue.state);
        state.running -= 1;
        state.done.push((self.index, result));
        self.queue.changed.notify_all();
    }
}

impl OwnedParallelTask {
    /// Compresses the segment.
    pub fn run(self) {
        let mut completion = Completion {
            queue: &self.queue,
            index: self.index,
            result: None,
        };
        let mut engine = self.pool.take();
        // Segments size their own buffers, so an error is a core bug
        // reported rather than a panic.
        let result = match compress_segment(&mut engine, &self.plan, &self.data, self.overlap) {
            Ok(blocks) => Ok(blocks),
            Err(error) => Err(EncodeError::from_core(error, 0, None)),
        };
        self.pool.give(engine);
        completion.result = Some(result);
    }
}

#[derive(Debug, Default)]
struct QueueState {
    queued: VecDeque<OwnedParallelTask>,
    running: usize,
    done: Vec<(u64, Result<Vec<u8>, EncodeError>)>,
}

#[derive(Debug, Default)]
struct QueueShared {
    state: Mutex<QueueState>,
    changed: Condvar,
}

/// Where a streaming session's tasks wait to be run; clone it into worker
/// threads.
#[derive(Debug, Clone, Default)]
pub struct ParallelTaskQueue {
    shared: Arc<QueueShared>,
}

impl ParallelTaskQueue {
    /// Takes the next queued task, if any.
    #[must_use]
    pub fn next_task(&self) -> Option<OwnedParallelTask> {
        let mut state = lock(&self.shared.state);
        let task = state.queued.pop_front()?;
        state.running += 1;
        Some(task)
    }

    /// Number of tasks queued and not yet taken.
    #[must_use]
    pub fn queued(&self) -> usize {
        lock(&self.shared.state).queued.len()
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum SessionState {
    Open,
    Finished,
    Failed,
}

/// A parallel frame whose input arrives in chunks.
///
/// Every full segment becomes a task in the session's
/// [`ParallelTaskQueue`]; finished segments are delivered in order by
/// [`Self::drain_into`]. The caller bounds memory by running tasks and
/// draining output as it pushes input; [`ParallelTaskQueue::queued`] reports
/// the backlog.
#[derive(Debug)]
pub struct ParallelEncoderSession<'c> {
    compressor: &'c mut ParallelCompressor,
    plan: FramePlan,
    declared: Option<u64>,
    queue: ParallelTaskQueue,
    /// Input of the segment being gathered.
    pending: Vec<u8>,
    /// The tail of earlier segments a new segment may reach into.
    overlap: Vec<u8>,
    received: u64,
    hasher: Xxh64,
    next_segment: u64,
    delivered: u64,
    header_written: bool,
    state: SessionState,
}

impl ParallelEncoderSession<'_> {
    /// The queue of this session's tasks.
    #[must_use]
    pub fn queue(&self) -> ParallelTaskQueue {
        self.queue.clone()
    }

    fn enqueue_pending(&mut self) {
        let overlap_len = self.overlap.len();
        let mut data = Vec::with_capacity(overlap_len + self.pending.len());
        data.extend_from_slice(&self.overlap);
        data.extend_from_slice(&self.pending);
        let keep = self.plan.overlap().min(data.len());
        self.overlap = data[data.len() - keep..].to_vec();
        self.pending.clear();
        let task = OwnedParallelTask {
            plan: self.plan,
            data,
            overlap: overlap_len,
            index: self.next_segment,
            queue: Arc::clone(&self.queue.shared),
            pool: Arc::clone(&self.compressor.pool),
        };
        self.next_segment += 1;
        lock(&self.queue.shared.state).queued.push_back(task);
    }

    /// Accepts input; every full segment is queued as a task.
    ///
    /// # Errors
    ///
    /// [`ParallelEncodeError::InvalidState`] after the session finished or
    /// failed, and [`ParallelEncodeError::InputSizeMismatch`] beyond a
    /// declared size, which fails the session.
    pub fn push(&mut self, mut input: &[u8]) -> Result<(), ParallelEncodeError> {
        if self.state != SessionState::Open {
            return Err(ParallelEncodeError::InvalidState);
        }
        let received = self.received + input.len() as u64;
        if let Some(declared) = self.declared
            && received > declared
        {
            self.state = SessionState::Failed;
            return Err(ParallelEncodeError::InputSizeMismatch {
                declared,
                actual: received,
            });
        }
        self.received = received;
        if self.plan.applied.fparams.checksum {
            self.hasher.update(input);
        }
        let segment = self.compressor.parallel.segment_size.get();
        while !input.is_empty() {
            let take = (segment - self.pending.len()).min(input.len());
            self.pending.extend_from_slice(&input[..take]);
            input = &input[take..];
            if self.pending.len() == segment {
                self.enqueue_pending();
            }
        }
        Ok(())
    }

    /// Runs queued tasks on the calling thread until none is left.
    pub fn run_queued(&self) {
        while let Some(task) = self.queue.next_task() {
            task.run();
        }
    }

    /// Appends the frame header and every segment that is ready, in order,
    /// to `out`; returns how many bytes were appended.
    ///
    /// # Errors
    ///
    /// The failure of a task, which fails the session.
    pub fn drain_into(&mut self, out: &mut Vec<u8>) -> Result<usize, ParallelEncodeError> {
        let start = out.len();
        if !self.header_written {
            out.extend_from_slice(&self.plan.header());
            self.header_written = true;
        }
        let mut state = lock(&self.queue.shared.state);
        state
            .done
            .sort_by_key(|(index, _)| ::core::cmp::Reverse(*index));
        while state
            .done
            .last()
            .is_some_and(|(index, _)| *index == self.delivered)
        {
            if let Some((_, result)) = state.done.pop() {
                match result {
                    Ok(bytes) => out.extend_from_slice(&bytes),
                    Err(error) => {
                        self.state = SessionState::Failed;
                        return Err(error.into());
                    }
                }
            }
            self.delivered += 1;
        }
        Ok(out.len() - start)
    }

    /// Ends the frame: queues the last segment, runs whatever is still
    /// queued on the calling thread, waits for tasks other threads are
    /// running, and appends the rest of the frame to `out`.
    ///
    /// # Errors
    ///
    /// [`ParallelEncodeError::InputSizeMismatch`] when the input fell short
    /// of a declared size, and the failure of a task.
    pub fn finish_into(&mut self, out: &mut Vec<u8>) -> Result<(), ParallelEncodeError> {
        if self.state != SessionState::Open {
            return Err(ParallelEncodeError::InvalidState);
        }
        if let Some(declared) = self.declared
            && self.received != declared
        {
            self.state = SessionState::Failed;
            return Err(ParallelEncodeError::InputSizeMismatch {
                declared,
                actual: self.received,
            });
        }
        if !self.pending.is_empty() {
            self.enqueue_pending();
        }
        self.run_queued();
        {
            let mut state = lock(&self.queue.shared.state);
            while state.running > 0 {
                state = self
                    .queue
                    .shared
                    .changed
                    .wait(state)
                    .unwrap_or_else(PoisonError::into_inner);
            }
        }
        self.drain_into(out)?;
        out.extend_from_slice(&self.plan.epilogue(&self.hasher));
        self.state = SessionState::Finished;
        Ok(())
    }
}
