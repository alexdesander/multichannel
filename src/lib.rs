//! A multi producer single consumer bundle of [`channels`] with configurable priorities,
//! weights and the ability to freeze specific channels.
//!
//! [`channels`]: https://docs.rs/crossbeam-channel/latest/crossbeam_channel/index.html
//!
//! # A simple demonstration
//! ```
//! use multichannel::MultiChannelBuilder;
//!
//! // Create a multi-channel with 5 channels.
//! // The first channel is unbounded, of highest priority, has a weight of 1 and is not frozen.
//! // The second and third channel are of second highest priority and unbounded.
//! // The fourth and fifth channel are of lowest priority and unbounded.
//! // The order of the channels is important, the priorities are determined by the order.
//! // The weight of the channels determines the probability of the channel being selected
//! // when multiple channels of the same priority have pending messages.
//! // The frozen flag determines if the channel is initially frozen, i.e. it will not be selected
//! // by the receiver at all. Sending messages to a frozen channel is still possible.
//! let (mtx, mut mrx) = MultiChannelBuilder::new()
//!     .with_channels(vec![
//!         vec![(1, None, false)],
//!         vec![(1, None, false), (2, None, false)],
//!         vec![(10, None, false), (1, None, false)],
//!     ])
//!     .build::<String>();
//!
//! mtx.send(1, "Medium Priority".to_string()).unwrap();
//! mtx.send(4, "Low Priority, Weight = 2".to_string()).unwrap();
//! mtx.send(3, "Low Priority, Weight = 1".to_string()).unwrap();
//! mtx.send(0, "High Priority".to_string()).unwrap();
//!
//! assert_eq!(mrx.recv().unwrap(), "High Priority");
//! assert_eq!(mrx.recv().unwrap(), "Medium Priority");
//! let mut same_prio_msgs = [mrx.recv().unwrap(), mrx.recv().unwrap()];
//! same_prio_msgs.sort();
//! assert_eq!(same_prio_msgs, ["Low Priority, Weight = 1", "Low Priority, Weight = 2"])
//! ```

use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};

use crossbeam_utils::sync::{Parker, Unparker};
use itertools::partition;
use rand::{
    distributions::{Distribution, WeightedIndex},
    rngs::SmallRng,
    SeedableRng,
};
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ReceiveError {
    #[error("All senders have been dropped")]
    AllSendersDropped,
}

struct Receiver<T> {
    inner: crossbeam_channel::Receiver<T>,
    frozen: Arc<AtomicBool>,
    weight: u32,
    original_index: usize,
}

pub struct MultiReceiver<T> {
    receivers: Vec<Vec<Receiver<T>>>,
    rng: SmallRng,
    parker: Parker,
    pending_msgs: Arc<AtomicUsize>,
    disconnected: Arc<AtomicBool>,
}

impl<T> MultiReceiver<T> {
    pub fn recv(&mut self) -> Result<T, ReceiveError> {
        loop {
            self.parker.park();

            // Find the lowest priority with pending messages
            let mut prio = None;
            'outer: for i in 0..self.receivers.len() {
                for j in 0..self.receivers[i].len() {
                    if !self.receivers[i][j].inner.is_empty()
                        && !self.receivers[i][j].frozen.load(Ordering::Relaxed)
                    {
                        prio = Some(i);
                        break 'outer;
                    }
                }
            }
            let Some(prio) = prio else {
                match Arc::<AtomicUsize>::strong_count(&self.pending_msgs) {
                    1 => return Err(ReceiveError::AllSendersDropped),
                    _ => {}
                }
                if self.disconnected.load(Ordering::Relaxed) {
                    return Err(ReceiveError::AllSendersDropped);
                }
                continue;
            };

            // Filter out the receivers that are frozen
            let end = partition(&mut self.receivers[prio], |r| {
                !r.frozen.load(Ordering::Relaxed)
            });

            // Filter out the empty receivers
            let end = partition(&mut self.receivers[prio][..end], |r| !r.inner.is_empty());
            let receivers = &self.receivers[prio][..end];

            // Select a receiver according to the weights
            if receivers.len() == 0 {
                continue;
            }
            if receivers.len() == 1 {
                self.parker.unparker().unpark();
                self.pending_msgs.fetch_sub(1, Ordering::Relaxed);
                match receivers[0].inner.recv() {
                    Ok(msg) => return Ok(msg),
                    Err(_) => return Err(ReceiveError::AllSendersDropped),
                }
            }
            let dist = WeightedIndex::new(receivers.iter().map(|r| r.weight)).unwrap();
            let receiver = &receivers[dist.sample(&mut self.rng)];
            self.parker.unparker().unpark();
            self.pending_msgs.fetch_sub(1, Ordering::Relaxed);
            match receiver.inner.recv() {
                Ok(msg) => return Ok(msg),
                Err(_) => return Err(ReceiveError::AllSendersDropped),
            }
        }
    }

    pub fn pending_msgs(&self) -> usize {
        self.pending_msgs.load(Ordering::Relaxed)
    }

    pub fn pending_msgs_by_channel(&self, index: usize) -> usize {
        let mut current_index = 0;
        for prio in &self.receivers {
            if current_index + prio.len() <= index {
                current_index += prio.len();
                continue;
            }

            for receiver in prio {
                if receiver.original_index == index {
                    return receiver.inner.len();
                }
            }
            unreachable!();
        }
        unreachable!("Index out of bounds");
    }
}

#[derive(Debug, Error)]
pub enum SendSerror {
    #[error("The receiver has been dropped")]
    ReceiverDropped,
}

#[derive(Clone)]
struct Sender<T> {
    inner: crossbeam_channel::Sender<T>,
    frozen: Arc<AtomicBool>,
}

#[derive(Clone)]
pub struct MultiSender<T> {
    senders: Vec<Sender<T>>,
    unparker: Unparker,
    pending_msgs: Arc<AtomicUsize>,
    disconnected: Arc<AtomicBool>,
}

impl<T> Drop for MultiSender<T> {
    fn drop(&mut self) {
        if Arc::<AtomicUsize>::strong_count(&self.pending_msgs) == 2 {
            self.disconnected.store(true, Ordering::Relaxed);
            self.unparker.unpark();
        }
    }
}

impl<T> MultiSender<T> {
    pub fn send(&self, index: usize, msg: T) -> Result<(), SendSerror> {
        self.unparker.unpark();
        if self.senders[index].inner.send(msg).is_err() {
            return Err(SendSerror::ReceiverDropped);
        }
        self.unparker.unpark();
        self.pending_msgs.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    pub fn freeze(&self, index: usize) {
        self.senders[index].frozen.store(true, Ordering::Relaxed);
    }

    pub fn unfreeze(&self, index: usize) {
        self.senders[index].frozen.store(false, Ordering::Relaxed);
    }

    pub fn pending_msgs(&self) -> usize {
        self.pending_msgs.load(Ordering::Relaxed)
    }

    pub fn pending_msgs_by_channel(&self, index: usize) -> usize {
        self.senders[index].inner.len()
    }
}

pub struct MultiChannelBuilder {
    channels: Vec<Vec<(u32, Option<usize>, bool)>>, // (weight, bounds, frozen)
}

impl MultiChannelBuilder {
    pub fn new() -> Self {
        Self {
            channels: Vec::new(),
        }
    }

    pub fn with_channels(mut self, channels: Vec<Vec<(u32, Option<usize>, bool)>>) -> Self {
        self.channels = channels;
        self
    }

    pub fn build<T>(&self) -> (MultiSender<T>, MultiReceiver<T>) {
        let mut receivers = Vec::new();
        let mut index_map = Vec::new();
        let mut senders = Vec::new();
        let mut absolute_index = 0;
        for prio in &self.channels {
            let mut same_prio_receivers = Vec::new();
            let mut same_prio_index_map = Vec::new();
            for (weight, bounds, frozen) in prio {
                let (tx, rx) = match *bounds {
                    Some(bounds) => crossbeam_channel::bounded::<T>(bounds),
                    None => crossbeam_channel::unbounded::<T>(),
                };
                let frozen = Arc::new(AtomicBool::new(*frozen));
                same_prio_receivers.push(Receiver {
                    inner: rx,
                    frozen: frozen.clone(),
                    weight: *weight,
                    original_index: absolute_index,
                });
                same_prio_index_map.push(same_prio_receivers.len() - 1);
                senders.push(Sender { inner: tx, frozen });
                absolute_index += 1;
            }
            receivers.push(same_prio_receivers);
            index_map.push(same_prio_index_map);
        }
        let parker = Parker::new();
        let unparker = parker.unparker().clone();
        let pending_msgs = Arc::new(AtomicUsize::new(0));
        let disconnected = Arc::new(AtomicBool::new(false));
        (
            MultiSender {
                senders,
                unparker,
                pending_msgs: pending_msgs.clone(),
                disconnected: disconnected.clone(),
            },
            MultiReceiver {
                receivers,
                rng: SmallRng::from_entropy(),
                parker,
                pending_msgs,
                disconnected,
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use std::thread;

    use super::*;

    #[test]
    fn test_single_message_seq() {
        let mut builder = MultiChannelBuilder::new();
        builder.channels = vec![
            vec![(2, None, false), (1, None, false)], // Highest priority
            vec![(100, None, false), (1, None, false)], // Lowest priority
        ];
        let (sender, mut receiver) = builder.build::<i32>();

        for x in 0..50000 {
            assert_eq!(receiver.pending_msgs(), 0);
            sender.send(rand::random::<usize>() % 4, x).unwrap();
            assert_eq!(receiver.pending_msgs(), 1);
            assert_eq!(receiver.recv().unwrap(), x);
        }
    }

    #[test]
    fn test_multiple_messages_seq() {
        let mut builder = MultiChannelBuilder::new();
        builder.channels = vec![
            vec![(2, None, false), (1, None, false)], // Highest priority
            vec![(10, None, false), (1, None, false)], // Middle priority
            vec![(100, None, false), (1, None, false)], // Lowest priority
        ];
        let (sender, mut receiver) = builder.build::<i32>();

        for x in 0..50000 {
            sender.send(rand::random::<usize>() % 4, x).unwrap();
        }
        assert_eq!(receiver.pending_msgs(), 50000);

        let mut received = Vec::with_capacity(50000);
        for _ in 0..50000 {
            received.push(receiver.recv().unwrap());
        }
        assert_eq!(receiver.pending_msgs(), 0);
        received.sort_unstable();
        for x in 0..50000 {
            assert_eq!(received[x], x as i32);
        }
    }

    #[test]
    fn test_multiple_batches_seq() {
        let mut builder = MultiChannelBuilder::new();
        builder.channels = vec![
            vec![(2, None, false), (1, None, false)], // Highest priority
            vec![(10, None, false), (1, None, false)], // Middle priority
            vec![(100, None, false), (1, None, false)], // Lowest priority
        ];
        let (sender, mut receiver) = builder.build::<i32>();

        for _ in 0..100 {
            for x in 0..500 {
                sender.send(rand::random::<usize>() % 4, x).unwrap();
            }
            assert_eq!(receiver.pending_msgs(), 500);
            assert_eq!(sender.pending_msgs(), 500);

            let mut received = Vec::with_capacity(500);
            for _ in 0..500 {
                received.push(receiver.recv().unwrap());
            }
            assert_eq!(receiver.pending_msgs(), 0);
            assert_eq!(sender.pending_msgs(), 0);
            received.sort_unstable();
            for x in 0..500 {
                assert_eq!(received[x], x as i32);
            }
        }
    }

    #[test]
    fn test_frozen_init_seq() {
        let mut builder = MultiChannelBuilder::new();
        builder.channels = vec![
            vec![(2, None, true), (1, None, false)], // Highest priority
            vec![(10, None, false)],                 // Middle priority
            vec![(100, None, false), (1, None, false)], // Lowest priority
        ];
        let (sender, mut receiver) = builder.build::<i32>();

        for x in 0..60000 {
            if x % 2 == 1 {
                sender.send(0, x).unwrap();
            } else {
                sender.send(rand::random::<usize>() % 4 + 1, x).unwrap();
            }
        }
        assert_eq!(receiver.pending_msgs(), 60000);

        let mut received = Vec::with_capacity(30000);
        for _ in 0..30000 {
            received.push(receiver.recv().unwrap());
        }
        assert_eq!(receiver.pending_msgs(), 30000);
        received.sort_unstable();
        for x in 0..30000 {
            assert_eq!(received[x], x as i32 * 2);
        }
        assert_eq!(receiver.pending_msgs(), 30000);
    }

    #[test]
    fn test_freezing_dynamically_from_sender_seq() {
        let mut builder = MultiChannelBuilder::new();
        builder.channels = vec![
            vec![(2, None, false), (1, None, false)], // Highest priority
            vec![(10, None, false), (49, None, false)], // Middle priority
            vec![(100, None, false), (1, None, false)], // Lowest priority
        ];
        let (sender, mut receiver) = builder.build::<i32>();
        sender.freeze(0);

        for x in 0..60000 {
            if x % 2 == 1 {
                sender.send(0, x).unwrap();
            } else {
                sender.send(rand::random::<usize>() % 5 + 1, x).unwrap();
            }
        }
        assert_eq!(receiver.pending_msgs(), 60000);

        let mut received = Vec::with_capacity(30000);
        for _ in 0..30000 {
            received.push(receiver.recv().unwrap());
        }
        assert_eq!(receiver.pending_msgs(), 30000);
        received.sort_unstable();
        for x in 0..30000 {
            assert_eq!(received[x], x as i32 * 2);
        }
        received.clear();
        sender.unfreeze(0);
        for x in 0..30000 {
            assert_eq!(receiver.recv().unwrap(), x as i32 * 2 + 1);
        }
        assert_eq!(receiver.pending_msgs(), 0);
    }

    #[test]
    fn test_par() {
        let mut builder = MultiChannelBuilder::new();
        builder.channels = vec![
            vec![(2, None, false), (1, None, false)], // Highest priority
            vec![(10, None, false)],                  // Middle priority
            vec![(100, None, false), (1, None, false)], // Lowest priority
        ];
        let (sender, mut receiver) = builder.build::<i32>();

        let send = thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(10));
            for x in 0..1000000 {
                sender.send(rand::random::<usize>() % 5, x).unwrap();
                sender.unfreeze(rand::random::<usize>() % 5);
                sender.freeze(rand::random::<usize>() % 5);
            }
            for x in 0..5 {
                sender.unfreeze(x);
            }
        });
        let receive = thread::spawn(move || {
            let mut received = Vec::new();
            for _ in 0..1000000 {
                received.push(receiver.recv().unwrap());
            }
            received.sort_unstable();
            for x in 0..1000000 {
                assert_eq!(received[x], x as i32);
            }
            assert_eq!(receiver.pending_msgs(), 0);
        });
        send.join().unwrap();
        receive.join().unwrap();
    }

    #[test]
    fn test_multiple_chaotic_par() {
        let mut builder = MultiChannelBuilder::new();
        builder.channels = vec![
            vec![(2, None, false), (1, None, false)], // Highest priority
            vec![(10, None, false), (10, None, false)], // Middle priority
            vec![(100, None, false), (1, None, false)], // Lowest priority
        ];
        let (sender, mut receiver) = builder.build::<i32>();

        let _sender = sender.clone();
        let send_0 = thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(10));
            for x in 0..50000 {
                _sender.send(rand::random::<usize>() % 5, x).unwrap();
                _sender.unfreeze(rand::random::<usize>() % 5);
                _sender.freeze(rand::random::<usize>() % 5);
            }
            for x in 0..5 {
                _sender.unfreeze(x);
            }
        });

        let _sender = sender.clone();
        let send_1 = thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(10));
            for x in 50000..100000 {
                _sender.send(rand::random::<usize>() % 5, x).unwrap();
                _sender.unfreeze(rand::random::<usize>() % 5);
                _sender.freeze(rand::random::<usize>() % 5);
            }
            for x in 0..5 {
                _sender.unfreeze(x);
            }
        });

        let _sender = sender.clone();
        let send_2 = thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(10));
            for x in 100000..150000 {
                _sender.send(rand::random::<usize>() % 5, x).unwrap();
                _sender.unfreeze(rand::random::<usize>() % 5);
                _sender.freeze(rand::random::<usize>() % 5);
            }
            for x in 0..5 {
                _sender.unfreeze(x);
            }
        });

        let send_3 = thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(10));
            for x in 150000..200000 {
                sender.send(rand::random::<usize>() % 5, x).unwrap();
                sender.unfreeze(rand::random::<usize>() % 5);
                sender.freeze(rand::random::<usize>() % 5);
            }
            for x in 0..5 {
                sender.unfreeze(x);
            }
        });

        let receive = thread::spawn(move || {
            let mut received = Vec::new();
            for _ in 0..200000 {
                received.push(receiver.recv().unwrap());
            }
            received.sort_unstable();
            for x in 0..200000 {
                assert_eq!(received[x], x as i32);
            }
            assert_eq!(receiver.pending_msgs(), 0);
        });

        send_0.join().unwrap();
        send_1.join().unwrap();
        send_2.join().unwrap();
        send_3.join().unwrap();
        receive.join().unwrap();
    }

    #[test]
    fn test_pending_msgs_by_channel() {
        for _ in 0..1000 {
            let mut builder = MultiChannelBuilder::new();
            builder.channels = vec![
                vec![(2, None, false), (1, None, false)], // Highest priority
                vec![(10, None, false), (10, None, false)], // Middle priority
                vec![(100, None, false), (1, None, false)], // Lowest priority
            ];
            let (sender, mut receiver) = builder.build::<i32>();

            for _x in 0..100 {
                sender.send(0, 1).unwrap();
            }
            for _x in 0..200 {
                sender.send(1, 1).unwrap();
            }
            for _x in 0..300 {
                sender.send(2, 1).unwrap();
            }
            for _x in 0..400 {
                sender.send(3, 1).unwrap();
            }
            for _x in 0..500 {
                sender.send(4, 1).unwrap();
            }
            for _x in 0..600 {
                sender.send(5, 1).unwrap();
            }

            assert_eq!(receiver.pending_msgs_by_channel(0), 100);
            assert_eq!(receiver.pending_msgs_by_channel(1), 200);
            assert_eq!(receiver.pending_msgs_by_channel(2), 300);
            assert_eq!(receiver.pending_msgs_by_channel(3), 400);
            assert_eq!(receiver.pending_msgs_by_channel(4), 500);
            assert_eq!(receiver.pending_msgs_by_channel(5), 600);
            assert_eq!(receiver.pending_msgs(), 2100);
            assert_eq!(sender.pending_msgs_by_channel(0), 100);
            assert_eq!(sender.pending_msgs_by_channel(1), 200);
            assert_eq!(sender.pending_msgs_by_channel(2), 300);
            assert_eq!(sender.pending_msgs_by_channel(3), 400);
            assert_eq!(sender.pending_msgs_by_channel(4), 500);
            assert_eq!(sender.pending_msgs_by_channel(5), 600);
            assert_eq!(sender.pending_msgs(), 2100);

            receiver.recv().unwrap();
            assert!(
                receiver.pending_msgs_by_channel(0) == 99
                    || receiver.pending_msgs_by_channel(1) == 199
            );
            assert!(
                sender.pending_msgs_by_channel(0) == 99 || sender.pending_msgs_by_channel(1) == 199
            );
            assert_eq!(receiver.pending_msgs_by_channel(2), 300);
            assert_eq!(receiver.pending_msgs_by_channel(3), 400);
            assert_eq!(receiver.pending_msgs(), 2099);
            for _ in 0..299 {
                receiver.recv().unwrap();
            }
            assert_eq!(receiver.pending_msgs_by_channel(0), 0);
            assert_eq!(receiver.pending_msgs_by_channel(1), 0);
            assert_eq!(receiver.pending_msgs_by_channel(2), 300);
        }
    }

    #[test]
    fn test_weight_ridiculous_difference() {
        for _ in 0..1000 {
            let mut builder = MultiChannelBuilder::new();
            builder.channels = vec![vec![(400000000, None, false), (1, None, false)]];
            let (sender, mut receiver) = builder.build::<i32>();

            for _ in 0..10000 {
                sender.send(0, 1).unwrap();
                sender.send(1, 2).unwrap();
            }

            let mut sample = Vec::new();
            for _ in 0..500 {
                sample.push(receiver.recv().unwrap());
            }
            sample.retain(|&x| x == 1);
            assert!(sample.len() >= 498);
        }
    }

    #[test]
    fn test_weight_even() {
        for _ in 0..10 {
            let mut builder = MultiChannelBuilder::new();
            builder.channels = vec![vec![(1000, None, false), (1000, None, false)]];
            let (sender, mut receiver) = builder.build::<i32>();

            for _ in 0..500000 {
                sender.send(0, 1).unwrap();
                sender.send(1, 2).unwrap();
            }

            let mut sample = Vec::new();
            for _ in 0..500000 {
                sample.push(receiver.recv().unwrap());
            }
            sample.retain(|&x| x == 1);
            assert!(sample.len() >= 500000 / 2 - 20000);
        }
    }

    #[test]
    fn hello_world() {
        let (mtx, mut mrx) = MultiChannelBuilder::new()
            .with_channels(vec![
                vec![(1, None, false)],
                vec![(1, None, false), (2, None, false)],
                vec![(10, None, false), (1, None, false)],
            ])
            .build::<String>();

        mtx.send(1, "Medium Priority".to_string()).unwrap();
        mtx.send(4, "Low Priority, Weight = 2".to_string()).unwrap();
        mtx.send(3, "Low Priority, Weight = 1".to_string()).unwrap();
        mtx.send(0, "High Priority".to_string()).unwrap();

        assert_eq!(mrx.recv().unwrap(), "High Priority");
        assert_eq!(mrx.recv().unwrap(), "Medium Priority");
        let mut same_prio_msgs = [mrx.recv().unwrap(), mrx.recv().unwrap()];
        same_prio_msgs.sort();
        assert_eq!(
            same_prio_msgs,
            ["Low Priority, Weight = 1", "Low Priority, Weight = 2"]
        )
    }

    #[test]
    fn test_all_senders_dropped() {
        let (mtx, mut mrx) = MultiChannelBuilder::new()
            .with_channels(vec![vec![(1, None, false)]])
            .build::<String>();

        drop(mtx);
        assert_eq!(mrx.recv(), Err(ReceiveError::AllSendersDropped));
    }
}
