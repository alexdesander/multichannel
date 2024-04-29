use multichannel::DynMultiReceiver;

#[derive(Debug)]
#[allow(dead_code)]
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