[![MIT License](https://img.shields.io/badge/License-MIT-green.svg)](https://choosealicense.com/licenses/mit/)
[![License](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)



<p align="center">
    <p align="center">
        <img src="logo.png" alt="Logo">
    </p>
</p>

# multichannel
A mpmc priority multi channel with dynamic channel registration and freezing.

## Installation
Add to your Cargo.toml file:
```toml
[dependencies]
multichannel = "0.2.0"
```

## Features
 - Dynamic channel creation and removal
 - Priority based message selection
 - Weighted message selection
 - Channel freezing
 - Bounded and unbounded channels
 - Thread safe
 - No unsafe code
 - Multi producer and multi consumer

 ## Performance
 The amount of functionality the DynMultiReceiver provides comes at a cost. Due to the freezing feature,
 every receive() call has a worst case of O(n) complexity, where n is the amount of channels. This is because
 the DynMultiReceiver has to iterate over all channels to find the highest priority channel with a message THAT IS NOT FROZEN.
 Otherwise using a heap would be a good idea, but the freezing feature makes this impossible.
 
 So if you have a huge amount of channels and you are not using the freezing feature, you might want to consider using a different
 implementation. For most use cases, the performance should be good enough.
 
 If you can implement your logic using only basic channels, you should do that. This implementation is meant for cases where
 you need more advanced features.

## Hello World
```rust
  use multichannel::DynMultiReceiver;

 #[derive(Debug)]
 enum Msg {
     Shutdown,
     IntegerData(i32),
     FloatingData(f32),
 }

 #[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
 enum Priority {
     High,
     Low,
 }

 fn main() {
     let mrx = DynMultiReceiver::<Msg, Priority>::new();
     // Create an unfrozen channel with high priority, a dummy weight and capacity of 1
     let shutdown_sender = mrx.new_channel(Priority::High, 1, false, Some(1));

     // Create two channels with low priority
     // int_sender has a weight of 33 and float_sender has a weight of 66
     // meaning that float_sender will be twice as likely to be selected
     // when calling receive() on mrx and no higher priority channel has a msg
     let int_sender = mrx.new_channel(Priority::Low, 33, false, None);
     let float_sender = mrx.new_channel(Priority::Low, 66, false, None);

     // Send some messages
     int_sender.send(Msg::IntegerData(33)).unwrap();
     int_sender.send(Msg::IntegerData(4031)).unwrap();
     float_sender.send(Msg::FloatingData(3.14)).unwrap();
     int_sender.send(Msg::IntegerData(2)).unwrap();
     float_sender.send(Msg::FloatingData(10.0)).unwrap();
     float_sender.send(Msg::FloatingData(0.0)).unwrap();

     // Receive some messages
     for _ in 0..4 {
         println!("{:?}", mrx.receive());
     }

     // Send a shutdown message
     shutdown_sender.send(Msg::Shutdown).unwrap();

     // There are still messages left in the int channel and float channel,
     // but the shutdown message will be received first, as it has higher priority
     match mrx.receive() {
         Msg::Shutdown => println!("Received shutdown message"),
         _ => unreachable!("Expected a shutdown message"),
     }
 }
```

### Contribution
Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.