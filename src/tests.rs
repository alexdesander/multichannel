#[cfg(test)]
mod tests {
    use std::{
        sync::{Arc, Barrier},
        time::Duration,
    };

    use crate::DynMultiReceiver;
    use rand::{prelude::SliceRandom, thread_rng, Rng};

    #[test]
    fn creation_destruction() {
        let amount = 1000;
        let mrx = DynMultiReceiver::<i32, u16>::new();
        let mut senders = Vec::new();
        for _ in 0..amount {
            senders.push(mrx.new_channel(10, 10, false, None));
        }
        senders.shuffle(&mut rand::thread_rng());
        for sender in senders {
            mrx.remove_channel(&sender);
        }
    }

    #[test]
    fn parallel_creation_destruction() {
        let amount = 256;
        let mrx = Arc::new(DynMultiReceiver::<i32, u16>::new());
        let barrier = Arc::new(Barrier::new(256));

        let mut threads = Vec::new();
        for _ in 0..256 {
            let mrx = mrx.clone();
            let barrier = barrier.clone();
            threads.push(std::thread::spawn(move || {
                barrier.wait();
                for _ in 0..amount {
                    let sender = mrx.new_channel(10, 10, false, None);
                    mrx.remove_channel(&sender);
                }
            }));
        }
        for thread in threads {
            thread.join().unwrap();
        }
        assert!(mrx.no_channels());
    }

    #[test]
    fn send_recv_unbounded() {
        let mrx = DynMultiReceiver::<i32, u16>::new();
        let sender = mrx.new_channel(10, 10, false, None);
        let sender_high_prio = mrx.new_channel(1, 10, false, None);
        for x in 0..100 {
            sender.send(100 + x).unwrap();
        }
        for x in 0..100 {
            sender_high_prio.send(x).unwrap();
        }
        for x in 0..199 {
            assert_eq!(mrx.receive(), x);
        }
    }

    #[test]
    fn send_recv_bounded() {
        let mrx = DynMultiReceiver::<i32, u16>::new();
        let sender = mrx.new_channel(10, 10, false, Some(100));
        let sender_high_prio = mrx.new_channel(1, 10, false, Some(100));
        for x in 0..100 {
            sender.send(100 + x).unwrap();
        }
        for x in 0..100 {
            sender_high_prio.send(x).unwrap();
        }
        for x in 0..199 {
            assert_eq!(mrx.receive(), x);
        }
    }

    #[test]
    fn send_recv_bounded_0() {
        let mrx = DynMultiReceiver::<i32, u16>::new();
        let sender = mrx.new_channel(10, 10, false, Some(0));
        let sender_high_prio = mrx.new_channel(1, 10, false, Some(0));

        std::thread::spawn(move || {
            sender.send(1).unwrap();
        });
        std::thread::spawn(move || {
            sender_high_prio.send(0).unwrap();
        });

        // Let's hope the threads have enough time to send the messages
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(mrx.receive(), 0);
        assert_eq!(mrx.receive(), 1);
    }

    #[test]
    fn send_recv_unbounded_chaotic() {
        let amount_senders = 500;
        let amount_messages = 5000;
        let mrx = DynMultiReceiver::<i32, u16>::new();

        for _ in 0..amount_senders {
            let sender = mrx.new_channel(
                thread_rng().gen_range(0..10),
                thread_rng().gen_range(1..10),
                false,
                None,
            );
            std::thread::spawn(move || {
                for x in 0..amount_messages {
                    sender.send(x).unwrap();
                }
            });
        }

        let mut messages = Vec::new();
        for _ in 0..amount_senders * amount_messages {
            messages.push(mrx.receive());
        }
        assert_eq!(
            messages.len(),
            amount_senders as usize * amount_messages as usize
        );
    }

    #[test]
    fn send_recv_bounded_chaotic() {
        let amount_senders = 500;
        let amount_messages = 5000;
        let mrx = DynMultiReceiver::<i32, u16>::new();

        for _ in 0..amount_senders {
            let sender = mrx.new_channel(
                thread_rng().gen_range(0..10),
                thread_rng().gen_range(1..10),
                false,
                Some(thread_rng().gen_range(0..100)),
            );
            std::thread::spawn(move || {
                for x in 0..amount_messages {
                    sender.send(x).unwrap();
                }
            });
        }

        let mut messages = Vec::new();
        for _ in 0..amount_senders * amount_messages {
            messages.push(mrx.receive());
        }
        assert_eq!(
            messages.len(),
            amount_senders as usize * amount_messages as usize
        );
    }

    #[test]
    fn multiple_receivers_and_senders() {
        let amount_msgs = 2000;
        let mrx = DynMultiReceiver::<usize, u16>::new();
        let barrier = Arc::new(Barrier::new(1000));

        let receivers = vec![mrx; 500];
        let mut senders = Vec::new();
        for _ in 0..500 {
            senders.push(receivers[0].new_channel(
                thread_rng().gen_range(0..10),
                thread_rng().gen_range(1..10),
                false,
                Some(thread_rng().gen_range(0..100)),
            ));
        }

        // Spawn senders
        for (idx, sender) in senders.into_iter().enumerate() {
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                for x in amount_msgs * idx..amount_msgs * (idx + 1) {
                    sender.send(x).unwrap();
                }
            });
        }

        // Spawn receivers
        let mut threads = Vec::new();
        for receiver in receivers {
            let barrier = barrier.clone();
            threads.push(std::thread::spawn(move || {
                barrier.wait();
                let mut received = Vec::new();
                for _ in 0..amount_msgs {
                    received.push(receiver.receive());
                }
                received
            }));
        }

        // Join receiver threads
        let mut all_msgs = Vec::new();
        for thread in threads {
            all_msgs.append(&mut thread.join().unwrap());
        }
        all_msgs.sort_unstable();
        for (idx, msg) in all_msgs.iter().enumerate() {
            assert_eq!(idx, *msg);
        }
    }

    #[test]
    fn freeze_unfreeze() {
        let mrx = DynMultiReceiver::<i32, u16>::new();
        let sender = mrx.new_channel(10, 10, false, None);
        let sender_high_prio = mrx.new_channel(1, 10, false, None);

        for x in 100..200 {
            sender.send(x).unwrap();
        }
        for x in 0..100 {
            sender_high_prio.send(x).unwrap();
        }

        for x in 0..50 {
            assert_eq!(mrx.receive(), x);
        }
        sender_high_prio.set_frozen(true);
        for x in 100..150 {
            assert_eq!(mrx.receive(), x);
        }
        sender_high_prio.set_frozen(false);
        for x in 50..100 {
            assert_eq!(mrx.receive(), x);
        }
        for x in 150..200 {
            assert_eq!(mrx.receive(), x);
        }
    }

    #[test]
    fn one_sender_many_receivers() {
        let barrier = Arc::new(Barrier::new(100));
        let mrx = DynMultiReceiver::<i32, u16>::new();
        let sender = mrx.new_channel(10, 10, false, None);
        let mut receivers = Vec::new();
        for _ in 0..100 {
            receivers.push(mrx.clone());
        }

        for x in 0..100 {
            sender.send(x).unwrap();
        }

        let mut receiver_threads = Vec::new();
        for receiver in receivers {
            let barrier = barrier.clone();
            receiver_threads.push(std::thread::spawn(move || {
                barrier.wait();
                receiver.receive()
            }));
        }

        let mut received = Vec::new();
        for thread in receiver_threads {
            received.push(thread.join().unwrap());
        }
        received.sort_unstable();
        for (idx, msg) in received.iter().enumerate() {
            assert_eq!(idx, *msg as usize);
        }
    }

    #[test]
    fn disconnect() {
        let mrx = DynMultiReceiver::<i32, u16>::new();
        let sender = mrx.new_channel(10, 10, false, None);
        mrx.remove_channel(&sender);
        assert!(mrx.no_channels());
        assert!(sender.send(0).is_err());
    }

    #[test]
    fn drop_mrx() {
        let mrx = DynMultiReceiver::<i32, u16>::new();
        let sender = mrx.new_channel(10, 10, false, None);
        drop(mrx);
        assert!(sender.send(0).is_err());
    }
}
