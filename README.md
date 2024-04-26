[![MIT License](https://img.shields.io/badge/License-MIT-green.svg)](https://choosealicense.com/licenses/mit/)
[![License](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)



<p align="center">
    <p align="center">
        <img src="logo.png" alt="Logo">
    </p>
</p>

# multichannel
A multi producer single consumer bundle of channels with configurable priorities, weights and the ability to freeze specific channels.

## Installation
Add to your Cargo.toml file:
```toml
[dependencies]
multichannel = "0.1.0"
```

## Features
 - Blocking on multiple channels
 - Strict channel priorities
 - Weighted channels for selection finetuning
 - Freezing channels

## Simple demonstration
```rust
/*
    Create a multi-channel system with five channels configured as follows:

    The first channel is high priority, unbounded, not frozen, and has a weight of 1.
    The second and third channels are unbounded with medium priority.
    The fourth and fifth channels are unbounded with low priority.
    Channels are prioritized based on their order.
    The weight of a channel affects its selection probability when multiple channels
    of the same priority have pending messages. Frozen channels are not selected by
    receivers, but you can still send messages to them.
*/
use multichannel::MultiChannelBuilder;

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
assert_eq!(same_prio_msgs, ["Low Priority, Weight = 1", "Low Priority, Weight = 2"])
```

### Contribution
Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.